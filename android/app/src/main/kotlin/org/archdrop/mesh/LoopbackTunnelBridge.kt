package org.archdrop.mesh

import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.io.Closeable
import java.io.InputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Userspace bridge on 127.0.0.1 forwarding bidirectional transfer and sync streams
 * between LocalSend HttpReceiver (127.0.0.1:53317) and the DERP relay mesh client.
 *
 * Guaranteed Commercial VPN Coexistence:
 * Strictly binds to 127.0.0.1 loopback. Under Linux/Android kernel routing policy database (RPDB),
 * Rule 0 (`0: from all lookup local`) evaluates 127.0.0.1 immediately in table local,
 * terminating route lookup before reaching VPN routing tables (>= 10000) on tun0.
 * As a result, transfers over this loopback bridge run completely uninhibited alongside
 * active commercial VPNs (e.g. AdGuard, Mullvad, ProtonVPN).
 */
class LoopbackTunnelBridge(
    private val derpClient: DerpMeshClient,
    val httpReceiverPort: Int = 53317,
    val bridgeListenPort: Int = 53318,
    var targetDesktopPeerKey: ByteArray? = null
) : Closeable {

    companion object {
        private const val TAG = "ArchDrop:LoopbackBridge"
        private const val BUFFER_SIZE = 32 * 1024
        val LOOPBACK_ADDR: InetAddress = InetAddress.getByName("127.0.0.1")

        private fun logD(msg: String) { try { Log.d(TAG, msg) } catch (_: Throwable) { println("DEBUG: $TAG: $msg") } }
        private fun logI(msg: String) { try { Log.i(TAG, msg) } catch (_: Throwable) { println("INFO: $TAG: $msg") } }
        private fun logW(msg: String, t: Throwable? = null) { try { Log.w(TAG, msg, t) } catch (_: Throwable) { println("WARN: $TAG: $msg") } }
    }

    private val isRunning = AtomicBoolean(false)
    val active: Boolean get() = isRunning.get()

    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())
    private var inboundBridgeJob: Job? = null
    private var outboundBridgeJob: Job? = null
    private var serverSocket: ServerSocket? = null

    /**
     * Starts the loopback bridge:
     * 1. Inbound worker: accepts virtual streams from [DerpMeshClient] and pipes them to 127.0.0.1:[httpReceiverPort].
     * 2. Outbound listener: listens on 127.0.0.1:[bridgeListenPort] and pipes connections to [DerpMeshClient.openStream].
     */
    fun start() {
        if (!isRunning.compareAndSet(false, true)) return
        logI("Starting LoopbackTunnelBridge (forwarding to 127.0.0.1:$httpReceiverPort)...")

        // 1. Inbound bridge from DERP to local HttpReceiver
        inboundBridgeJob = scope.launch {
            while (isActive && isRunning.get()) {
                try {
                    val virtualStream = derpClient.acceptStream()
                    launch {
                        handleInboundVirtualStream(virtualStream)
                    }
                } catch (e: Exception) {
                    if (!isRunning.get()) break
                    logW("Exception accepting virtual stream from DERP client", e)
                }
            }
        }

        // 2. Outbound listener on loopback
        try {
            val ss = ServerSocket(bridgeListenPort, 50, LOOPBACK_ADDR)
            serverSocket = ss
            logI("Loopback bridge listening on 127.0.0.1:$bridgeListenPort")

            outboundBridgeJob = scope.launch {
                while (isActive && isRunning.get() && !ss.isClosed) {
                    try {
                        val clientSocket = ss.accept()
                        launch {
                            handleOutboundLocalConnection(clientSocket)
                        }
                    } catch (e: Exception) {
                        if (!isRunning.get() || ss.isClosed) break
                        logW("Exception accepting local loopback connection", e)
                    }
                }
            }
        } catch (e: Exception) {
            logW("Could not bind outbound bridge port $bridgeListenPort: ${e.message}")
        }
    }

    /**
     * Pipes an inbound DERP virtual stream into the local HttpReceiver on 127.0.0.1:[httpReceiverPort].
     */
    private suspend fun handleInboundVirtualStream(virtualStream: DerpVirtualStream) {
        try {
            Socket(LOOPBACK_ADDR, httpReceiverPort).use { localSocket ->
                localSocket.tcpNoDelay = true
                virtualStream.onResetCallback = {
                    try { localSocket.close() } catch (_: Exception) {}
                }
                pipeBidirectional(
                    streamIn = virtualStream.inputStream,
                    streamOut = virtualStream.outputStream,
                    sockIn = localSocket.getInputStream(),
                    sockOut = localSocket.getOutputStream(),
                    localSocket = localSocket,
                    virtualStream = virtualStream
                )
            }
        } catch (e: Exception) {
            logW("Error bridging inbound virtual stream ${virtualStream.streamId} to HttpReceiver: ${e.message}")
        } finally {
            virtualStream.close()
        }
    }

    /**
     * Pipes a local loopback client socket out through a DERP virtual stream to [targetDesktopPeerKey].
     */
    private suspend fun handleOutboundLocalConnection(localSocket: Socket) {
        val targetKey = targetDesktopPeerKey
        if (targetKey == null) {
            logW("Cannot forward outbound connection: targetDesktopPeerKey is not configured")
            localSocket.close()
            return
        }

        try {
            val virtualStream = derpClient.openStream(targetKey)
            localSocket.use { sock ->
                sock.tcpNoDelay = true
                virtualStream.onResetCallback = {
                    try { sock.close() } catch (_: Exception) {}
                }
                pipeBidirectional(
                    streamIn = virtualStream.inputStream,
                    streamOut = virtualStream.outputStream,
                    sockIn = sock.getInputStream(),
                    sockOut = sock.getOutputStream(),
                    localSocket = sock,
                    virtualStream = virtualStream
                )
            }
        } catch (e: Exception) {
            logW("Error bridging outbound connection to DERP stream: ${e.message}")
        }
    }

    /**
     * Robust bidirectional byte piping between a virtual DERP stream and a TCP socket.
     */
    private suspend fun pipeBidirectional(
        streamIn: InputStream,
        streamOut: OutputStream,
        sockIn: InputStream,
        sockOut: OutputStream,
        localSocket: Socket? = null,
        virtualStream: DerpVirtualStream? = null
    ) {
        val forwardJob = scope.launch {
            try {
                pipeStream(streamIn, sockOut)
            } catch (_: Exception) {}
            finally {
                try { sockOut.flush() } catch (_: Exception) {}
                if (virtualStream?.isStreamReset == true) {
                    try { localSocket?.close() } catch (_: Exception) {}
                } else {
                    try { localSocket?.shutdownOutput() } catch (_: Exception) {}
                }
            }
        }

        val reverseJob = scope.launch {
            try {
                pipeStream(sockIn, streamOut)
            } catch (_: Exception) {}
            finally {
                try { streamOut.flush() } catch (_: Exception) {}
                forwardJob.cancel()
            }
        }

        reverseJob.join()
        forwardJob.cancel()
    }

    private fun pipeStream(input: InputStream, output: OutputStream) {
        val buffer = ByteArray(BUFFER_SIZE)
        var bytesRead: Int
        while (input.read(buffer).also { bytesRead = it } != -1) {
            output.write(buffer, 0, bytesRead)
            output.flush()
        }
    }

    override fun close() {
        if (isRunning.compareAndSet(true, false)) {
            logI("Stopping LoopbackTunnelBridge...")
            inboundBridgeJob?.cancel()
            outboundBridgeJob?.cancel()
            try {
                serverSocket?.close()
            } catch (_: Exception) {}
            serverSocket = null
        }
    }
}
