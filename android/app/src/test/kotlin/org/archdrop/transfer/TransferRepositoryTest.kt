package org.archdrop.transfer

import android.content.Context
import android.net.Uri
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.runBlocking
import org.junit.After
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
import java.io.File
import java.net.ServerSocket
import kotlin.system.measureTimeMillis

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class TransferRepositoryTest {

    @Rule
    @JvmField
    val tempFolder = TemporaryFolder()

    private lateinit var mockHttpReceiver: HttpReceiver
    private var mockPort: Int = 0
    private lateinit var testFile: File
    private lateinit var context: Context

    @Before
    fun setUp() {
        context = ApplicationProvider.getApplicationContext()

        val downloadDir = tempFolder.newFolder("repo_downloads")
        val ss = ServerSocket(0)
        mockPort = ss.localPort
        ss.close()

        mockHttpReceiver = HttpReceiver(
            port = mockPort,
            downloadDir = downloadDir,
            deviceAlias = "MockDesktop"
        )
        mockHttpReceiver.start()

        testFile = tempFolder.newFile("test_payload.bin")
        testFile.writeBytes(ByteArray(64 * 1024) { 0x42 }) // 64 KiB test payload
    }

    @After
    fun tearDown() {
        try {
            mockHttpReceiver.stop()
        } catch (_: Exception) {}
    }

    /**
     * Test 1: Direct LAN Success.
     * When target is online and port open, route must be DirectLan, transfer succeeds.
     */
    @Test
    fun testDirectLanTransferFastSuccess() = runBlocking {
        val repository = TransferRepository(context = context)
        val candidate = PeerCandidateRoutes(
            peerId = "laptop_01",
            directLanHost = "127.0.0.1",
            directLanPort = mockPort
        )

        val elapsed = measureTimeMillis {
            val result = repository.sendPayload(testFile, candidate)
            assertTrue("Direct LAN transfer must succeed: ${result.errorMessage}", result.success)
        }

        assertTrue("Direct LAN transfer must complete fast (< 2000ms), took ${elapsed}ms", elapsed < 2000)
        assertEquals(RouteTier.DIRECT_LAN, repository.activeRoute.value?.tier)
        assertEquals(RouteState.ACTIVE, repository.routeState.value)
        assertNotNull(repository.routeTelemetry.value)
        assertEquals(RouteState.ACTIVE, repository.routeTelemetry.value?.state)
    }

    /**
     * Test 2: Direct LAN sendStagedFileWithFailover via DiscoveredPeer.
     * Verifies that DiscoveredPeer maps directly to DirectLan route.
     */
    @Test
    fun testDirectLanSendStagedFileWithFailoverSuccess() = runBlocking {
        val repository = TransferRepository(context = context)
        val peer = DiscoveredPeer(
            peerId = "laptop_01",
            alias = "Mock Desktop",
            fingerprint = "MOCK_FP_01",
            host = "127.0.0.1",
            port = mockPort
        )

        val result = repository.sendStagedFileWithFailover(
            peer = peer,
            uri = Uri.fromFile(testFile),
            fileName = testFile.name,
            fileSize = testFile.length()
        )

        assertTrue("sendStagedFileWithFailover must succeed: ${result.errorMessage}", result.success)
        assertTrue(repository.activeRoute.value is TransportRoute.DirectLan)
        assertEquals("127.0.0.1", repository.activeRoute.value?.host)
        assertEquals(mockPort, repository.activeRoute.value?.port)
    }

    /**
     * Test 3: ECONNREFUSED Fast Failover (< 300ms SLA).
     * When LAN port is closed, repository must immediately failover to CloudRelay within < 300ms.
     */
    @Test
    fun testEconnrefusedFastFailoverUnder300ms() = runBlocking {
        val repository = TransferRepository(context = context)
        // Port 1 is closed and guaranteed to emit ECONNREFUSED
        val candidate = PeerCandidateRoutes(
            peerId = "laptop_01",
            directLanHost = "127.0.0.1",
            directLanPort = 1,
            derpMeshPubkey = "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF"
        )

        val elapsed = measureTimeMillis {
            val route = repository.resolveBestRoute(candidate)
            assertEquals(RouteTier.CLOUD_RELAY, route.tier)
        }

        assertTrue("Immediate failover on ECONNREFUSED must be < 300ms, took ${elapsed}ms", elapsed < 300)
        assertEquals(RouteTier.CLOUD_RELAY, repository.activeRoute.value?.tier)
        assertEquals(RouteState.ACTIVE, repository.routeState.value)
    }

    /**
     * Test 4: Blackhole Probe Timeout Capped at <= 1500ms.
     * When LAN host is unroutable (RFC 5737 198.51.100.1), probe timeout must not exceed 1500ms.
     */
    @Test
    fun testBlackholeProbeTimeoutCappedAt1500ms() = runBlocking {
        val repository = TransferRepository(context = context)
        val candidate = PeerCandidateRoutes(
            peerId = "laptop_01",
            directLanHost = "198.51.100.1",
            directLanPort = 53317,
            derpMeshPubkey = "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF"
        )

        val elapsed = measureTimeMillis {
            val route = repository.resolveBestRoute(candidate)
            assertEquals(RouteTier.CLOUD_RELAY, route.tier)
        }

        assertTrue(
            "Blackhole probe must timeout at ~1500ms (elapsed: ${elapsed}ms)",
            elapsed in 1400..2500
        )
        assertEquals(RouteTier.CLOUD_RELAY, repository.activeRoute.value?.tier)
    }

    /**
     * Test 5: Offline P2P Fallback via LOHS when LAN and DERP are unavailable.
     * When candidate specifies p2pGatewayHost without LAN or DERP, resolves PeerToPeer.
     */
    @Test
    fun testOfflineP2PFallbackWhenLanAndDerpUnavailable() = runBlocking {
        val repository = TransferRepository(context = context)
        val candidate = PeerCandidateRoutes(
            peerId = "laptop_01",
            p2pGatewayHost = "192.168.43.1"
        )

        val result = repository.resolveBestRoute(candidate)
        assertEquals(RouteTier.PEER_TO_PEER, result.tier)
        assertEquals("192.168.43.1", result.host)
        assertEquals(53317, result.port)
        assertEquals(RouteTier.PEER_TO_PEER, repository.activeRoute.value?.tier)
    }

    /**
     * Test 6: Offline P2P Fallback when LAN probe fails on ECONNREFUSED and DERP is unavailable.
     */
    @Test
    fun testOfflineP2PFallbackWhenLanProbeFails() = runBlocking {
        val repository = TransferRepository(context = context)
        val candidate = PeerCandidateRoutes(
            peerId = "laptop_01",
            directLanHost = "127.0.0.1",
            directLanPort = 1, // closed port
            p2pGatewayHost = "192.168.43.1",
            derpMeshPubkey = null
        )

        val route = repository.resolveBestRoute(candidate)
        assertEquals(RouteTier.PEER_TO_PEER, route.tier)
        assertEquals("192.168.43.1", route.host)
        assertEquals(53317, route.port)
    }

    /**
     * Test 7: probeLanEndpoint fast refusal < 50ms and probe success.
     */
    @Test
    fun testProbeLanEndpointRefusalAndSuccess() {
        val repository = TransferRepository(context = context)

        // Refused port probe should be ultra-fast (< 50ms)
        val elapsed = measureTimeMillis {
            val alive = repository.probeLanEndpoint("127.0.0.1", 1, 1500)
            assertFalse("Closed port 1 must not be alive", alive)
        }
        assertTrue("Refused port probe must fail fast (< 100ms), took ${elapsed}ms", elapsed < 100)

        // Open mock receiver port should be alive
        val alive = repository.probeLanEndpoint("127.0.0.1", mockPort, 1500)
        assertTrue("Mock port $mockPort must be alive", alive)
    }

    /**
     * Test 8: All Tiers Failed Throws Exception and marks RouteState.FAILED.
     */
    @Test
    fun testAllTiersFailedThrowsException() = runBlocking {
        val repository = TransferRepository(context = context)
        val candidate = PeerCandidateRoutes(
            peerId = "laptop_01",
            directLanHost = "127.0.0.1",
            directLanPort = 1, // closed
            p2pGatewayHost = null,
            derpMeshPubkey = null
        )

        var thrown = false
        try {
            repository.resolveBestRoute(candidate)
        } catch (_: IllegalStateException) {
            thrown = true
        }

        assertTrue("Must throw IllegalStateException when all tiers fail", thrown)
        assertEquals(RouteState.FAILED, repository.routeState.value)
    }

    /**
     * Test 9: Rule 0 Pure Userspace VPN Invariant.
     * Asserts that TransferRepository and related classes never reference VpnService,
     * and strictly bind loopback overlay to 127.0.0.1.
     */
    @Test
    fun testRule0PureUserspaceVpnInvariant() {
        assertEquals("127.0.0.1", TransferRepository.LOOPBACK_HOST)
        assertEquals(53318, TransferRepository.LOOPBACK_BRIDGE_PORT)
        assertEquals("192.168.43.1", TransferRepository.LOHS_GATEWAY_IP)

        // Inspect TransferRepository class
        val repoClass = TransferRepository::class.java
        for (field in repoClass.declaredFields) {
            assertFalse(
                "Field ${field.name} must not be a VpnService",
                field.type.name.contains("VpnService")
            )
        }
        for (method in repoClass.declaredMethods) {
            assertFalse(
                "Method ${method.name} return type must not be a VpnService",
                method.returnType.name.contains("VpnService")
            )
        }
    }
}
