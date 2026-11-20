package org.archdrop.mesh

import java.io.ByteArrayOutputStream
import java.io.EOFException
import java.io.InputStream
import java.io.OutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Canonical DERP frame types matching crates/archdrop_mesh/src/frame.rs.
 */
enum class DerpFrameType(val code: Byte) {
    SERVER_KEY(0x01),
    CLIENT_INFO(0x02),
    SERVER_INFO(0x03),
    SEND_PACKET(0x04),
    RECV_PACKET(0x05),
    KEEP_ALIVE(0x06),
    PEER_GONE(0x07),
    PEER_PRESENT(0x08),
    PING(0x09),
    PONG(0x0a);

    companion object {
        fun fromByte(code: Byte): DerpFrameType? {
            return when (code.toInt() and 0xFF) {
                0x01 -> SERVER_KEY
                0x02 -> CLIENT_INFO
                0x03 -> SERVER_INFO
                0x04 -> SEND_PACKET
                0x05 -> RECV_PACKET
                0x06 -> KEEP_ALIVE
                0x07 -> PEER_GONE
                0x08 -> PEER_PRESENT
                0x09, 0x0c -> PING
                0x0a, 0x0d -> PONG
                else -> null
            }
        }
    }
}

/**
 * Strongly typed length-prefixed binary DERP protocol frames.
 *
 * Wire format:
 * `[FrameType: 1B][PayloadLength: 4B BE][Payload: NB]`
 */
sealed class DerpFrame(val frameType: DerpFrameType) {

    /**
     * Relay public key (32B) and optional challenge salt sent to connecting client.
     */
    data class ServerKey(
        val serverKey: ByteArray,
        val challenge: ByteArray = ByteArray(0)
    ) : DerpFrame(DerpFrameType.SERVER_KEY) {
        init {
            require(serverKey.size == 32) { "serverKey must be 32 bytes" }
        }

        override fun equals(other: Any?): Boolean {
            if (this === other) return true
            if (javaClass != other?.javaClass) return false
            other as ServerKey
            return serverKey.contentEquals(other.serverKey) && challenge.contentEquals(other.challenge)
        }

        override fun hashCode(): Int = 31 * serverKey.contentHashCode() + challenge.contentHashCode()
    }

    /**
     * Client authentication sent to relay: client public key (32B), Unix timestamp (8B BE), and Ed25519 signature (64B).
     */
    data class ClientInfo(
        val clientKey: ByteArray,
        val timestamp: Long,
        val signature: ByteArray
    ) : DerpFrame(DerpFrameType.CLIENT_INFO) {
        init {
            require(clientKey.size == 32) { "clientKey must be 32 bytes" }
            require(signature.size == 64) { "signature must be 64 bytes" }
        }

        override fun equals(other: Any?): Boolean {
            if (this === other) return true
            if (javaClass != other?.javaClass) return false
            other as ClientInfo
            return clientKey.contentEquals(other.clientKey) &&
                    timestamp == other.timestamp &&
                    signature.contentEquals(other.signature)
        }

        override fun hashCode(): Int {
            var result = clientKey.contentHashCode()
            result = 31 * result + timestamp.hashCode()
            result = 31 * result + signature.contentHashCode()
            return result
        }
    }

    /**
     * Server acceptance metadata and session parameters (JSON or UTF-8).
     */
    data class ServerInfo(val data: ByteArray) : DerpFrame(DerpFrameType.SERVER_INFO) {
        override fun equals(other: Any?): Boolean {
            if (this === other) return true
            if (javaClass != other?.javaClass) return false
            other as ServerInfo
            return data.contentEquals(other.data)
        }

        override fun hashCode(): Int = data.contentHashCode()
    }

    /**
     * Forward packet destined to a remote peer: [dstKey: 32B] + [payload: NB].
     */
    data class SendPacket(
        val dstKey: ByteArray,
        val payload: ByteArray
    ) : DerpFrame(DerpFrameType.SEND_PACKET) {
        init {
            require(dstKey.size == 32) { "dstKey must be 32 bytes" }
        }

        override fun equals(other: Any?): Boolean {
            if (this === other) return true
            if (javaClass != other?.javaClass) return false
            other as SendPacket
            return dstKey.contentEquals(other.dstKey) && payload.contentEquals(other.payload)
        }

        override fun hashCode(): Int = 31 * dstKey.contentHashCode() + payload.contentHashCode()
    }

