package org.archdrop.mesh

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.archdrop.transfer.HttpReceiver
import org.archdrop.transfer.models.FileDto
import org.archdrop.transfer.models.PrepareUploadRequestDto
import org.archdrop.transfer.models.PrepareUploadResponseDto
import org.archdrop.transfer.models.RegisterDto
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import java.io.InputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class LoopbackTunnelBridgeTest {

    @Rule
    @JvmField
    val tempFolder = TemporaryFolder()

    private lateinit var mockRelayServer: ServerSocket
    private var relayPort: Int = 0
    private val relayRunning = AtomicBoolean(true)

    private val serverIdentity = MeshIdentity.generate()
    private val clientIdentity = MeshIdentity.generate()
    private val desktopPeerKey = ByteArray(32) { 0xDE.toByte() }

    private lateinit var httpReceiver: HttpReceiver
    private var httpPort: Int = 0

    private val json = Json { ignoreUnknownKeys = true }

    @Before
    fun setUp() {
        mockRelayServer = ServerSocket(0)
        mockRelayServer.soTimeout = 5000
        relayPort = mockRelayServer.localPort
        relayRunning.set(true)

        val downloadDir = tempFolder.newFolder("downloads")
        // Use dynamically allocated port for test isolation
        val tempSs = ServerSocket(0)
        httpPort = tempSs.localPort
        tempSs.close()

        httpReceiver = HttpReceiver(
            port = httpPort,
            downloadDir = downloadDir,
            deviceAlias = "Test Android Device"
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

    @Test
    fun testLoopbackBridgeStrictBinding() {
        val client = DerpMeshClient(clientIdentity)
        val bridge = LoopbackTunnelBridge(
            derpClient = client,
            httpReceiverPort = httpPort,
            bridgeListenPort = 0 // dynamically allocate
        )
        bridge.start()

        assertEquals(InetAddress.getByName("127.0.0.1"), LoopbackTunnelBridge.LOOPBACK_ADDR)
        assertTrue(bridge.active)

        bridge.close()
        assertFalse(bridge.active)
        client.close()
    }

    @Test
    fun testInboundDerpVirtualStreamToHttpReceiverBridging() {
        runBlocking {
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

            // When ready, relay initiates incoming stream from desktopPeerKey to Android client
            // streamId = 42, flag = SYN
            val synPayload = DerpStreamProtocol.encodeStreamPayload(42L, DerpStreamProtocol.FLAG_SYN)
            DerpFrame.RecvPacket(desktopPeerKey, synPayload).writeTo(output)

            // Desktop sends HTTP GET /api/localsend/v2/info
            val httpRequest = "GET /api/localsend/v2/info HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n".toByteArray(Charsets.US_ASCII)
            val dataPayload = DerpStreamProtocol.encodeStreamPayload(42L, DerpStreamProtocol.FLAG_DATA, httpRequest)
            DerpFrame.RecvPacket(desktopPeerKey, dataPayload).writeTo(output)

            val finPayload = DerpStreamProtocol.encodeStreamPayload(42L, DerpStreamProtocol.FLAG_FIN)
            DerpFrame.RecvPacket(desktopPeerKey, finPayload).writeTo(output)

            // Read forwarded HTTP response packets from Android client over DERP
            var responseText = ""
            while (relayRunning.get() && !sock.isClosed) {
                val frame = try {
                    DerpFrame.decodeFromStream(input)
                } catch (_: Exception) {
                    break
                } ?: break

                if (frame is DerpFrame.SendPacket) {
                    val streamPayload = DerpStreamProtocol.decodeStreamPayload(frame.payload)
                    if (streamPayload != null) {
                        if (streamPayload.data.isNotEmpty()) {
                            responseText += String(streamPayload.data, Charsets.UTF_8)
                        }
                        if (streamPayload.flags.toInt() and DerpStreamProtocol.FLAG_FIN.toInt() != 0) {
                            break
                        }
                    }
                }
            }

            assertTrue("Response must contain HTTP 200 OK", responseText.contains("HTTP/1.1 200 OK"))
            assertTrue("Response must contain deviceAlias", responseText.contains("Test Android Device"))
        }

        val client = DerpMeshClient(clientIdentity)
        client.connect("tcp://127.0.0.1:$relayPort", timeoutMs = 5_000L)

        val bridge = LoopbackTunnelBridge(
            derpClient = client,
            httpReceiverPort = httpPort,
            bridgeListenPort = 0
        )
        bridge.start()

        // Wait for relay job to verify HTTP response
        relayJob.join()

        bridge.close()
        client.close()
        relaySocket?.close()
        }
    }
}
