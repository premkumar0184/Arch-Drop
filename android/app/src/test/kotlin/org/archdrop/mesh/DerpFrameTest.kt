package org.archdrop.mesh

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder

class DerpFrameTest {

    @Test
    fun testServerKeyFrameEncodingAndDecoding() {
        val serverKey = ByteArray(32) { (it + 1).toByte() }
        val challenge = "random-challenge-salt-xyz".toByteArray(Charsets.UTF_8)
        val frame = DerpFrame.ServerKey(serverKey, challenge)

        val encoded = frame.encode()
        assertEquals(0x01.toByte(), encoded[0]) // ServerKey type
        val payloadLen = ByteBuffer.wrap(encoded, 1, 4).order(ByteOrder.BIG_ENDIAN).int
        assertEquals(32 + challenge.size, payloadLen)

        val decoded = DerpFrame.decode(encoded)
        assertNotNull(decoded)
        assertTrue(decoded is DerpFrame.ServerKey)
        val decodedServerKey = decoded as DerpFrame.ServerKey
        assertArrayEquals(serverKey, decodedServerKey.serverKey)
        assertArrayEquals(challenge, decodedServerKey.challenge)
    }

    @Test
    fun testClientInfoFrameEncodingAndDecoding() {
        val clientKey = ByteArray(32) { 0xAA.toByte() }
        val timestamp = 1731062400L
        val signature = ByteArray(64) { (it * 2).toByte() }
        val frame = DerpFrame.ClientInfo(clientKey, timestamp, signature)

        val encoded = frame.encode()
        assertEquals(0x02.toByte(), encoded[0])
        val payloadLen = ByteBuffer.wrap(encoded, 1, 4).order(ByteOrder.BIG_ENDIAN).int
        assertEquals(104, payloadLen)

        val decoded = DerpFrame.decode(encoded)
        assertNotNull(decoded)
        assertTrue(decoded is DerpFrame.ClientInfo)
        val decodedInfo = decoded as DerpFrame.ClientInfo
        assertArrayEquals(clientKey, decodedInfo.clientKey)
        assertEquals(timestamp, decodedInfo.timestamp)
        assertArrayEquals(signature, decodedInfo.signature)
    }

    @Test
    fun testServerInfoFrameEncodingAndDecoding() {
        val infoJson = "{\"version\":\"1.0\",\"status\":\"OK\"}".toByteArray(Charsets.UTF_8)
        val frame = DerpFrame.ServerInfo(infoJson)

        val encoded = frame.encode()
        assertEquals(0x03.toByte(), encoded[0])

        val decoded = DerpFrame.decode(encoded)
        assertNotNull(decoded)
        assertTrue(decoded is DerpFrame.ServerInfo)
        assertArrayEquals(infoJson, (decoded as DerpFrame.ServerInfo).data)
    }

    @Test
    fun testSendAndRecvPacketFrames() {
        val peerKey = ByteArray(32) { 0x42.toByte() }
        val payload = "payload data inside DERP packet".toByteArray(Charsets.UTF_8)

        val sendFrame = DerpFrame.SendPacket(peerKey, payload)
        val sendEncoded = sendFrame.encode()
        assertEquals(0x04.toByte(), sendEncoded[0])

        val sendDecoded = DerpFrame.decode(sendEncoded)
        assertTrue(sendDecoded is DerpFrame.SendPacket)
        assertArrayEquals(peerKey, (sendDecoded as DerpFrame.SendPacket).dstKey)
        assertArrayEquals(payload, sendDecoded.payload)

        val recvFrame = DerpFrame.RecvPacket(peerKey, payload)
        val recvEncoded = recvFrame.encode()
        assertEquals(0x05.toByte(), recvEncoded[0])

        val recvDecoded = DerpFrame.decode(recvEncoded)
        assertTrue(recvDecoded is DerpFrame.RecvPacket)
        assertArrayEquals(peerKey, (recvDecoded as DerpFrame.RecvPacket).srcKey)
        assertArrayEquals(payload, recvDecoded.payload)
    }

