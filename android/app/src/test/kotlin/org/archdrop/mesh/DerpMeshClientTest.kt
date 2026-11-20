package org.archdrop.mesh

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.io.InputStream
import java.io.OutputStream
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.atomic.AtomicBoolean

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class DerpMeshClientTest {

    private lateinit var mockRelayServer: ServerSocket
    private var relayPort: Int = 0
    private val relayRunning = AtomicBoolean(true)

    private val serverIdentity = MeshIdentity.generate()
    private val clientIdentity = MeshIdentity.generate()

    @Before
    fun setUp() {
        mockRelayServer = ServerSocket(0)
        mockRelayServer.soTimeout = 5000
        relayPort = mockRelayServer.localPort
        relayRunning.set(true)
    }

    @After
    fun tearDown() {
        relayRunning.set(false)
        try {
            mockRelayServer.close()
        } catch (_: Exception) {}
    }

    @Test
    fun testMutualEd25519HandshakeAndVirtualStreamDataTransfer() = runBlocking {
        val serverReceivedClientKey = AtomicBoolean(false)
        val streamDataReceived = ByteArray(1024)
        var streamDataLen = 0

        val remotePeerKey = ByteArray(32) { 0x55.toByte() }

        // Start mock relay in background
        val relayJob = launch(Dispatchers.IO) {
            val sock = mockRelayServer.accept()
            val input = sock.getInputStream()
            val output = sock.getOutputStream()

            // 1. Send ServerKey frame to connecting client
            val challenge = "CHALLENGE_SALT".toByteArray(Charsets.UTF_8)
            val serverKeyFrame = DerpFrame.ServerKey(serverIdentity.publicKeyBytes, challenge)
            serverKeyFrame.writeTo(output)

            // 2. Read ClientInfo frame from client
            val clientInfoFrame = DerpFrame.decodeFromStream(input) as? DerpFrame.ClientInfo
            assertNotNull(clientInfoFrame)
            assertArrayEquals(clientIdentity.publicKeyBytes, clientInfoFrame!!.clientKey)

            // Verify client challenge signature
            val valid = MeshCrypto.verifyChallenge(
                clientKey = clientInfoFrame.clientKey,
                serverKey = serverIdentity.publicKeyBytes,
                timestamp = clientInfoFrame.timestamp,
                signature = clientInfoFrame.signature
            )
            assertTrue("Client signature must be verified", valid)
            serverReceivedClientKey.set(true)

            // 3. Send ServerInfo frame confirming acceptance
            val serverInfo = DerpFrame.ServerInfo("{\"status\":\"AUTHENTICATED\"}".toByteArray(Charsets.UTF_8))
            serverInfo.writeTo(output)

            // 4. Relay loop: client sends SendPacket with SYN, relay reflects back as RecvPacket to simulate peer
            while (relayRunning.get() && !sock.isClosed) {
                val frame = try {
                    DerpFrame.decodeFromStream(input)
                } catch (e: Exception) {
                    break
                } ?: break

                when (frame) {
                    is DerpFrame.SendPacket -> {
                        val payload = DerpStreamProtocol.decodeStreamPayload(frame.payload)
                        if (payload != null) {
                            if (payload.flags.toInt() and DerpStreamProtocol.FLAG_SYN.toInt() != 0) {
                                // Peer responds with DATA payload
                                val testData = "Hello from remote peer over DERP virtual stream!".toByteArray(Charsets.UTF_8)
                                val responsePayload = DerpStreamProtocol.encodeStreamPayload(
                                    streamId = payload.streamId,
                                    flags = DerpStreamProtocol.FLAG_DATA,
                                    data = testData
                                )
                                val recvPacket = DerpFrame.RecvPacket(
                                    srcKey = remotePeerKey,
                                    payload = responsePayload
                                )
                                recvPacket.writeTo(output)

                                // And then sends FIN
                                val finPayload = DerpStreamProtocol.encodeStreamPayload(
                                    streamId = payload.streamId,
                                    flags = DerpStreamProtocol.FLAG_FIN
                                )
                                DerpFrame.RecvPacket(remotePeerKey, finPayload).writeTo(output)
                            }
                        }
                    }
                    is DerpFrame.Ping -> {
                        DerpFrame.Pong(frame.nonce).writeTo(output)
                    }
                    else -> {}
                }
            }
        }

        // Initialize and connect DerpMeshClient
        val client = DerpMeshClient(clientIdentity)
        client.connect("tcp://127.0.0.1:$relayPort", timeoutMs = 5_000L)

        assertTrue(client.isConnected)
        assertTrue(serverReceivedClientKey.get())
        assertArrayEquals(serverIdentity.publicKeyBytes, client.serverKey)

        // Open a virtual stream to remote peer
        val stream = client.openStream(remotePeerKey)
        val readBuf = ByteArray(256)
        val bytesRead = stream.inputStream.read(readBuf)
        assertTrue(bytesRead > 0)
        val receivedText = String(readBuf, 0, bytesRead, Charsets.UTF_8)
        assertEquals("Hello from remote peer over DERP virtual stream!", receivedText)

        // Reading after FIN returns -1 (EOF)
        val eofRead = stream.inputStream.read(readBuf)
        assertEquals(-1, eofRead)

        stream.close()
        client.close()
        assertFalse(client.isConnected)

        relayJob.cancel()
    }

    @Test
    fun testKeepaliveAndPingPongHandling() = runBlocking {
        val receivedPingNonce = ByteArray(8)
        val pingNonce = byteArrayOf(11, 22, 33, 44, 55, 66, 77, 88)

        val relayJob = launch(Dispatchers.IO) {
            val sock = mockRelayServer.accept()
            val input = sock.getInputStream()
            val output = sock.getOutputStream()

            // Complete handshake
            DerpFrame.ServerKey(serverIdentity.publicKeyBytes).writeTo(output)
            DerpFrame.decodeFromStream(input) // ClientInfo
            DerpFrame.ServerInfo("{}".toByteArray()).writeTo(output)

            // Send Ping probe to client
            DerpFrame.Ping(pingNonce).writeTo(output)

            // Read Pong response from client
            val pongFrame = DerpFrame.decodeFromStream(input) as? DerpFrame.Pong
            assertNotNull(pongFrame)
            System.arraycopy(pongFrame!!.nonce, 0, receivedPingNonce, 0, 8)
        }

        val client = DerpMeshClient(clientIdentity, keepaliveIntervalMs = 500L)
        client.connect("tcp://127.0.0.1:$relayPort", timeoutMs = 5_000L)

        withTimeout(3_000L) {
            while (receivedPingNonce.all { it == 0.toByte() }) {
                kotlinx.coroutines.delay(50)
            }
        }

        assertArrayEquals(pingNonce, receivedPingNonce)
        client.close()
        relayJob.cancel()
    }
}
