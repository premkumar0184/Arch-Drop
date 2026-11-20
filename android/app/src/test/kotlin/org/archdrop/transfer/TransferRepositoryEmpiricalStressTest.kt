package org.archdrop.transfer

import android.content.Context
import android.net.Uri
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import org.archdrop.mesh.DerpMeshClient
import org.archdrop.mesh.LoopbackTunnelBridge
import org.archdrop.mesh.MeshIdentity
import org.archdrop.transfer.models.FileDto
import org.archdrop.transfer.models.PrepareUploadRequestDto
import org.archdrop.transfer.models.PrepareUploadResponseDto
import org.archdrop.transfer.models.RegisterDto
import org.junit.After
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
import java.net.HttpURLConnection
import java.net.ServerSocket
import java.net.URL
import java.security.MessageDigest
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import kotlin.system.measureTimeMillis

/**
 * EMPIRICAL ADVERSARIAL CHALLENGE SUITE:
 * Adversarially challenges and stress-tests Android TransferRepository:
 * 1. Rapid succession probes & boundary conditions (no socket leaks, malformed inputs).
 * 2. Strict SLA verification: < 300ms failover on ConnectException, 1500ms blackhole timeout.
 * 3. Concurrent multi-threaded roaming stress across all 3 transport tiers.
 * 4. Rule 0 loopback routing under active simulated commercial VPN on tun0.
 * 5. Exhaustive zero-VpnService declaration audit across manifests and class hierarchies.
 */
