package org.archdrop.ui.webcam

import android.app.Application
import android.content.Context
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.archdrop.webcam.WebcamConnectionState
import org.archdrop.webcam.WebcamLens
import org.archdrop.webcam.WebcamResolution
import org.archdrop.webcam.WebcamStreamManager
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.lang.reflect.Method

/**
 * Adversarial fake stream manager allowing direct state manipulation.
 */
class AdversarialFakeWebcamStreamManager(context: Context) : WebcamStreamManager(context) {
    private var streamingActive = false
    var startStreamingCallCount = 0
    var stopStreamingCallCount = 0
    var lastTargetHost: String? = null
    var lastTargetPort: Int = 0
    var lastResolution: WebcamResolution? = null
    var lastLens: WebcamLens? = null

    override fun isStreamingActive(): Boolean = streamingActive

    override fun startStreaming(
        lifecycleOwner: LifecycleOwner,
        targetHost: String,
        targetPort: Int,
        resolution: WebcamResolution,
        lens: WebcamLens,
        localFingerprint: String,
        quality: Int
    ) {
        startStreamingCallCount++
        streamingActive = true
        lastTargetHost = targetHost
        lastTargetPort = targetPort
        lastResolution = resolution
        lastLens = lens
        _connectionState.value = WebcamConnectionState.Streaming
        _fps.value = 30.0
    }

    override fun stopStreaming() {
        stopStreamingCallCount++
        streamingActive = false
        _connectionState.value = WebcamConnectionState.Disconnected
        _fps.value = 0.0
    }

    fun updateConnectionStateDirectly(state: WebcamConnectionState, active: Boolean? = null) {
        if (active != null) {
            streamingActive = active
        }
        _connectionState.value = state
        if (state is WebcamConnectionState.Streaming) {
            _fps.value = 30.0
        } else if (state is WebcamConnectionState.Error || state is WebcamConnectionState.Disconnected || state is WebcamConnectionState.Idle) {
            _fps.value = 0.0
        }
    }

    fun simulateError(message: String) {
        streamingActive = false
        _connectionState.value = WebcamConnectionState.Error(message)
        _fps.value = 0.0
    }
}