    /**
     * Packet delivered to client from a remote peer: [srcKey: 32B] + [payload: NB].
     */
    data class RecvPacket(
        val srcKey: ByteArray,
        val payload: ByteArray
    ) : DerpFrame(DerpFrameType.RECV_PACKET) {
        init {
            require(srcKey.size == 32) { "srcKey must be 32 bytes" }
        }

        override fun equals(other: Any?): Boolean {
            if (this === other) return true
            if (javaClass != other?.javaClass) return false
            other as RecvPacket
            return srcKey.contentEquals(other.srcKey) && payload.contentEquals(other.payload)
        }

        override fun hashCode(): Int = 31 * srcKey.contentHashCode() + payload.contentHashCode()
    }

    /**
     * Empty heartbeat frame (0 payload bytes) to keep stateful NAT firewall pinholes open.
     */
    object KeepAlive : DerpFrame(DerpFrameType.KEEP_ALIVE) {
        override fun toString(): String = "DerpFrame.KeepAlive"
    }

    /**
     * Relay notification that a peer is offline: [peerKey: 32B].
     */
    data class PeerGone(val peerKey: ByteArray) : DerpFrame(DerpFrameType.PEER_GONE) {
        init {
            require(peerKey.size == 32) { "peerKey must be 32 bytes" }
        }

        override fun equals(other: Any?): Boolean {
            if (this === other) return true
            if (javaClass != other?.javaClass) return false
            other as PeerGone
            return peerKey.contentEquals(other.peerKey)
        }

        override fun hashCode(): Int = peerKey.contentHashCode()
    }

    /**
     * Relay notification that a peer is online: [peerKey: 32B].
     */
    data class PeerPresent(val peerKey: ByteArray) : DerpFrame(DerpFrameType.PEER_PRESENT) {
        init {
            require(peerKey.size == 32) { "peerKey must be 32 bytes" }
        }

        override fun equals(other: Any?): Boolean {
            if (this === other) return true
            if (javaClass != other?.javaClass) return false
            other as PeerPresent
            return peerKey.contentEquals(other.peerKey)
        }

        override fun hashCode(): Int = peerKey.contentHashCode()
    }

    /**
     * Ping probe with 8-byte nonce.
     */
    data class Ping(val nonce: ByteArray) : DerpFrame(DerpFrameType.PING) {
        init {
            require(nonce.size == 8) { "nonce must be 8 bytes" }
        }

        override fun equals(other: Any?): Boolean {
            if (this === other) return true
            if (javaClass != other?.javaClass) return false
            other as Ping
            return nonce.contentEquals(other.nonce)
        }

        override fun hashCode(): Int = nonce.contentHashCode()
    }

    /**
     * Pong echo with matching 8-byte nonce.
     */
    data class Pong(val nonce: ByteArray) : DerpFrame(DerpFrameType.PONG) {
        init {
            require(nonce.size == 8) { "nonce must be 8 bytes" }
        }

        override fun equals(other: Any?): Boolean {
            if (this === other) return true
            if (javaClass != other?.javaClass) return false
            other as Pong
            return nonce.contentEquals(other.nonce)
        }

        override fun hashCode(): Int = nonce.contentHashCode()
    }

