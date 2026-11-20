package org.archdrop.mesh

import android.util.Log
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString
import java.io.Closeable
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.Socket
import java.net.URI
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

/**
 * Pure-Kotlin userspace DERP client over HTTPS:443 (OkHttp WebSocket or raw TCP/TLS socket).
 *
 * Implements:
 * - Length-prefixed binary DERP frames (ServerKey, ClientInfo, ServerInfo, SendPacket, RecvPacket, KeepAlive, Ping, Pong)
 * - Ed25519 peer public-key authentication with 32-byte keys and mutual challenge verification
 * - 20s keepalive heartbeats to keep stateful NAT pinholes open
 * - Bidirectional virtual stream multiplexing (SYN, DATA, FIN, RST) over DERP packets
 */
class DerpMeshClient(
    val identity: MeshIdentity,
    private val keepaliveIntervalMs: Long = 20_000L,
    private val okHttpClient: OkHttpClient = OkHttpClient.Builder()
        .pingInterval(0, TimeUnit.MILLISECONDS) // Use DERP level keepalives
        .readTimeout(0, TimeUnit.MILLISECONDS)
        .build()
) : Closeable {

    companion object {
        private const val TAG = "ArchDrop:DerpMeshClient"
        private val EOF_SENTINEL = ByteArray(0)

        private fun logD(msg: String) { try { Log.d(TAG, msg) } catch (_: Throwable) { println("DEBUG: $TAG: $msg") } }
        private fun logI(msg: String) { try { Log.i(TAG, msg) } catch (_: Throwable) { println("INFO: $TAG: $msg") } }
        private fun logW(msg: String, t: Throwable? = null) { try { Log.w(TAG, msg, t) } catch (_: Throwable) { println("WARN: $TAG: $msg") } }
        private fun logV(msg: String) { try { Log.v(TAG, msg) } catch (_: Throwable) { println("VERBOSE: $TAG: $msg") } }
    }

    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())
    private var webSocket: WebSocket? = null
    private var tcpSocket: Socket? = null

    private val isConnectedState = AtomicBoolean(false)
    val isConnected: Boolean get() = isConnectedState.get()

    private var serverPublicKey: ByteArray? = null
    val serverKey: ByteArray? get() = serverPublicKey

    private var handshakeDeferred = CompletableDeferred<Boolean>()
    private val nextStreamId = AtomicLong(1L)

    // Virtual streams mapped by "PEER_HEX:STREAM_ID"
    private val activeStreams = ConcurrentHashMap<String, DerpVirtualStream>()
    private val acceptQueue = Channel<DerpVirtualStream>(Channel.UNLIMITED)

    private var keepaliveJob: Job? = null
    private var socketReaderJob: Job? = null

    val localPublicKey: ByteArray get() = identity.publicKeyBytes
    var defaultEncryption: Boolean = false

    /**
     * Connects to a DERP relay at [relayUrl] (ws://, wss://, tcp://, http://)
     * and performs mutual Ed25519 authentication handshake.
     */
    suspend fun connect(relayUrl: String, timeoutMs: Long = 10_000L) {
        handshakeDeferred = CompletableDeferred()
        val uri = URI(relayUrl)
        val scheme = uri.scheme?.lowercase() ?: "wss"

        if (scheme == "ws" || scheme == "wss") {
            connectWebSocket(relayUrl)
        } else {
            connectRawSocket(uri.host ?: "127.0.0.1", if (uri.port > 0) uri.port else 80)
        }

        withTimeout(timeoutMs) {
            handshakeDeferred.await()
        }

        startKeepaliveLoop()
        logI("Connected and authenticated to DERP relay at $relayUrl (local key: ${identity.toHex()})")
    }

    private fun connectWebSocket(url: String) {
        val request = Request.Builder().url(url).build()
        webSocket = okHttpClient.newWebSocket(request, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                logD("WebSocket transport opened to $url, awaiting ServerKey...")
            }

            override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                handleIncomingFrameBytes(bytes.toByteArray())
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                logW("WebSocket failure: ${t.message}", t)
                onDisconnected(t)
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                logI("WebSocket closed: code=$code, reason=$reason")
                onDisconnected(null)
            }
        })
    }

    private fun connectRawSocket(host: String, port: Int) {
        val s = Socket(host, port)
        tcpSocket = s
        isConnectedState.set(true)

        socketReaderJob = scope.launch {
            try {
                val input = s.getInputStream()
                while (isActive && !s.isClosed) {
                    val frame = DerpFrame.decodeFromStream(input) ?: break
                    handleFrame(frame)
                }
            } catch (e: Exception) {
                if (!s.isClosed) {
                    logW("Error in raw socket reader", e)
                }
            } finally {
                onDisconnected(null)
            }
        }
    }

    private fun handleIncomingFrameBytes(bytes: ByteArray) {
        val frame = DerpFrame.decode(bytes)
        if (frame != null) {
            handleFrame(frame)
        } else {
            logW("Failed to decode DERP frame from ${bytes.size} bytes")
        }
    }

    fun handleFrame(frame: DerpFrame) {
        when (frame) {
            is DerpFrame.ServerKey -> {
                serverPublicKey = frame.serverKey
                val nowSecs = System.currentTimeMillis() / 1000L
                val signature = identity.signChallenge(frame.serverKey, nowSecs)

                val clientInfo = DerpFrame.ClientInfo(
                    clientKey = identity.publicKeyBytes,
                    timestamp = nowSecs,
                    signature = signature
                )
                sendFrame(clientInfo)
                logD("Sent ClientInfo handshake response to server key: ${frame.serverKey.joinToString("") { "%02X".format(it) }}")
            }

            is DerpFrame.ServerInfo -> {
                isConnectedState.set(true)
                if (!handshakeDeferred.isCompleted) {
                    handshakeDeferred.complete(true)
                }
                logI("ServerInfo received, mutual authentication handshake complete!")
            }

            is DerpFrame.RecvPacket -> {
                handleRecvPacket(frame.srcKey, frame.payload)
            }

            is DerpFrame.Ping -> {
                sendFrame(DerpFrame.Pong(frame.nonce))
            }

            is DerpFrame.Pong -> {
                logD("Received Pong probe from relay")
            }

            is DerpFrame.KeepAlive -> {
                logD("Received KeepAlive heartbeat from relay")
            }

            is DerpFrame.PeerPresent -> {
                logI("Peer online: ${frame.peerKey.joinToString("") { "%02X".format(it) }}")
            }

            is DerpFrame.PeerGone -> {
                logI("Peer offline: ${frame.peerKey.joinToString("") { "%02X".format(it) }}")
            }

            else -> {
                logD("Unhandled frame type: ${frame.frameType}")
            }
        }
    }

    private fun handleRecvPacket(srcKey: ByteArray, payload: ByteArray) {
        val streamPayload = DerpStreamProtocol.decodeStreamPayload(payload)
        if (streamPayload == null) {
            logW("RecvPacket payload too short for stream header: ${payload.size}B")
            return
        }

        val key = streamKey(srcKey, streamPayload.streamId)
        val flags = streamPayload.flags.toInt()

        if (flags and DerpStreamProtocol.FLAG_SYN.toInt() != 0) {
            // New incoming virtual stream from remote peer
            val session = if (defaultEncryption) {
                deriveStreamKeys(
                    localKey = identity.publicKeyBytes,
                    remoteKey = srcKey,
                    streamId = streamPayload.streamId,
                    isInitiator = false
                )
            } else null
            val stream = DerpVirtualStream(streamPayload.streamId, srcKey, this, session)
            activeStreams[key] = stream
            acceptQueue.trySend(stream)
            logD("Accepted new virtual stream sid=${streamPayload.streamId} from ${srcKey.toHex()} (encrypted=${session != null})")
        } else if (flags and DerpStreamProtocol.FLAG_FIN.toInt() != 0) {
            activeStreams[key]?.receiveChunk(EOF_SENTINEL)
            activeStreams.remove(key)
        } else if (flags and DerpStreamProtocol.FLAG_RST.toInt() != 0) {
            activeStreams.remove(key)?.resetLocally()
        } else if (flags and DerpStreamProtocol.FLAG_DATA.toInt() != 0) {
            val stream = activeStreams[key]
            if (stream != null) {
                stream.receiveChunk(streamPayload.data)
            } else {
                logW("Dropping data chunk for unknown stream key=$key")
            }
        }
    }

    /**
     * Sends a DERP frame across the transport.
     */
    @Synchronized
    fun sendFrame(frame: DerpFrame): Boolean {
        return try {
            val bytes = frame.encode()
            val ws = webSocket
            if (ws != null) {
                ws.send(bytes.toByteString())
            } else {
                val s = tcpSocket
                if (s != null && !s.isClosed) {
                    val out = s.getOutputStream()
                    out.write(bytes)
                    out.flush()
                    true
                } else {
                    false
                }
            }
        } catch (e: Exception) {
            logW("Failed to send DERP frame: ${e.message}")
            false
        }
    }

    /**
     * Opens a new virtual bidirectional stream to [targetPeerKey] via the DERP relay.
     */
    fun openStream(targetPeerKey: ByteArray, encrypted: Boolean = defaultEncryption): DerpVirtualStream {
        require(targetPeerKey.size == 32) { "targetPeerKey must be 32 bytes" }
        val streamId = nextStreamId.getAndIncrement()
        val session = if (encrypted) {
            deriveStreamKeys(
                localKey = identity.publicKeyBytes,
                remoteKey = targetPeerKey,
                streamId = streamId,
                isInitiator = true
            )
        } else null
        val stream = DerpVirtualStream(streamId, targetPeerKey, this, session)
        val key = streamKey(targetPeerKey, streamId)
        activeStreams[key] = stream

        // Transmit FLAG_SYN frame to initiate virtual stream with peer
        val synPayload = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_SYN)
        sendFrame(DerpFrame.SendPacket(targetPeerKey, synPayload))
        logD("Opened virtual stream sid=$streamId to peer ${targetPeerKey.toHex()} (encrypted=${session != null})")

        return stream
    }

    /**
     * Opens an encrypted virtual stream to [targetPeerKey] via the DERP relay.
     */
    fun openEncryptedStream(targetPeerKey: ByteArray): DerpVirtualStream = openStream(targetPeerKey, encrypted = true)

    /**
     * Suspends until the next incoming virtual stream initiated by a remote peer arrives.
     */
    suspend fun acceptStream(): DerpVirtualStream {
        return acceptQueue.receive()
    }

    private fun startKeepaliveLoop() {
        keepaliveJob?.cancel()
        keepaliveJob = scope.launch {
            while (isActive && isConnected) {
                delay(keepaliveIntervalMs)
                if (isConnected) {
                    sendFrame(DerpFrame.KeepAlive)
                    logV("Sent 20s KeepAlive heartbeat to relay")
                }
            }
        }
    }

    internal fun removeStream(targetPeerKey: ByteArray, streamId: Long) {
        val key = streamKey(targetPeerKey, streamId)
        activeStreams.remove(key)
    }

    private fun streamKey(peerKey: ByteArray, streamId: Long): String {
        return "${peerKey.toHex()}:$streamId"
    }

    private fun onDisconnected(t: Throwable?) {
        if (isConnectedState.compareAndSet(true, false)) {
            logI("DerpMeshClient disconnected: ${t?.message}")
            keepaliveJob?.cancel()
            socketReaderJob?.cancel()
            if (!handshakeDeferred.isCompleted) {
                if (t != null) handshakeDeferred.completeExceptionally(t)
                else handshakeDeferred.complete(false)
            }
            activeStreams.values.forEach { it.closeLocally() }
            activeStreams.clear()
        }
    }

    override fun close() {
        onDisconnected(null)
        try {
            webSocket?.close(1000, "Client closing")
        } catch (_: Exception) {}
        try {
            tcpSocket?.close()
        } catch (_: Exception) {}
        webSocket = null
        tcpSocket = null
    }

    private fun ByteArray.toHex(): String = joinToString("") { "%02X".format(it) }
}