/**
 * Empirical adversarial stress tests challenging:
 * 1. ViewModel State Machine Concurrency (rapid start/stop, dynamic resolution/lens switching, idempotency).
 * 2. Preference Resilience (malformed, whitespace-only, IPv6 host strings, fallback to 10.2.0.2).
 * 3. Connection state sync across all network lifecycle transitions.
 * 4. Teardown safety and onCleared cleanup.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class WebcamViewModelAdversarialStressTest {

    private lateinit var app: Application
    private val testDispatcher = StandardTestDispatcher()
    private val testScope = TestScope(testDispatcher)

    @Before
    fun setUp() {
        Dispatchers.setMain(testDispatcher)
        app = ApplicationProvider.getApplicationContext<Application>()
        app.getSharedPreferences("archdrop_prefs", Context.MODE_PRIVATE)
            .edit()
            .clear()
            .apply()
    }

    @After
    fun tearDown() {
        Dispatchers.resetMain()
    }

    @Test
    fun testRapidBackToBackStartStop_maintainsConsistentState() = runTest {
        val fakeManager = AdversarialFakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        // 100 rapid start -> stop cycles
        for (i in 1..100) {
            viewModel.startStreaming(lifecycleOwner)
            testScheduler.advanceTimeBy(500)
            assertTrue("Iteration $i: Streaming must be true after startStreaming", viewModel.isStreaming.value)
            assertTrue("Iteration $i: Fake manager must report active", fakeManager.isStreamingActive())
            assertEquals(i, fakeManager.startStreamingCallCount)

            viewModel.stopStreaming()
            testScheduler.advanceTimeBy(500)
            assertFalse("Iteration $i: Streaming must be false after stopStreaming", viewModel.isStreaming.value)
            assertFalse("Iteration $i: Fake manager must report inactive", fakeManager.isStreamingActive())
            assertEquals(i, fakeManager.stopStreamingCallCount)
        }

        assertEquals(100, fakeManager.startStreamingCallCount)
        assertEquals(100, fakeManager.stopStreamingCallCount)
        assertFalse(viewModel.isStreaming.value)
    }

    @Test
    fun testRapidIdempotentStartStreaming_singleInvocation() = runTest {
        val fakeManager = AdversarialFakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        viewModel.startStreaming(lifecycleOwner)
        testScheduler.advanceTimeBy(500)
        assertTrue(viewModel.isStreaming.value)
        assertEquals(1, fakeManager.startStreamingCallCount)

        // 100 consecutive calls to startStreaming must be strictly no-ops
        for (i in 1..100) {
            viewModel.startStreaming(lifecycleOwner)
            testScheduler.advanceTimeBy(500)
            assertTrue(viewModel.isStreaming.value)
            assertEquals("Start call count must remain 1 on duplicate call $i", 1, fakeManager.startStreamingCallCount)
        }

        viewModel.stopStreaming()
        testScheduler.advanceTimeBy(500)
        assertFalse(viewModel.isStreaming.value)
        assertEquals(1, fakeManager.stopStreamingCallCount)
    }

    @Test
    fun testRapidIdempotentStopStreaming_whenAlreadyStopped() = runTest {
        val fakeManager = AdversarialFakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)

        // Calling stopStreaming 50 times when not streaming
        assertFalse(viewModel.isStreaming.value)
        for (i in 1..50) {
            viewModel.stopStreaming()
            testScheduler.advanceTimeBy(500)
            assertFalse(viewModel.isStreaming.value)
        }
    }

    @Test
    fun testRapidResolutionSwitching_whileStreaming() = runTest {
        val fakeManager = AdversarialFakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        viewModel.startStreaming(lifecycleOwner)
        testScheduler.advanceTimeBy(500)
        assertTrue(viewModel.isStreaming.value)
        assertEquals(1, fakeManager.startStreamingCallCount)

        val resolutions = listOf(
            WebcamResolution.RES_480P,
            WebcamResolution.RES_1080P,
            WebcamResolution.RES_720P,
            WebcamResolution.RES_1080P,
            WebcamResolution.RES_480P,
            WebcamResolution.RES_720P
        )

        var expectedStops = 0
        var expectedStarts = 1

        for (res in resolutions) {
            viewModel.setResolution(res, lifecycleOwner)
            testScheduler.advanceTimeBy(500)
            expectedStops++
            expectedStarts++

            assertEquals(res, viewModel.selectedResolution.value)
            assertEquals(res, fakeManager.lastResolution)
            assertEquals(expectedStops, fakeManager.stopStreamingCallCount)
            assertEquals(expectedStarts, fakeManager.startStreamingCallCount)
            assertTrue(viewModel.isStreaming.value)
        }

        // Setting the exact same resolution should be a no-op (no restart)
        val currentRes = viewModel.selectedResolution.value
        for (i in 1..10) {
            viewModel.setResolution(currentRes, lifecycleOwner)
            testScheduler.advanceTimeBy(500)
            assertEquals("Redundant resolution change must not increment stop count", expectedStops, fakeManager.stopStreamingCallCount)
            assertEquals("Redundant resolution change must not increment start count", expectedStarts, fakeManager.startStreamingCallCount)
        }

        viewModel.stopStreaming()
        testScheduler.advanceTimeBy(500)
        assertFalse(viewModel.isStreaming.value)
    }

    @Test
    fun testRapidLensSwitching_whileStreaming() = runTest {
        val fakeManager = AdversarialFakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        viewModel.startStreaming(lifecycleOwner)
        testScheduler.advanceTimeBy(500)
        assertTrue(viewModel.isStreaming.value)
        assertEquals(1, fakeManager.startStreamingCallCount)

        var expectedStops = 0
        var expectedStarts = 1

        // 30 rapid back/front lens switches
        for (i in 1..30) {
            val targetLens = if (i % 2 == 1) WebcamLens.FRONT else WebcamLens.BACK
            viewModel.setLens(targetLens, lifecycleOwner)
            testScheduler.advanceTimeBy(500)
            expectedStops++
            expectedStarts++

            assertEquals(targetLens, viewModel.selectedLens.value)
            assertEquals(targetLens, fakeManager.lastLens)
            assertEquals(expectedStops, fakeManager.stopStreamingCallCount)
            assertEquals(expectedStarts, fakeManager.startStreamingCallCount)
            assertTrue(viewModel.isStreaming.value)
        }

        // Setting the exact same lens should be a no-op (no restart)
        val currentLens = viewModel.selectedLens.value
        for (i in 1..10) {
            viewModel.setLens(currentLens, lifecycleOwner)
            testScheduler.advanceTimeBy(500)
            assertEquals("Redundant lens change must not increment stop count", expectedStops, fakeManager.stopStreamingCallCount)
            assertEquals("Redundant lens change must not increment start count", expectedStarts, fakeManager.startStreamingCallCount)
        }

        viewModel.stopStreaming()
        testScheduler.advanceTimeBy(500)
        assertFalse(viewModel.isStreaming.value)
    }

    @Test
    fun testResolutionAndLensSwitching_whenNotStreaming() = runTest {
        val fakeManager = AdversarialFakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        assertFalse(viewModel.isStreaming.value)

        // Switching resolution with lifecycleOwner when idle should update state but NOT start stream
        viewModel.setResolution(WebcamResolution.RES_1080P, lifecycleOwner)
        testScheduler.advanceTimeBy(500)
        assertEquals(WebcamResolution.RES_1080P, viewModel.selectedResolution.value)
        assertFalse(viewModel.isStreaming.value)
        assertEquals(0, fakeManager.startStreamingCallCount)

        // Switching lens with lifecycleOwner when idle should update state but NOT start stream
        viewModel.setLens(WebcamLens.FRONT, lifecycleOwner)
        testScheduler.advanceTimeBy(500)
        assertEquals(WebcamLens.FRONT, viewModel.selectedLens.value)
        assertFalse(viewModel.isStreaming.value)
        assertEquals(0, fakeManager.startStreamingCallCount)
    }

    @Test
    fun testPreferenceFallback_emptyWhitespaceAndNull() = runTest {
        // 1. Unset preference -> fallback to "10.2.0.2"
        val vm1 = WebcamViewModel(app)
        assertEquals("Unset preference must fall back to 10.2.0.2", "10.2.0.2", vm1.targetHost.value)

        // 2. Empty string preference in SharedPreferences -> fallback to "10.2.0.2"
        app.getSharedPreferences("archdrop_prefs", Context.MODE_PRIVATE)
            .edit()
            .putString("last_peer_host", "")
            .apply()
        val vm2 = WebcamViewModel(app)
        assertEquals("Empty string preference must fall back to 10.2.0.2", "10.2.0.2", vm2.targetHost.value)

        // 3. Whitespace-only string in SharedPreferences -> fallback to "10.2.0.2"
        app.getSharedPreferences("archdrop_prefs", Context.MODE_PRIVATE)
            .edit()
            .putString("last_peer_host", "    \t \n  ")
            .apply()
        val vm3 = WebcamViewModel(app)
        assertEquals("Whitespace-only preference must fall back to 10.2.0.2", "10.2.0.2", vm3.targetHost.value)

        // 4. Calling setTargetHost with blank strings must not overwrite existing valid host
        val vm4 = WebcamViewModel(app)
        vm4.setTargetHost("192.168.1.150")
        assertEquals("192.168.1.150", vm4.targetHost.value)

        vm4.setTargetHost("")
        assertEquals("Blank setTargetHost must be ignored", "192.168.1.150", vm4.targetHost.value)

        vm4.setTargetHost("    ")
        assertEquals("Whitespace setTargetHost must be ignored", "192.168.1.150", vm4.targetHost.value)

        vm4.setTargetHost("\t\r\n")
        assertEquals("Control whitespace setTargetHost must be ignored", "192.168.1.150", vm4.targetHost.value)

        val persisted = app.getSharedPreferences("archdrop_prefs", Context.MODE_PRIVATE)
            .getString("last_peer_host", null)
        assertEquals("192.168.1.150", persisted)
    }

    @Test
    fun testPreferenceResilience_IPv6AndComplexHosts() = runTest {
        val viewModel = WebcamViewModel(app)

        // IPv6 unbracketed
        viewModel.setTargetHost("fe80::1ff:fe23:4567:890a")
        assertEquals("fe80::1ff:fe23:4567:890a", viewModel.targetHost.value)

        // IPv6 bracketed
        viewModel.setTargetHost("[::1]")
        assertEquals("[::1]", viewModel.targetHost.value)

        // IPv6 with whitespace
        viewModel.setTargetHost("   2001:0db8:85a3:0000:0000:8a2e:0370:7334   ")
        assertEquals("2001:0db8:85a3:0000:0000:8a2e:0370:7334", viewModel.targetHost.value)

        // mDNS hostname
        viewModel.setTargetHost("  laptop.local  ")
        assertEquals("laptop.local", viewModel.targetHost.value)

        // WireGuard standard IP
        viewModel.setTargetHost("10.2.0.2")
        assertEquals("10.2.0.2", viewModel.targetHost.value)
    }

    @Test
    fun testConnectionStateSync_allTransitions() = runTest {
        val fakeManager = AdversarialFakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)

        // Initial Idle
        assertTrue(viewModel.connectionState.value is WebcamConnectionState.Idle)
        assertFalse(viewModel.isStreaming.value)

        // Connecting -> isStreaming is true
        fakeManager.updateConnectionStateDirectly(WebcamConnectionState.Connecting, active = true)
        testScheduler.advanceTimeBy(500)
        assertTrue(viewModel.isStreaming.value)

        // Streaming -> isStreaming is true
        fakeManager.updateConnectionStateDirectly(WebcamConnectionState.Streaming, active = true)
        testScheduler.advanceTimeBy(500)
        assertTrue(viewModel.isStreaming.value)

        // Error -> isStreaming is false (streamManager.isStreamingActive() is false)
        fakeManager.simulateError("Connection refused: 53317")
        testScheduler.advanceTimeBy(500)
        assertFalse(viewModel.isStreaming.value)
        assertTrue(viewModel.connectionState.value is WebcamConnectionState.Error)
        assertEquals("Connection refused: 53317", (viewModel.connectionState.value as WebcamConnectionState.Error).message)

        // Disconnected -> isStreaming is false
        fakeManager.updateConnectionStateDirectly(WebcamConnectionState.Disconnected, active = false)
        testScheduler.advanceTimeBy(500)
        assertFalse(viewModel.isStreaming.value)

        // Idle -> isStreaming is false
        fakeManager.updateConnectionStateDirectly(WebcamConnectionState.Idle, active = false)
        testScheduler.advanceTimeBy(500)
        assertFalse(viewModel.isStreaming.value)
    }

    @Test
    fun testTeardown_onClearedStopsStreaming() = runTest {
        val fakeManager = AdversarialFakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        viewModel.startStreaming(lifecycleOwner)
        testScheduler.advanceTimeBy(500)
        assertTrue(viewModel.isStreaming.value)
        assertEquals(1, fakeManager.startStreamingCallCount)

        // Invoke protected onCleared via reflection
        val onClearedMethod: Method = WebcamViewModel::class.java.getDeclaredMethod("onCleared")
        onClearedMethod.isAccessible = true
        onClearedMethod.invoke(viewModel)
        testScheduler.advanceTimeBy(500)

        assertFalse("Streaming must be inactive after onCleared", viewModel.isStreaming.value)
        assertEquals("stopStreaming must be called on onCleared", 1, fakeManager.stopStreamingCallCount)
        assertFalse("StreamManager must be stopped", fakeManager.isStreamingActive())
    }
}