    /**
     * Encodes this frame into length-prefixed bytes:
     * `[frameType: 1B][payloadLen: 4B BE][payload: NB]`
     */
    fun encode(): ByteArray {
        val payload = when (this) {
            is ServerKey -> {
                val buf = ByteBuffer.allocate(32 + challenge.size)
                buf.put(serverKey)
                buf.put(challenge)
                buf.array()
            }
            is ClientInfo -> {
                val buf = ByteBuffer.allocate(32 + 8 + 64).order(ByteOrder.BIG_ENDIAN)
                buf.put(clientKey)
                buf.putLong(timestamp)
                buf.put(signature)
                buf.array()
            }
            is ServerInfo -> data
            is SendPacket -> {
                val buf = ByteBuffer.allocate(32 + payload.size)
                buf.put(dstKey)
                buf.put(payload)
                buf.array()
            }
            is RecvPacket -> {
                val buf = ByteBuffer.allocate(32 + payload.size)
                buf.put(srcKey)
                buf.put(payload)
                buf.array()
            }
            is KeepAlive -> ByteArray(0)
            is PeerGone -> peerKey
            is PeerPresent -> peerKey
            is Ping -> nonce
            is Pong -> nonce
        }

        val totalLen = 5 + payload.size
        val out = ByteBuffer.allocate(totalLen).order(ByteOrder.BIG_ENDIAN)
        out.put(frameType.code)
        out.putInt(payload.size)
        out.put(payload)
        return out.array()
    }

    fun writeTo(output: OutputStream) {
        output.write(encode())
        output.flush()
    }

    companion object {
        const val MAX_FRAME_SIZE = 16 * 1024 * 1024

        /**
         * Decodes a single DERP frame from raw bytes.
         * Returns the decoded frame or null if bytes are insufficient or invalid.
         */
        fun decode(bytes: ByteArray): DerpFrame? {
            if (bytes.size < 5) return null
            val buf = ByteBuffer.wrap(bytes).order(ByteOrder.BIG_ENDIAN)
            val typeByte = buf.get()
            val payloadLen = buf.int
            if (payloadLen < 0 || payloadLen > MAX_FRAME_SIZE) return null
            if (bytes.size < 5 + payloadLen) return null

            val fType = DerpFrameType.fromByte(typeByte) ?: return null
            val payload = ByteArray(payloadLen)
            buf.get(payload)

            return parsePayload(fType, payload)
        }

        /**
         * Decodes a single DERP frame from an InputStream in blocking fashion.
         * Returns null on clean EOF at frame boundary.
         */
        fun decodeFromStream(input: InputStream): DerpFrame? {
            val typeByteInt = input.read()
            if (typeByteInt == -1) return null
            val typeByte = typeByteInt.toByte()

            val lenBytes = ByteArray(4)
            readFully(input, lenBytes)
            val payloadLen = ByteBuffer.wrap(lenBytes).order(ByteOrder.BIG_ENDIAN).int
            if (payloadLen < 0 || payloadLen > MAX_FRAME_SIZE) {
                throw IllegalArgumentException("Invalid DERP frame payload length: $payloadLen")
            }

            val payload = ByteArray(payloadLen)
            if (payloadLen > 0) {
                readFully(input, payload)
            }

            val fType = DerpFrameType.fromByte(typeByte)
                ?: throw IllegalArgumentException("Unknown DERP frame type: 0x%02x".format(typeByte))

            return parsePayload(fType, payload)
        }

        private fun parsePayload(fType: DerpFrameType, payload: ByteArray): DerpFrame {
            val pBuf = ByteBuffer.wrap(payload).order(ByteOrder.BIG_ENDIAN)
            return when (fType) {
                DerpFrameType.SERVER_KEY -> {
                    require(payload.size >= 32) { "ServerKey payload too short: ${payload.size} < 32" }
                    val key = ByteArray(32)
                    pBuf.get(key)
                    val challenge = ByteArray(payload.size - 32)
                    pBuf.get(challenge)
                    ServerKey(key, challenge)
                }
                DerpFrameType.CLIENT_INFO -> {
                    require(payload.size >= 104) { "ClientInfo payload too short: ${payload.size} < 104" }
                    val clientKey = ByteArray(32)
                    pBuf.get(clientKey)
                    val timestamp = pBuf.long
                    val signature = ByteArray(64)
                    pBuf.get(signature)
                    ClientInfo(clientKey, timestamp, signature)
                }
                DerpFrameType.SERVER_INFO -> ServerInfo(payload)
                DerpFrameType.SEND_PACKET -> {
                    require(payload.size >= 32) { "SendPacket payload too short: ${payload.size} < 32" }
                    val dst = ByteArray(32)
                    pBuf.get(dst)
                    val data = ByteArray(payload.size - 32)
                    pBuf.get(data)
                    SendPacket(dst, data)
                }
                DerpFrameType.RECV_PACKET -> {
                    require(payload.size >= 32) { "RecvPacket payload too short: ${payload.size} < 32" }
                    val src = ByteArray(32)
                    pBuf.get(src)
                    val data = ByteArray(payload.size - 32)
                    pBuf.get(data)
                    RecvPacket(src, data)
                }
                DerpFrameType.KEEP_ALIVE -> KeepAlive
                DerpFrameType.PEER_GONE -> {
                    require(payload.size >= 32) { "PeerGone payload too short: ${payload.size} < 32" }
                    val key = ByteArray(32)
                    pBuf.get(key)
                    PeerGone(key)
                }
                DerpFrameType.PEER_PRESENT -> {
                    require(payload.size >= 32) { "PeerPresent payload too short: ${payload.size} < 32" }
                    val key = ByteArray(32)
                    pBuf.get(key)
                    PeerPresent(key)
                }
                DerpFrameType.PING -> {
                    require(payload.size >= 8) { "Ping payload too short: ${payload.size} < 8" }
                    val nonce = ByteArray(8)
                    pBuf.get(nonce)
                    Ping(nonce)
                }
                DerpFrameType.PONG -> {
                    require(payload.size >= 8) { "Pong payload too short: ${payload.size} < 8" }
                    val nonce = ByteArray(8)
                    pBuf.get(nonce)
                    Pong(nonce)
                }
            }
        }

        private fun readFully(input: InputStream, buffer: ByteArray) {
            var offset = 0
            while (offset < buffer.size) {
                val read = input.read(buffer, offset, buffer.size - offset)
                if (read == -1) throw EOFException("Unexpected EOF reading frame payload")
                offset += read
            }
        }
    }
}