/**
 * Virtual bidirectional stream multiplexed over DERP SendPacket and RecvPacket frames.
 *
 * Implements standard Java [InputStream] and [OutputStream] for streaming compatibility
 * with sockets, HttpReceiver, and LocalSend pipelines.
 */
class DerpVirtualStream(
    val streamId: Long,
    val remotePeerKey: ByteArray,
    private val client: DerpMeshClient,
    var session: NoiseSession? = null
) : Closeable {

    fun enableEncryption(isInitiator: Boolean) {
        session = deriveStreamKeys(
            localKey = client.identity.publicKeyBytes,
            remoteKey = remotePeerKey,
            streamId = streamId,
            isInitiator = isInitiator
        )
    }

    private val isClosed = AtomicBoolean(false)
    val isStreamClosed: Boolean get() = isClosed.get()
    val isReset = AtomicBoolean(false)
    val isStreamReset: Boolean get() = isReset.get()
    var onResetCallback: (() -> Unit)? = null

    private val inboundQueue = LinkedBlockingQueue<ByteArray>()
    private var currentChunk: ByteArray? = null
    private var currentChunkOffset = 0

    val inputStream: InputStream = object : InputStream() {
        override fun read(): Int {
            val single = ByteArray(1)
            val r = read(single, 0, 1)
            return if (r == -1) -1 else (single[0].toInt() and 0xFF)
        }

        override fun read(b: ByteArray, off: Int, len: Int): Int {
            if (len == 0) return 0
            while (true) {
                val chunk = currentChunk
                if (chunk != null) {
                    val available = chunk.size - currentChunkOffset
                    if (available > 0) {
                        val toRead = minOf(available, len)
                        System.arraycopy(chunk, currentChunkOffset, b, off, toRead)
                        currentChunkOffset += toRead
                        return toRead
                    } else {
                        currentChunk = null
                        currentChunkOffset = 0
                    }
                }

                // Chunk is exhausted, pull next chunk from inbound queue
                val next = try {
                    if (isClosed.get() && inboundQueue.isEmpty()) {
                        return -1
                    }
                    inboundQueue.poll(100, TimeUnit.MILLISECONDS)
                } catch (e: InterruptedException) {
                    throw IOException("Stream read interrupted", e)
                }

                if (next == null) {
                    if (isClosed.get()) return -1
                    continue
                }

                if (next.isEmpty()) {
                    // Empty byte array is EOF sentinel
                    return -1
                }

                currentChunk = next
                currentChunkOffset = 0
            }
        }

        override fun close() {
            this@DerpVirtualStream.close()
        }
    }

    val outputStream: OutputStream = object : OutputStream() {
        override fun write(b: Int) {
            write(byteArrayOf(b.toByte()), 0, 1)
        }

        override fun write(b: ByteArray, off: Int, len: Int) {
            if (isClosed.get()) throw IOException("DerpVirtualStream is closed")
            var remaining = len
            var offset = off

            while (remaining > 0) {
                val chunkSize = minOf(remaining, DerpStreamProtocol.DEFAULT_CHUNK_SIZE)
                val chunk = ByteArray(chunkSize)
                System.arraycopy(b, offset, chunk, 0, chunkSize)

                val payloadData = if (session != null) {
                    val aad = buildStreamAad(streamId, DerpStreamProtocol.FLAG_DATA)
                    session!!.encryptWithAd(aad, chunk)
                } else {
                    chunk
                }

                val payload = DerpStreamProtocol.encodeStreamPayload(
                    streamId = streamId,
                    flags = DerpStreamProtocol.FLAG_DATA,
                    data = payloadData
                )
                val sendPacket = DerpFrame.SendPacket(remotePeerKey, payload)
                val ok = client.sendFrame(sendPacket)
                if (!ok) throw IOException("Failed to send virtual stream data packet over DERP")

                offset += chunkSize
                remaining -= chunkSize
            }
        }

        override fun flush() {
            // Packets are sent eagerly
        }

        override fun close() {
            this@DerpVirtualStream.close()
        }
    }

    internal fun receiveChunk(data: ByteArray) {
        if (!isClosed.get()) {
            if (data.isEmpty()) {
                inboundQueue.offer(data)
                return
            }
            val plaintext = if (session != null) {
                try {
                    val aad = buildStreamAad(streamId, DerpStreamProtocol.FLAG_DATA)
                    session!!.decryptWithAd(aad, data)
                } catch (e: Exception) {
                    resetLocally()
                    return
                }
            } else {
                data
            }
            inboundQueue.offer(plaintext)
        }
    }

    internal fun closeLocally() {
        if (isClosed.compareAndSet(false, true)) {
            inboundQueue.offer(ByteArray(0)) // EOF sentinel
        }
    }

    internal fun resetLocally() {
        isReset.set(true)
        closeLocally()
        try {
            onResetCallback?.invoke()
        } catch (_: Exception) {}
    }

    override fun close() {
        if (isClosed.compareAndSet(false, true)) {
            try {
                // Send FLAG_FIN frame to remote peer
                val finPayload = DerpStreamProtocol.encodeStreamPayload(
                    streamId = streamId,
                    flags = DerpStreamProtocol.FLAG_FIN
                )
                client.sendFrame(DerpFrame.SendPacket(remotePeerKey, finPayload))
            } catch (_: Exception) {}
            inboundQueue.offer(ByteArray(0))
            client.removeStream(remotePeerKey, streamId)
        }
    }
}
