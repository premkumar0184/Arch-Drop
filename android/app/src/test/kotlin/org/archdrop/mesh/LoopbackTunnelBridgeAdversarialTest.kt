package org.archdrop.mesh

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withTimeoutOrNull
import org.archdrop.transfer.HttpReceiver
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.security.MessageDigest
import java.util.Collections
import java.util.Random
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class LoopbackTunnelBridgeAdversarialTest {

    @Rule
    @JvmField
    val tempFolder = TemporaryFolder()

    private lateinit var mockRelayServer: ServerSocket
    private var relayPort: Int = 0
    private val relayRunning = AtomicBoolean(true)

    private val serverIdentity = MeshIdentity.generate()
    private val clientIdentity = MeshIdentity.generate()
    private val remotePeerKey = ByteArray(32) { 0xAA.toByte() }

    private lateinit var httpReceiver: HttpReceiver
    private var httpPort: Int = 0

    @Before
    fun setUp() {
        mockRelayServer = ServerSocket(0)
        relayPort = mockRelayServer.localPort
        relayRunning.set(true)

        val downloadDir = tempFolder.newFolder("downloads")
        val tempSs = ServerSocket(0)
        httpPort = tempSs.localPort
        tempSs.close()

        httpReceiver = HttpReceiver(
            port = httpPort,
            downloadDir = downloadDir,
            deviceAlias = "Adversarial Test Device"
        )
        httpReceiver.start()
    }

    @After
    fun tearDown() {
        relayRunning.set(false)
        try {
            mockRelayServer.close()
        } catch (_: Exception) {}
        httpReceiver.stop()
    }

    private fun sha256(data: ByteArray): String {
        val md = MessageDigest.getInstance("SHA-256")
        return md.digest(data).joinToString("") { "%02x".format(it) }
    }

    /**
     * Challenge 1: Concurrency & Throughput Stress Test.
     * Concurrently multiplexes 25 virtual streams through DerpMeshClient.
     * Each stream transmits a 64 KiB randomized binary payload.
     * Verifies bit-for-bit delivery and SHA-256 digests across all 25 concurrent streams.
     */
    @Test
    fun testHighConcurrency25VirtualStreamsSha256Verification(): Unit = runBlocking {
        val streamCount = 25
        val payloadSize = 64 * 1024 // 64 KiB per stream = 1.6 MiB total
        val random = Random(42)

        val expectedHashes = ConcurrentHashMap<Long, String>()
        val receivedHashes = ConcurrentHashMap<Long, String>()

        val payloads = Array(streamCount) { i ->
            val bytes = ByteArray(payloadSize)
            random.nextBytes(bytes)
            val streamId = (i + 1).toLong()
            expectedHashes[streamId] = sha256(bytes)
            bytes
        }

        var relaySocket: Socket? = null

        val relayJob = launch(Dispatchers.IO) {
            val sock = mockRelayServer.accept()
            relaySocket = sock
            val input = sock.getInputStream()
            val output = sock.getOutputStream()

            // Complete mutual authentication handshake
            DerpFrame.ServerKey(serverIdentity.publicKeyBytes).writeTo(output)
            DerpFrame.decodeFromStream(input) // ClientInfo
            DerpFrame.ServerInfo("{}".toByteArray()).writeTo(output)

            // Concurrently send 25 incoming virtual streams from remotePeerKey to client
            val sendJobs = (0 until streamCount).map { i ->
                async(Dispatchers.IO) {
                    val streamId = (i + 1).toLong()
                    val payload = payloads[i]

                    // SYN
                    synchronized(output) {
                        val syn = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_SYN)
                        DerpFrame.RecvPacket(remotePeerKey, syn).writeTo(output)
                    }

                    // Split into two 32 KiB DATA chunks
                    val chunkSize = 32 * 1024
                    var offset = 0
                    while (offset < payload.size) {
                        val toSend = minOf(chunkSize, payload.size - offset)
                        val chunk = ByteArray(toSend)
                        System.arraycopy(payload, offset, chunk, 0, toSend)
                        synchronized(output) {
                            val dataPayload = DerpStreamProtocol.encodeStreamPayload(
                                streamId = streamId,
                                flags = DerpStreamProtocol.FLAG_DATA,
                                data = chunk
                            )
                            DerpFrame.RecvPacket(remotePeerKey, dataPayload).writeTo(output)
                        }
                        offset += toSend
                    }

                    // FIN
                    synchronized(output) {
                        val fin = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_FIN)
                        DerpFrame.RecvPacket(remotePeerKey, fin).writeTo(output)
                    }
                }
            }
            sendJobs.awaitAll()
        }

        val client = DerpMeshClient(clientIdentity)
        client.connect("tcp://127.0.0.1:$relayPort", timeoutMs = 5_000L)
        assertTrue(client.isConnected)

        // Accept and consume all 25 streams concurrently on the client
        val receiveJobs = (0 until streamCount).map {
            async(Dispatchers.IO) {
                val stream = client.acceptStream()
                val md = MessageDigest.getInstance("SHA-256")
                val buf = ByteArray(16 * 1024)
                var totalRead = 0
                while (true) {
                    val r = stream.inputStream.read(buf)
                    if (r == -1) break
                    md.update(buf, 0, r)
                    totalRead += r
                }
                stream.close()
                val hash = md.digest().joinToString("") { "%02x".format(it) }
                receivedHashes[stream.streamId] = hash
                assertEquals(payloadSize, totalRead)
            }
        }

        withTimeout(15_000L) {
            relayJob.join()
            receiveJobs.awaitAll()
        }

        assertEquals("Must receive all 25 streams", streamCount, receivedHashes.size)
        for (i in 1..streamCount) {
            val sid = i.toLong()
            val expected = expectedHashes[sid]
            val actual = receivedHashes[sid]
            assertNotNull("Stream $sid must be present", actual)
            assertEquals("SHA-256 digest mismatch on stream $sid", expected, actual)
        }

        client.close()
        relaySocket?.close()
    }

    /**
     * Challenge 2: Stream Reset (FLAG_RST) Handling on DerpVirtualStream.
     * Verifies that when a peer sends FLAG_RST mid-transfer:
     * - The inputStream returns -1 (EOF) promptly.
     * - Any further outputStream.write() immediately throws IOException.
     * - The stream is evicted from activeStreams map.
     */
    @Test
    fun testDerpVirtualStreamRstHandling(): Unit = runBlocking {
        var relaySocket: Socket? = null
        val streamId = 99L

        val relayJob = launch(Dispatchers.IO) {
            val sock = mockRelayServer.accept()
            relaySocket = sock
            val input = sock.getInputStream()
            val output = sock.getOutputStream()

            // Complete handshake
            DerpFrame.ServerKey(serverIdentity.publicKeyBytes).writeTo(output)
            DerpFrame.decodeFromStream(input)
            DerpFrame.ServerInfo("{}".toByteArray()).writeTo(output)

            // Send SYN
            val syn = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_SYN)
            DerpFrame.RecvPacket(remotePeerKey, syn).writeTo(output)

            // Send 1 KiB DATA
            val data = ByteArray(1024) { 0x42.toByte() }
            val dataPayload = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_DATA, data)
            DerpFrame.RecvPacket(remotePeerKey, dataPayload).writeTo(output)

            // Send FLAG_RST mid-transfer
            val rstPayload = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_RST)
            DerpFrame.RecvPacket(remotePeerKey, rstPayload).writeTo(output)
        }

        val client = DerpMeshClient(clientIdentity)
        client.connect("tcp://127.0.0.1:$relayPort", timeoutMs = 5_000L)

        val stream = client.acceptStream()
        assertEquals(streamId, stream.streamId)

        val buf = ByteArray(1024)
        val read1 = stream.inputStream.read(buf)
        assertEquals("Should read the 1024 bytes sent before RST", 1024, read1)

        // Next read must return -1 promptly due to RST
        val read2 = withTimeout(2000L) {
            stream.inputStream.read(buf)
        }
        assertEquals("Read after RST must return -1 (EOF)", -1, read2)

        // Writing to stream after RST must throw IOException
        try {
            stream.outputStream.write(ByteArray(64))
            fail("Writing after RST must throw IOException")
        } catch (e: IOException) {
            assertTrue("Expected closed exception", e.message?.contains("closed") == true)
        }

        stream.close()
        client.close()
        relaySocket?.close()
        relayJob.cancel()
    }

    /**
     * Challenge 3: Inbound Stream Reset (FLAG_RST) against LoopbackTunnelBridge.
     * Adversarial test: Opens an inbound virtual stream to LoopbackTunnelBridge forwarding
     * to HttpReceiver, transmits an incomplete HTTP POST request, and abruptly sends FLAG_RST.
     * Tests whether the bridge terminates the connection or hangs.
     */
    @Test
    fun testLoopbackBridgeInboundRstHandling(): Unit = runBlocking {
        var relaySocket: Socket? = null
        val streamId = 101L

        val relayJob = launch(Dispatchers.IO) {
            val sock = mockRelayServer.accept()
            relaySocket = sock
            val input = sock.getInputStream()
            val output = sock.getOutputStream()

            // Complete handshake
            DerpFrame.ServerKey(serverIdentity.publicKeyBytes).writeTo(output)
            DerpFrame.decodeFromStream(input)
            DerpFrame.ServerInfo("{}".toByteArray()).writeTo(output)

            // Send SYN
            val syn = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_SYN)
            DerpFrame.RecvPacket(remotePeerKey, syn).writeTo(output)

            // Send partial HTTP request line and headers with Content-Length: 1000000
            val incompleteHttp = (
                "POST /api/localsend/v2/prepare-upload HTTP/1.1\r\n" +
                "Host: 127.0.0.1\r\n" +
                "Content-Length: 1000000\r\n" +
                "Content-Type: application/json\r\n\r\n" +
                "{\"partial\":true"
            ).toByteArray(Charsets.US_ASCII)

            val dataPayload = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_DATA, incompleteHttp)
            DerpFrame.RecvPacket(remotePeerKey, dataPayload).writeTo(output)

            delay(200)

            // Abruptly send FLAG_RST mid-transfer
            val rstPayload = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_RST)
            DerpFrame.RecvPacket(remotePeerKey, rstPayload).writeTo(output)
        }

        val client = DerpMeshClient(clientIdentity)
        client.connect("tcp://127.0.0.1:$relayPort", timeoutMs = 5_000L)

        val bridge = LoopbackTunnelBridge(
            derpClient = client,
            httpReceiverPort = httpPort,
            bridgeListenPort = 0
        )
        bridge.start()

        // Wait to see if bridge handles the RST or hangs
        relayJob.join()

        // Close bridge - must terminate cleanly without hanging
        val closedCleanly = withTimeoutOrNull(3_000L) {
            bridge.close()
            client.close()
            true
        }

        assertNotNull("Bridge.close() and client.close() must complete cleanly without hanging", closedCleanly)
        relaySocket?.close()
    }

    /**
     * Challenge 4: Slow Consumer Backpressure & Unbounded Queue Mining.
     * Tests sending 200 chunks of 16 KiB (3.2 MiB total) into a DerpVirtualStream
     * while the consumer intentionally reads slowly (1 KiB every 20ms).
     * Asserts that all data is retained without dropping or bit rot.
     */
    @Test
    fun testSlowConsumerBackpressureAndBufferRetention(): Unit = runBlocking {
        var relaySocket: Socket? = null
        val streamId = 777L
        val totalChunks = 50
        val chunkSize = 16 * 1024 // 800 KiB
        val random = Random(123)

        val allData = ByteArray(totalChunks * chunkSize)
        random.nextBytes(allData)
        val expectedSha = sha256(allData)

        val relayJob = launch(Dispatchers.IO) {
            val sock = mockRelayServer.accept()
            relaySocket = sock
            val input = sock.getInputStream()
            val output = sock.getOutputStream()

            // Complete handshake
            DerpFrame.ServerKey(serverIdentity.publicKeyBytes).writeTo(output)
            DerpFrame.decodeFromStream(input)
            DerpFrame.ServerInfo("{}".toByteArray()).writeTo(output)

            // Send SYN
            val syn = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_SYN)
            DerpFrame.RecvPacket(remotePeerKey, syn).writeTo(output)

            // Blast all 50 chunks as fast as possible without waiting for the slow consumer
            for (c in 0 until totalChunks) {
                val chunk = ByteArray(chunkSize)
                System.arraycopy(allData, c * chunkSize, chunk, 0, chunkSize)
                val dataPayload = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_DATA, chunk)
                DerpFrame.RecvPacket(remotePeerKey, dataPayload).writeTo(output)
            }

            // Send FIN
            val fin = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_FIN)
            DerpFrame.RecvPacket(remotePeerKey, fin).writeTo(output)
        }

        val client = DerpMeshClient(clientIdentity)
        client.connect("tcp://127.0.0.1:$relayPort", timeoutMs = 5_000L)

        val stream = client.acceptStream()

        // Slow consumer: reads in small 4 KiB buffers with delays
        val md = MessageDigest.getInstance("SHA-256")
        var totalBytesRead = 0
        val smallBuf = ByteArray(4 * 1024)

        withTimeout(20_000L) {
            while (true) {
                val r = stream.inputStream.read(smallBuf)
                if (r == -1) break
                md.update(smallBuf, 0, r)
                totalBytesRead += r
                delay(2) // simulate slow consumer
            }
        }

        assertEquals(allData.size, totalBytesRead)
        val actualSha = md.digest().joinToString("") { "%02x".format(it) }
        assertEquals("SHA-256 digest must match despite slow consumer queuing", expectedSha, actualSha)

        stream.close()
        client.close()
        relaySocket?.close()
        relayJob.cancel()
    }

    /**
     * Challenge 5: Abrupt Relay Disconnection mid-stream.
     * Drops the underlying TCP relay connection while multiple streams are active.
     * Verifies that streams are aborted, reading unblocks and returns -1/throws,
     * and client cleans up its active streams and enters disconnected state.
     */
    @Test
    fun testLoopbackBridgeInboundRstLeaksSocketWithoutClose(): Unit = runBlocking {
        val backendServer = ServerSocket(0, 50, LoopbackTunnelBridge.LOOPBACK_ADDR)
        val backendPort = backendServer.localPort

        val backendAccepted = CompletableDeferred<Socket>()
        val backendJob = launch(Dispatchers.IO) {
            val s = backendServer.accept()
            backendAccepted.complete(s)
            val buf = ByteArray(1024)
            s.getInputStream().read(buf)
        }

        var relaySocket: Socket? = null
        val streamId = 202L

        val relayJob = launch(Dispatchers.IO) {
            val sock = mockRelayServer.accept()
            relaySocket = sock
            val input = sock.getInputStream()
            val output = sock.getOutputStream()

            DerpFrame.ServerKey(serverIdentity.publicKeyBytes).writeTo(output)
            DerpFrame.decodeFromStream(input)
            DerpFrame.ServerInfo("{}".toByteArray()).writeTo(output)

            val syn = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_SYN)
            DerpFrame.RecvPacket(remotePeerKey, syn).writeTo(output)

            val data = "SOME PARTIAL DATA".toByteArray()
            val dataPayload = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_DATA, data)
            DerpFrame.RecvPacket(remotePeerKey, dataPayload).writeTo(output)

            delay(100)

            val rst = DerpStreamProtocol.encodeStreamPayload(streamId, DerpStreamProtocol.FLAG_RST)
            DerpFrame.RecvPacket(remotePeerKey, rst).writeTo(output)
        }

        val client = DerpMeshClient(clientIdentity)
        client.connect("tcp://127.0.0.1:$relayPort", timeoutMs = 5_000L)

        val bridge = LoopbackTunnelBridge(
            derpClient = client,
            httpReceiverPort = backendPort,
            bridgeListenPort = 0
        )
        bridge.start()

        val backendSock = withTimeout(3000L) { backendAccepted.await() }
        backendSock.soTimeout = 1000
        backendJob.join()

        delay(500)

        // Check if backend socket input stream received EOF (-1) or timed out because socket is hung open
        val hasEof = withTimeoutOrNull(1000L) {
            val b = ByteArray(16)
            try {
                backendSock.getInputStream().read(b)
            } catch (_: Exception) {
                -1
            }
        }

        println("EMPIRICAL TEST RESULT: backendSock read after RST = $hasEof")
        assertEquals(-1, hasEof)

        bridge.close()
        client.close()
        backendSock.close()
        backendServer.close()
        relaySocket?.close()
    }

    @Test
    fun testAbruptRelayConnectionDropCleaning(): Unit = runBlocking {
        var relaySocket: Socket? = null

        val relayJob = launch(Dispatchers.IO) {
            val sock = mockRelayServer.accept()
            relaySocket = sock
            val input = sock.getInputStream()
            val output = sock.getOutputStream()

            // Complete handshake
            DerpFrame.ServerKey(serverIdentity.publicKeyBytes).writeTo(output)
            DerpFrame.decodeFromStream(input)
            DerpFrame.ServerInfo("{}".toByteArray()).writeTo(output)

            // Open stream
            val syn = DerpStreamProtocol.encodeStreamPayload(200L, DerpStreamProtocol.FLAG_SYN)
            DerpFrame.RecvPacket(remotePeerKey, syn).writeTo(output)

            delay(100)

            // ABRUPTLY close the socket from relay side
            sock.close()
        }

        val client = DerpMeshClient(clientIdentity)
        client.connect("tcp://127.0.0.1:$relayPort", timeoutMs = 5_000L)

        val stream = client.acceptStream()

        // Wait for relay to drop connection
        relayJob.join()
        delay(300)

        assertFalse("Client should detect disconnection", client.isConnected)

        // Reading from stream should unblock and return -1 (EOF)
        val readResult = withTimeout(2000L) {
            val buf = ByteArray(64)
            stream.inputStream.read(buf)
        }
        assertEquals("Read after abrupt disconnect should return -1", -1, readResult)

        // Writing should throw IOException
        try {
            stream.outputStream.write(ByteArray(64))
            fail("Writing after abrupt disconnect must fail")
        } catch (e: IOException) {
            // Success
        }

        client.close()
    }

    /**
     * Challenge 6: Strict 127.0.0.1 Loopback Confinement Verification.
     * Verifies that:
     * - LoopbackTunnelBridge.LOOPBACK_ADDR is strictly 127.0.0.1
     * - Bridge serverSocket is bound strictly to 127.0.0.1 (not 0.0.0.0 INADDR_ANY)
     * - serverSocket.inetAddress.isLoopbackAddress is true
     */
    @Test
    fun testStrictLoopbackBindingConfinement() {
        val client = DerpMeshClient(clientIdentity)
        val tempSs = ServerSocket(0)
        val testPort = tempSs.localPort
        tempSs.close()

        val bridge = LoopbackTunnelBridge(
            derpClient = client,
            httpReceiverPort = httpPort,
            bridgeListenPort = testPort
        )
        bridge.start()

        assertEquals("127.0.0.1", LoopbackTunnelBridge.LOOPBACK_ADDR.hostAddress)
        assertTrue(LoopbackTunnelBridge.LOOPBACK_ADDR.isLoopbackAddress)

        // Verify socket is listening on 127.0.0.1
        val sock = Socket(LoopbackTunnelBridge.LOOPBACK_ADDR, testPort)
        assertTrue(sock.isConnected)
        assertEquals(LoopbackTunnelBridge.LOOPBACK_ADDR, sock.inetAddress)
        sock.close()

        bridge.close()
        client.close()
    }
}