    @Test
    fun testKeepAliveFrame() {
        val frame = DerpFrame.KeepAlive
        val encoded = frame.encode()
        assertEquals(5, encoded.size)
        assertEquals(0x06.toByte(), encoded[0])
        assertEquals(0, ByteBuffer.wrap(encoded, 1, 4).order(ByteOrder.BIG_ENDIAN).int)

        val decoded = DerpFrame.decode(encoded)
        assertEquals(DerpFrame.KeepAlive, decoded)
    }

    @Test
    fun testPeerPresenceAndProbes() {
        val peerKey = ByteArray(32) { 0x77.toByte() }
        val nonce = byteArrayOf(1, 2, 3, 4, 5, 6, 7, 8)

        val gone = DerpFrame.PeerGone(peerKey)
        val goneDecoded = DerpFrame.decode(gone.encode())
        assertTrue(goneDecoded is DerpFrame.PeerGone)
        assertArrayEquals(peerKey, (goneDecoded as DerpFrame.PeerGone).peerKey)

        val present = DerpFrame.PeerPresent(peerKey)
        val presentDecoded = DerpFrame.decode(present.encode())
        assertTrue(presentDecoded is DerpFrame.PeerPresent)
        assertArrayEquals(peerKey, (presentDecoded as DerpFrame.PeerPresent).peerKey)

        val ping = DerpFrame.Ping(nonce)
        val pingDecoded = DerpFrame.decode(ping.encode())
        assertTrue(pingDecoded is DerpFrame.Ping)
        assertArrayEquals(nonce, (pingDecoded as DerpFrame.Ping).nonce)

        val pong = DerpFrame.Pong(nonce)
        val pongDecoded = DerpFrame.decode(pong.encode())
        assertTrue(pongDecoded is DerpFrame.Pong)
        assertArrayEquals(nonce, (pongDecoded as DerpFrame.Pong).nonce)
    }

    @Test
    fun testStreamPayloadCodec() {
        val streamId = 9876543210L
        val data = "Streaming chunk for virtual connection".toByteArray(Charsets.UTF_8)
        val encodedPayload = DerpStreamProtocol.encodeStreamPayload(
            streamId = streamId,
            flags = DerpStreamProtocol.FLAG_DATA,
            data = data
        )

        assertEquals(DerpStreamProtocol.STREAM_HEADER_LEN + data.size, encodedPayload.size)

        val decoded = DerpStreamProtocol.decodeStreamPayload(encodedPayload)
        assertNotNull(decoded)
        assertEquals(streamId, decoded!!.streamId)
        assertEquals(DerpStreamProtocol.FLAG_DATA, decoded.flags)
        assertArrayEquals(data, decoded.data)

        // SYN control frame with empty data
        val synPayload = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_SYN)
        val decodedSyn = DerpStreamProtocol.decodeStreamPayload(synPayload)
        assertNotNull(decodedSyn)
        assertEquals(DerpStreamProtocol.FLAG_SYN, decodedSyn!!.flags)
        assertEquals(0, decodedSyn.data.size)
    }

    @Test
    fun testDecodeFromStreamContinuous() {
        val baos = ByteArrayOutputStream()
        val frame1 = DerpFrame.KeepAlive
        val frame2 = DerpFrame.Ping(byteArrayOf(10, 20, 30, 40, 50, 60, 70, 80))
        val frame3 = DerpFrame.Pong(byteArrayOf(10, 20, 30, 40, 50, 60, 70, 80))

        frame1.writeTo(baos)
        frame2.writeTo(baos)
        frame3.writeTo(baos)

        val bais = ByteArrayInputStream(baos.toByteArray())
        assertEquals(DerpFrame.KeepAlive, DerpFrame.decodeFromStream(bais))
        assertEquals(frame2, DerpFrame.decodeFromStream(bais))
        assertEquals(frame3, DerpFrame.decodeFromStream(bais))
        assertNull(DerpFrame.decodeFromStream(bais)) // Clean EOF
    }

    @Test
    fun testInvalidOrTruncatedFrames() {
        // Less than 5 bytes
        assertNull(DerpFrame.decode(byteArrayOf(1, 2, 3)))

        // Truncated payload
        val partial = byteArrayOf(0x01, 0x00, 0x00, 0x00, 0x20, 0x05, 0x06)
        assertNull(DerpFrame.decode(partial))
    }
}