/**
 * Multiplexed virtual stream protocol over DERP packets matching crates/archdrop_mesh/src/stream.rs.
 *
 * Header (10 bytes):
 * `[streamId: 8B BE][flags: 1B][reserved: 1B][data: NB]`
 */
object DerpStreamProtocol {
    const val FLAG_SYN: Byte = 0x01
    const val FLAG_DATA: Byte = 0x02
    const val FLAG_FIN: Byte = 0x04
    const val FLAG_RST: Byte = 0x08
    const val FLAG_ACK: Byte = 0x10

    const val STREAM_HEADER_LEN = 10
    const val DEFAULT_CHUNK_SIZE = 32 * 1024

    data class StreamPayload(
        val streamId: Long,
        val flags: Byte,
        val data: ByteArray
    ) {
        override fun equals(other: Any?): Boolean {
            if (this === other) return true
            if (javaClass != other?.javaClass) return false
            other as StreamPayload
            return streamId == other.streamId && flags == other.flags && data.contentEquals(other.data)
        }

        override fun hashCode(): Int {
            var result = streamId.hashCode()
            result = 31 * result + flags
            result = 31 * result + data.contentHashCode()
            return result
        }
    }

    fun encodeStreamPayload(streamId: Long, flags: Byte, data: ByteArray = ByteArray(0)): ByteArray {
        val buf = ByteBuffer.allocate(STREAM_HEADER_LEN + data.size).order(ByteOrder.BIG_ENDIAN)
        buf.putLong(streamId)
        buf.put(flags)
        buf.put(0x00.toByte()) // reserved
        buf.put(data)
        return buf.array()
    }

    fun decodeStreamPayload(payload: ByteArray): StreamPayload? {
        if (payload.size < STREAM_HEADER_LEN) return null
        val buf = ByteBuffer.wrap(payload).order(ByteOrder.BIG_ENDIAN)
        val streamId = buf.long
        val flags = buf.get()
        val _reserved = buf.get()
        val data = ByteArray(payload.size - STREAM_HEADER_LEN)
        buf.get(data)
        return StreamPayload(streamId, flags, data)
    }
}
