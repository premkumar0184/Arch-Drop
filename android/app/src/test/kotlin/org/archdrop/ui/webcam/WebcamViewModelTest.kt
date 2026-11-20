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
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

class MockLifecycleOwner(
    initialState: Lifecycle.State = Lifecycle.State.RESUMED
) : LifecycleOwner {
    private val registry = LifecycleRegistry(this).apply {
        currentState = initialState
    }
    override val lifecycle: Lifecycle get() = registry
}

class FakeWebcamStreamManager(context: Context) : WebcamStreamManager(context) {
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
        _fps.value = 29.8
    }

    override fun stopStreaming() {
        stopStreamingCallCount++
        streamingActive = false
        _connectionState.value = WebcamConnectionState.Disconnected
        _fps.value = 0.0
    }

    fun setConnectionState(state: WebcamConnectionState) {
        _connectionState.value = state
    }

    fun simulateError(message: String) {
        streamingActive = false
        _connectionState.value = WebcamConnectionState.Error(message)
        _fps.value = 0.0
    }
}

@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class WebcamViewModelTest {

    private lateinit var app: Application
    private val testDispatcher = StandardTestDispatcher()
    private val testScope = TestScope(testDispatcher)

    @Before
    fun setUp() {
        Dispatchers.setMain(testDispatcher)
        app = ApplicationProvider.getApplicationContext<Application>()
        // Clear prefs before each test
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
    fun testInitialState_defaultsAreCompliant() = runTest {
        val viewModel = WebcamViewModel(app)

        assertEquals("Default resolution must be 720p HD", WebcamResolution.RES_720P, viewModel.selectedResolution.value)
        assertEquals("Default lens must be BACK camera", WebcamLens.BACK, viewModel.selectedLens.value)
        assertFalse("Streaming must be idle initially", viewModel.isStreaming.value)
        assertFalse("Low power mode must be disabled by default", viewModel.isLowPowerMode.value)
        assertFalse("Camera permission must be false initially", viewModel.hasCameraPermission.value)
        assertEquals(53317, viewModel.targetPort)
        assertEquals("Default fallback host must be 10.2.0.2", "10.2.0.2", viewModel.targetHost.value)
        assertEquals(0.0, viewModel.fps.value, 0.001)
        assertTrue("Connection state must be Idle", viewModel.connectionState.value is WebcamConnectionState.Idle)
    }

    @Test
    fun testInitialState_loadsSavedHostPreference() = runTest {
        app.getSharedPreferences("archdrop_prefs", Context.MODE_PRIVATE)
            .edit()
            .putString("last_peer_host", "192.168.1.120")
            .apply()

        val viewModel = WebcamViewModel(app)
        assertEquals("192.168.1.120", viewModel.targetHost.value)
    }

    @Test
    fun testSetTargetHost_persistsAndTrims() = runTest {
        val viewModel = WebcamViewModel(app)
        viewModel.setTargetHost("   192.168.1.42   ")

        assertEquals("192.168.1.42", viewModel.targetHost.value)
        val saved = app.getSharedPreferences("archdrop_prefs", Context.MODE_PRIVATE)
            .getString("last_peer_host", null)
        assertEquals("192.168.1.42", saved)

        // Empty/blank host updates should be ignored
        viewModel.setTargetHost("   ")
        assertEquals("192.168.1.42", viewModel.targetHost.value)
    }

    @Test
    fun testResolutionSelection_updatesState() = runTest {
        val viewModel = WebcamViewModel(app)

        viewModel.setResolution(WebcamResolution.RES_480P)
        assertEquals(WebcamResolution.RES_480P, viewModel.selectedResolution.value)
        assertEquals(640, viewModel.selectedResolution.value.width)
        assertEquals(480, viewModel.selectedResolution.value.height)

        viewModel.setResolution(WebcamResolution.RES_1080P)
        assertEquals(WebcamResolution.RES_1080P, viewModel.selectedResolution.value)
        assertEquals(1920, viewModel.selectedResolution.value.width)
        assertEquals(1080, viewModel.selectedResolution.value.height)

        viewModel.setResolution(WebcamResolution.RES_720P)
        assertEquals(WebcamResolution.RES_720P, viewModel.selectedResolution.value)
    }

    @Test
    fun testLensSelection_updatesState() = runTest {
        val viewModel = WebcamViewModel(app)

        viewModel.setLens(WebcamLens.FRONT)
        assertEquals(WebcamLens.FRONT, viewModel.selectedLens.value)
        assertEquals("Front Camera", viewModel.selectedLens.value.label)

        viewModel.setLens(WebcamLens.BACK)
        assertEquals(WebcamLens.BACK, viewModel.selectedLens.value)
        assertEquals("Back Camera", viewModel.selectedLens.value.label)
    }

    @Test
    fun testLowPowerModeToggle_togglesAndSetsDirectly() = runTest {
        val viewModel = WebcamViewModel(app)

        assertFalse(viewModel.isLowPowerMode.value)
        viewModel.toggleLowPowerMode()
        assertTrue(viewModel.isLowPowerMode.value)

        viewModel.toggleLowPowerMode()
        assertFalse(viewModel.isLowPowerMode.value)

        viewModel.setLowPowerMode(true)
        assertTrue(viewModel.isLowPowerMode.value)

        viewModel.setLowPowerMode(false)
        assertFalse(viewModel.isLowPowerMode.value)
    }

    @Test
    fun testCameraPermissionUpdates() = runTest {
        val viewModel = WebcamViewModel(app)

        assertFalse(viewModel.hasCameraPermission.value)
        viewModel.updateCameraPermission(true)
        assertTrue(viewModel.hasCameraPermission.value)

        viewModel.updateCameraPermission(false)
        assertFalse(viewModel.hasCameraPermission.value)
    }

    @Test
    fun testStartAndStopStreaming_withStreamManager() = runTest {
        val fakeManager = FakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        // Initially not streaming
        assertFalse(viewModel.isStreaming.value)
        assertEquals(0.0, viewModel.fps.value, 0.001)

        // Start streaming
        viewModel.startStreaming(lifecycleOwner)
        testScheduler.advanceTimeBy(500)

        assertTrue(viewModel.isStreaming.value)
        assertEquals(1, fakeManager.startStreamingCallCount)
        assertEquals(29.8, viewModel.fps.value, 0.001)
        assertTrue(viewModel.connectionState.value is WebcamConnectionState.Streaming)
        assertEquals("10.2.0.2", fakeManager.lastTargetHost)
        assertEquals(53317, fakeManager.lastTargetPort)
        assertEquals(WebcamResolution.RES_720P, fakeManager.lastResolution)
        assertEquals(WebcamLens.BACK, fakeManager.lastLens)

        // Stopping streaming
        viewModel.stopStreaming()
        testScheduler.advanceTimeBy(500)

        assertFalse(viewModel.isStreaming.value)
        assertEquals(1, fakeManager.stopStreamingCallCount)
        assertEquals(0.0, viewModel.fps.value, 0.001)
        assertTrue(viewModel.connectionState.value is WebcamConnectionState.Disconnected)
    }

    @Test
    fun testDuplicateStartStreaming_isIdempotent() = runTest {
        val fakeManager = FakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        viewModel.startStreaming(lifecycleOwner)
        testScheduler.advanceTimeBy(500)
        assertTrue(viewModel.isStreaming.value)
        assertEquals(1, fakeManager.startStreamingCallCount)

        // Duplicate start should be ignored
        viewModel.startStreaming(lifecycleOwner)
        testScheduler.advanceTimeBy(500)
        assertTrue(viewModel.isStreaming.value)
        assertEquals(1, fakeManager.startStreamingCallCount)

        viewModel.stopStreaming()
        testScheduler.advanceTimeBy(500)
        assertFalse(viewModel.isStreaming.value)
    }

    @Test
    fun testDynamicResolutionAndLensSwitch_whileStreaming() = runTest {
        val fakeManager = FakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        viewModel.startStreaming(lifecycleOwner)
        testScheduler.advanceTimeBy(500)
        assertTrue(viewModel.isStreaming.value)
        assertEquals(1, fakeManager.startStreamingCallCount)

        // Switch resolution while streaming
        viewModel.setResolution(WebcamResolution.RES_1080P, lifecycleOwner)
        testScheduler.advanceTimeBy(500)
        assertEquals(WebcamResolution.RES_1080P, viewModel.selectedResolution.value)
        assertEquals(WebcamResolution.RES_1080P, fakeManager.lastResolution)
        assertEquals(1, fakeManager.stopStreamingCallCount)
        assertEquals(2, fakeManager.startStreamingCallCount)
        assertTrue(viewModel.isStreaming.value)

        // Switch lens while streaming
        viewModel.setLens(WebcamLens.FRONT, lifecycleOwner)
        testScheduler.advanceTimeBy(500)
        assertEquals(WebcamLens.FRONT, viewModel.selectedLens.value)
        assertEquals(WebcamLens.FRONT, fakeManager.lastLens)
        assertEquals(2, fakeManager.stopStreamingCallCount)
        assertEquals(3, fakeManager.startStreamingCallCount)
        assertTrue(viewModel.isStreaming.value)

        viewModel.stopStreaming()
        testScheduler.advanceTimeBy(500)
        assertFalse(viewModel.isStreaming.value)
    }

    @Test
    fun testStreamManagerError_updatesViewModelStreamingState() = runTest {
        val fakeManager = FakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        viewModel.startStreaming(lifecycleOwner)
        testScheduler.advanceTimeBy(500)
        assertTrue(viewModel.isStreaming.value)

        // Simulate server rejection or network failure
        fakeManager.simulateError("Server rejected stream: HTTP 403")
        testScheduler.advanceTimeBy(500)

        assertFalse(viewModel.isStreaming.value)
        assertTrue(viewModel.connectionState.value is WebcamConnectionState.Error)
        assertEquals("Server rejected stream: HTTP 403", (viewModel.connectionState.value as WebcamConnectionState.Error).message)
    }

    @Test
    fun testWebcamScreenQuadrupleAndStatusMapping() {
        val errorState = WebcamConnectionState.Error("Network failure")
        assertEquals("Network failure", errorState.message)

        val streamingState = WebcamConnectionState.Streaming
        assertNotNull(streamingState)

        val connectingState = WebcamConnectionState.Connecting
        assertNotNull(connectingState)

        val disconnectedState = WebcamConnectionState.Disconnected
        assertNotNull(disconnectedState)

        val idleState = WebcamConnectionState.Idle
        assertNotNull(idleState)
    }
}