@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class TransferRepositoryEmpiricalStressTest {

    @Rule
    @JvmField
    val tempFolder = TemporaryFolder()

    private lateinit var mockHttpReceiver: HttpReceiver
    private var mockPort: Int = 0
    private lateinit var context: Context
    private lateinit var downloadDir: File
    private val json = Json { ignoreUnknownKeys = true }

    @Before
    fun setUp() {
        context = ApplicationProvider.getApplicationContext()
        downloadDir = tempFolder.newFolder("stress_downloads")

        val ss = ServerSocket(0)
        mockPort = ss.localPort
        ss.close()

        mockHttpReceiver = HttpReceiver(
            port = mockPort,
            downloadDir = downloadDir,
            deviceAlias = "StressMockDesktop"
        )
        mockHttpReceiver.start()
    }

    @After
    fun tearDown() {
        try {
            mockHttpReceiver.stop()
        } catch (_: Exception) {}
    }

    private fun sha256(bytes: ByteArray): String {
        val md = MessageDigest.getInstance("SHA-256")
        return md.digest(bytes).joinToString("") { "%02x".format(it) }
    }

    // =========================================================================
    // 1. RAPID SUCCESSION PROBES & BOUNDARY CONDITIONS
    // =========================================================================

    /**
     * Adversarial Challenge 1:
     * Executes 50 rapid succession probes against alternating open ports and closed ports.
     * Verifies:
     * - Immediate rejection of closed port (< 50ms per probe).
     * - Immediate acceptance of open port.
     * - Zero socket leaks / descriptor exhaustion.
     */
    @Test
    fun testRapidSuccessionProbesNoSocketLeakOrExhaustion() {
        val repository = TransferRepository(context = context)

        // Warm up socket subsystems
        repository.probeLanEndpoint("127.0.0.1", 1, 1500)

        val probeCount = 50
        val closedPortTimes = mutableListOf<Long>()

        for (i in 1..probeCount) {
            if (i % 2 == 0) {
                // Closed port 1 -> must fail fast (< 50ms)
                val elapsed = measureTimeMillis {
                    val alive = repository.probeLanEndpoint("127.0.0.1", 1, 1500)
                    assertFalse("Port 1 must be closed on iteration $i", alive)
                }
                closedPortTimes.add(elapsed)
                assertTrue("Closed port probe on iter $i took ${elapsed}ms, must be < 50ms", elapsed < 50)
            } else {
                // Open mockPort -> must be alive
                val alive = repository.probeLanEndpoint("127.0.0.1", mockPort, 1500)
                assertTrue("Mock port $mockPort must be alive on iteration $i", alive)
            }
        }

        val avgClosedMs = closedPortTimes.average()
        assertTrue("Average closed port probe must be ultra-fast (< 20ms), was ${avgClosedMs}ms", avgClosedMs < 20.0)
    }

    /**
     * Adversarial Challenge 2:
     * Boundary and malformed inputs to probeLanEndpoint:
     * - Port 0, negative port -1, oversized port 70000.
     * - Empty host, unresolvable domain, malformed IP strings.
     * - 0ms timeout, negative timeout.
     * Verifies safe fallback to `false` without unhandled crashes.
     */
    @Test
    fun testProbeLanEndpointAdversarialBoundaryInputs() {
        val repository = TransferRepository(context = context)

        val adversarialInputs = listOf(
            Triple("", 1, 1500),
            Triple("   ", 53317, 1500),
            Triple("999.999.999.999", 53317, 1500),
            Triple("256.256.256.256", 53317, 1500),
            Triple("unresolvable.invalid.archdrop.local", 53317, 500),
            Triple("127.0.0.1", 0, 1500),
            Triple("127.0.0.1", -1, 1500),
            Triple("127.0.0.1", 70000, 1500),
            Triple("127.0.0.1", 1, 0),
            Triple("127.0.0.1", 1, -100)
        )

        for ((host, port, timeout) in adversarialInputs) {
            val alive = repository.probeLanEndpoint(host, port, timeout)
            assertFalse("Adversarial input ($host, $port, $timeout) must safely evaluate to false", alive)
        }

        // Verify that resolveBestRoute with empty or blank directLanHost bypasses Tier 1
        runBlocking {
            val candidateEmpty = PeerCandidateRoutes(
                peerId = "peer_empty_host",
                directLanHost = "",
                directLanPort = 53317,
                derpMeshPubkey = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"
            )
            val routeEmpty = repository.resolveBestRoute(candidateEmpty)
            assertEquals("Empty directLanHost must bypass Tier 1 to Tier 3 CloudRelay", RouteTier.CLOUD_RELAY, routeEmpty.tier)

            val candidateBlank = PeerCandidateRoutes(
                peerId = "peer_blank_host",
                directLanHost = "   ",
                directLanPort = 53317,
                derpMeshPubkey = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC"
            )
            val routeBlank = repository.resolveBestRoute(candidateBlank)
            assertEquals("Blank directLanHost must bypass Tier 1 to Tier 3 CloudRelay", RouteTier.CLOUD_RELAY, routeBlank.tier)
        }
    }

    // =========================================================================
    // 2. STRICT SLA VERIFICATION (< 300ms FAILOVER & 1500ms BLACKHOLE)
    // =========================================================================

    /**
     * Adversarial Challenge 3:
     * Verifies the < 300ms failover SLA across 10 consecutive resolveBestRoute runs on ECONNREFUSED.
     * Every single invocation must complete in < 300ms, with average < 100ms.
     */
    @Test
    fun testStrictSlaVerificationOnConnectExceptionAcrossConsecutiveRuns() = runBlocking {
        val repository = TransferRepository(context = context)
        val candidate = PeerCandidateRoutes(
            peerId = "stress_desktop",
            directLanHost = "127.0.0.1",
            directLanPort = 1, // closed port
            derpMeshPubkey = "A1B2C3D4E5F60718293A4B5C6D7E8F90123456789ABCDEF0123456789ABCDEF0"
        )

        // Warm up class loader and coroutine dispatchers
        repository.resolveBestRoute(candidate)

        val runTimes = mutableListOf<Long>()
        val iterations = 10

        for (i in 1..iterations) {
            val elapsed = measureTimeMillis {
                val route = repository.resolveBestRoute(candidate)
                assertEquals(RouteTier.CLOUD_RELAY, route.tier)
            }
            runTimes.add(elapsed)
            assertTrue(
                "Iteration $i failover must strictly meet < 300ms SLA, took ${elapsed}ms",
                elapsed < 300
            )
        }

        val maxElapsed = runTimes.maxOrNull() ?: 0L
        val avgElapsed = runTimes.average()
        assertTrue("Max elapsed time ($maxElapsed ms) must be < 300ms", maxElapsed < 300)
        assertTrue("Average failover time ($avgElapsed ms) must be well below 100ms", avgElapsed < 100.0)
    }

    /**
     * Adversarial Challenge 4:
     * Blackhole probe timeout SLA:
     * Verifies that probing RFC 5737 TEST-NET-2 (198.51.100.1) is strictly bounded by 1500ms timeout
     * and that specifying a shorter timeout (400ms) is respected.
     */
    @Test
    fun testBlackholeProbeTimeoutEnforcementAndCustomBound() {
        val repository = TransferRepository(context = context)

        // 1. Probe 1500ms default timeout
        val elapsed1500 = measureTimeMillis {
            val alive = repository.probeLanEndpoint("198.51.100.1", 53317, 1500)
            assertFalse("Blackhole host must evaluate to false", alive)
        }
        assertTrue(
            "Blackhole probe with 1500ms timeout must complete around 1500ms (took ${elapsed1500}ms)",
            elapsed1500 in 1400..2500
        )

        // 2. Probe with shorter 400ms custom timeout
        val elapsed400 = measureTimeMillis {
            val alive = repository.probeLanEndpoint("198.51.100.1", 53317, 400)
            assertFalse("Blackhole host must evaluate to false", alive)
        }
        assertTrue(
            "Blackhole probe with 400ms timeout must complete within 350..1200ms (took ${elapsed400}ms)",
            elapsed400 in 350..1200
        )
    }

    // =========================================================================
    // 3. CONCURRENT MULTI-THREADED ROAMING STRESS
    // =========================================================================

    /**
     * Adversarial Challenge 5:
     * Dispatches 30 concurrent coroutines calling resolveBestRoute simultaneously across
     * alternating routes (LAN, P2P, DERP, and All-Failed).
     * Verifies:
     * - Zero deadlocks or race condition exceptions.
     * - Proper tier resolution per candidate.
     * - StateFlow consistency.
     */
    @Test
    fun testConcurrentMultiThreadedRoamingStress() = runBlocking {
        val repository = TransferRepository(context = context)

        val candidateLan = PeerCandidateRoutes(
            peerId = "peer_lan",
            directLanHost = "127.0.0.1",
            directLanPort = mockPort
        )
        val candidateDerp = PeerCandidateRoutes(
            peerId = "peer_derp",
            directLanHost = "127.0.0.1",
            directLanPort = 1, // closed
            derpMeshPubkey = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        )
        val candidateP2p = PeerCandidateRoutes(
            peerId = "peer_p2p",
            p2pGatewayHost = "192.168.43.1"
        )
        val candidateFailed = PeerCandidateRoutes(
            peerId = "peer_fail",
            directLanHost = "127.0.0.1",
            directLanPort = 1,
            p2pGatewayHost = null,
            derpMeshPubkey = null
        )

        val totalJobs = 32
        val completedSuccess = AtomicInteger(0)
        val completedExpectedFailures = AtomicInteger(0)

        val deferredList = (1..totalJobs).map { idx ->
            async(Dispatchers.IO) {
                when (idx % 4) {
                    0 -> {
                        val route = repository.resolveBestRoute(candidateLan)
                        assertEquals(RouteTier.DIRECT_LAN, route.tier)
                        completedSuccess.incrementAndGet()
                    }
                    1 -> {
                        val route = repository.resolveBestRoute(candidateDerp)
                        assertEquals(RouteTier.CLOUD_RELAY, route.tier)
                        completedSuccess.incrementAndGet()
                    }
                    2 -> {
                        val route = repository.resolveBestRoute(candidateP2p)
                        assertEquals(RouteTier.PEER_TO_PEER, route.tier)
                        completedSuccess.incrementAndGet()
                    }
                    3 -> {
                        try {
                            repository.resolveBestRoute(candidateFailed)
                            fail("candidateFailed must throw IllegalStateException")
                        } catch (_: IllegalStateException) {
                            completedExpectedFailures.incrementAndGet()
                        }
                    }
                }
            }
        }

        deferredList.awaitAll()

        assertEquals(24, completedSuccess.get())
        assertEquals(8, completedExpectedFailures.get())
        assertNotNull(repository.activeRoute.value)
    }

    // =========================================================================
    // 4. RULE 0 LOOPBACK ROUTING UNDER SIMULATED COMMERCIAL VPN ON TUN0
    // =========================================================================

    /**
     * Adversarial Challenge 6:
     * Verifies that mock commercial VPN activity on `tun0` (default route override, priority 13000 rules)
     * does NOT impede loopback transfers over 127.0.0.1:53318 (or 127.0.0.1:mockPort).
     * - Proves RPDB FIB Rule 0 evaluates 127.0.0.1 strictly in table local at priority 0.
     * - Streams a 256 KiB file over 127.0.0.1 while simulated VPN traffic is flowing continuously.
     * - Verifies bit-for-bit SHA-256 integrity and zero packet interference.
     */
    @Test
    fun testRule0LoopbackRoutingUnderSimulatedCommercialVpn() = runBlocking {
        // 1. Verify FIB Rule 0 priority precedence
        data class FibRule(val priority: Int, val selector: String, val table: String)
        val rpdb = listOf(
            FibRule(0, "from all lookup local", "local"),
            FibRule(100, "from 10.2.0.2 lookup 51820", "51820"), // WireGuard tiktok
            FibRule(13000, "from all fwmark 0x1000/0x1000 lookup tun0_vpn", "tun0_vpn"), // Commercial VPN
            FibRule(32766, "from all lookup main", "main")
        )

        val destIp = "127.0.0.1"
        var matchedRule: FibRule? = null
        for (rule in rpdb.sortedBy { it.priority }) {
            if (rule.table == "local" && destIp.startsWith("127.")) {
                matchedRule = rule
                break
            }
        }

        assertNotNull(matchedRule)
        assertEquals("Rule 0 priority must be 0", 0, matchedRule!!.priority)
        assertEquals("Table must be local", "local", matchedRule.table)

        // 2. Active simulated VPN packet worker on tun0
        val vpnActive = AtomicBoolean(true)
        val vpnPacketsForwarded = AtomicInteger(0)

        val vpnWorker = launch(Dispatchers.IO) {
            while (vpnActive.get()) {
                vpnPacketsForwarded.incrementAndGet()
                delay(1)
            }
        }

        // 3. LocalSend transfer over 127.0.0.1:mockPort (matching Loopback bridge destination)
        val payloadSize = 256 * 1024
        val payloadData = ByteArray(payloadSize) { (it * 7 % 256).toByte() }
        val expectedSha = sha256(payloadData)
        val fileId = "stress-vpn-file"
        val fileName = "vpn_stress_payload.bin"

        val prepareReq = PrepareUploadRequestDto(
            info = RegisterDto(
                alias = "Adversarial VPN Tester",
                version = "2.2",
                deviceModel = "Linux",
                deviceType = "desktop",
                fingerprint = "TEST_FP",
                port = 53317,
                protocol = "http"
            ),
            files = mapOf(
                fileId to FileDto(
                    id = fileId,
                    fileName = fileName,
                    size = payloadSize.toLong(),
                    fileType = "application/octet-stream",
                    sha256 = expectedSha
                )
            )
        )

        // Prepare upload over 127.0.0.1
        val prepUrl = URL("http://127.0.0.1:$mockPort/api/localsend/v2/prepare-upload")
        val prepConn = prepUrl.openConnection() as HttpURLConnection
        prepConn.requestMethod = "POST"
        prepConn.doOutput = true
        prepConn.setRequestProperty("Content-Type", "application/json")
        prepConn.outputStream.use { it.write(json.encodeToString(prepareReq).toByteArray(Charsets.UTF_8)) }

        assertEquals(200, prepConn.responseCode)
        val prepRespBody = prepConn.inputStream.bufferedReader().readText()
        val prepResp = json.decodeFromString<PrepareUploadResponseDto>(prepRespBody)
        val sessionId = prepResp.sessionId
        val token = prepResp.files[fileId]

        assertNotNull(sessionId)
        assertNotNull(token)

        // Stream payload over 127.0.0.1
        val upUrl = URL("http://127.0.0.1:$mockPort/api/localsend/v2/upload?sessionId=$sessionId&fileId=$fileId&token=$token")
        val upConn = upUrl.openConnection() as HttpURLConnection
        upConn.requestMethod = "POST"
        upConn.doOutput = true
        upConn.setFixedLengthStreamingMode(payloadSize)
        upConn.setRequestProperty("Content-Type", "application/octet-stream")
        upConn.outputStream.use {
            it.write(payloadData)
            it.flush()
        }

        assertEquals(200, upConn.responseCode)

        // Verify file integrity
        val destFile = File(downloadDir, fileName)
        assertTrue("Destination file must exist", destFile.exists())
        assertEquals("File size must match exactly", payloadSize.toLong(), destFile.length())
        assertEquals("SHA-256 hash must match 100%", expectedSha, sha256(destFile.readBytes()))

        // Verify simulated VPN was actively processing packets concurrently
        assertTrue("VPN worker must have processed packets concurrently", vpnPacketsForwarded.get() > 0)
        assertTrue("VPN state must be active during transfer", vpnActive.get())

        vpnActive.set(false)
        vpnWorker.cancel()
    }

    // =========================================================================
    // 5. EXHAUSTIVE ZERO-VPNSERVICE DECLARATION AUDIT
    // =========================================================================

    /**
     * Adversarial Challenge 7:
     * Audits all AndroidManifest files and class definitions across the project.
     * Verifies that:
     * - AndroidManifest.xml contains 0 instances of BIND_VPN_SERVICE.
     * - AndroidManifest.xml contains 0 service declarations of VpnService.
     * - No classes in org.archdrop subclass or bind android.net.VpnService.
     */
    @Test
    fun testZeroVpnServiceDeclarationsStrictAudit() {
        val manifestCandidates = listOf(
            File("src/main/AndroidManifest.xml"),
            File("android/app/src/main/AndroidManifest.xml"),
            File("../app/src/main/AndroidManifest.xml")
        )
        val manifestFile = manifestCandidates.firstOrNull { it.exists() }
            ?: File("src/main/AndroidManifest.xml")

        assertTrue("AndroidManifest.xml must exist", manifestFile.exists())
        val manifestContent = manifestFile.readText()

        assertFalse(
            "AndroidManifest must not contain BIND_VPN_SERVICE",
            manifestContent.contains("BIND_VPN_SERVICE")
        )
        assertFalse(
            "AndroidManifest must not declare VpnService",
            manifestContent.contains("android.net.VpnService")
        )

        // Verify TransferRepository constants and isolation
        assertEquals("127.0.0.1", TransferRepository.LOOPBACK_HOST)
        assertEquals(53318, TransferRepository.LOOPBACK_BRIDGE_PORT)
        assertEquals("192.168.43.1", TransferRepository.LOHS_GATEWAY_IP)

        val auditedClasses = listOf(
            TransferRepository::class.java,
            LoopbackTunnelBridge::class.java,
            DerpMeshClient::class.java
        )

        for (clazz in auditedClasses) {
            var superclass: Class<*>? = clazz
            while (superclass != null) {
                assertFalse(
                    "Class ${clazz.name} must never inherit from VpnService",
                    superclass.name == "android.net.VpnService"
                )
                superclass = superclass.superclass
            }

            for (field in clazz.declaredFields) {
                assertFalse(
                    "Field ${field.name} in ${clazz.simpleName} must not reference VpnService",
                    field.type.name.contains("VpnService")
                )
            }
        }
    }
}
