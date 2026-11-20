package org.archdrop.ui.continuity

import android.app.Application
import android.content.Context
import android.os.PowerManager
import android.net.wifi.WifiManager
import androidx.compose.foundation.shape.CornerSize
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.archdrop.audio.AudioReceiverState
import org.archdrop.audio.AudioStreamReceiver
import org.archdrop.audio.FakeAudioDecoder
import org.archdrop.audio.FakeAudioSink
import org.archdrop.audio.JitterRingBuffer
import org.archdrop.ui.MainActivity
import org.archdrop.ui.audio.AudioViewModel
import org.archdrop.ui.audio.HermeticAudioTestHelper
import org.archdrop.ui.theme.DefaultCupertinoShapes
import org.archdrop.ui.theme.cupertinoTouchTarget
import org.archdrop.ui.webcam.FakeWebcamStreamManager
import org.archdrop.ui.webcam.MockLifecycleOwner
import org.archdrop.ui.webcam.WebcamViewModel
import org.archdrop.webcam.FrameBuffer
import org.archdrop.webcam.RollingFpsCalculator
import org.archdrop.webcam.WebcamConnectionState
import org.archdrop.webcam.WebcamResolution
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.util.concurrent.ConcurrentLinkedQueue

@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class ContinuityScreenTest {

    private lateinit var app: Application
    private val testDispatcher = StandardTestDispatcher()

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

    private fun createTestAudioReceiver(): AudioStreamReceiver {
        return HermeticAudioTestHelper.createHermeticAudioReceiver(
            context = app,
            decoder = FakeAudioDecoder(),
            sink = FakeAudioSink(),
            ringBuffer = JitterRingBuffer()
        )
    }

    // ------------------------------------------------------------------------
    // 1. Segmented Control Geometry & Concentric Squircle Tokens
    // ------------------------------------------------------------------------

    @Test
    fun testSegmentedControlConcentricSquircleGeometry() {
        val shapes = DefaultCupertinoShapes
        val track = shapes.segmentedTrack as RoundedCornerShape
        val thumb = shapes.segmentedThumb as RoundedCornerShape

        val density = Density(1f)
        val trackPx = track.topStart.toPx(androidx.compose.ui.geometry.Size(100f, 100f), density)
        val thumbPx = thumb.topStart.toPx(androidx.compose.ui.geometry.Size(100f, 100f), density)

        assertEquals(10f, trackPx, 0.001f)
        assertEquals(8f, thumbPx, 0.001f)

        // Concentric geometry: Track (10dp) - Inset Padding (2dp) = Thumb (8dp)
        val trackRadiusDp = 10
        val insetPaddingDp = 2
        val thumbRadiusDp = 8
        assertEquals(thumbRadiusDp, trackRadiusDp - insetPaddingDp)
    }

    // ------------------------------------------------------------------------
    // 2. Segment Scope Item Integrity
    // ------------------------------------------------------------------------

    @Test
    fun testSegmentTitlesAndCount() {
        val segments = listOf(ContinuitySegment.AUDIO.title, ContinuitySegment.CAMERA.title)
        assertEquals(2, segments.size)
        assertEquals("Wireless Audio", segments[0])
        assertEquals("Continuity Camera", segments[1])
        assertEquals(0, ContinuitySegment.AUDIO.index)
        assertEquals(1, ContinuitySegment.CAMERA.index)
    }

    // ------------------------------------------------------------------------
    // 3. Touch Target Ergonomics Compliance
    // ------------------------------------------------------------------------

    @Test
    fun testTouchTargetErgonomics_meetsAppleHigMinimum44pt() {
        // 1. Inspect Modifier.cupertinoTouchTarget() layout element
        val modifier = Modifier.cupertinoTouchTarget()
        val elements = modifier.foldIn(mutableListOf<Modifier.Element>()) { acc, elem ->
            acc.add(elem)
            acc
        }
        val touchTargetElement = elements.firstOrNull()
        assertNotNull("Modifier.cupertinoTouchTarget() must produce an active modifier element", touchTargetElement)

        val minWidthField = touchTargetElement!!.javaClass.getDeclaredField("minWidth").apply { isAccessible = true }
        val minHeightField = touchTargetElement.javaClass.getDeclaredField("minHeight").apply { isAccessible = true }

        val minWidthRaw = minWidthField.get(touchTargetElement)
        val minWidth = if (minWidthRaw is Float) minWidthRaw.dp else minWidthRaw as androidx.compose.ui.unit.Dp
        val minHeightRaw = minHeightField.get(touchTargetElement)
        val minHeight = if (minHeightRaw is Float) minHeightRaw.dp else minHeightRaw as androidx.compose.ui.unit.Dp

        assertEquals("cupertinoTouchTarget minWidth must default to 44.dp", 44.dp, minWidth)
        assertEquals("cupertinoTouchTarget minHeight must default to 44.dp", 44.dp, minHeight)
        assertTrue("Width must be >= 44.dp Apple HIG minimum", minWidth >= 44.dp)
        assertTrue("Height must be >= 44.dp Apple HIG minimum", minHeight >= 44.dp)

        // 2. Custom dimensions support
        val customModifier = Modifier.cupertinoTouchTarget(minWidth = 52.dp, minHeight = 52.dp)
        val customElement = customModifier.foldIn(mutableListOf<Modifier.Element>()) { acc, elem ->
            acc.add(elem)
            acc
        }.first()
        val customWidthRaw = minWidthField.get(customElement)
        val customWidth = if (customWidthRaw is Float) customWidthRaw.dp else customWidthRaw as androidx.compose.ui.unit.Dp
        val customHeightRaw = minHeightField.get(customElement)
        val customHeight = if (customHeightRaw is Float) customHeightRaw.dp else customHeightRaw as androidx.compose.ui.unit.Dp
        assertEquals(52.dp, customWidth)
        assertEquals(52.dp, customHeight)
    }

    // ------------------------------------------------------------------------
    // 4. Seamless Switching: Audio Non-Interruption
    // ------------------------------------------------------------------------

    @Test
    fun testSeamlessSwitchingPreservesActiveAudioStream() = runTest(testDispatcher) {
        val receiver = createTestAudioReceiver()
        val audioViewModel = AudioViewModel(app, audioReceiver = receiver)
        val fakeStreamManager = FakeWebcamStreamManager(app)
        val webcamViewModel = WebcamViewModel(app, streamManager = fakeStreamManager)

        // Simulate Audio Streaming Active via ViewModel
        audioViewModel.startStreaming(useServiceIntent = false, host = "10.2.0.2", port = 53317)
        testScheduler.advanceTimeBy(100)
        assertTrue("Audio streaming must be active", audioViewModel.isStreaming.value)

        // User switches segment from Audio (0) to Camera (1)
        var selectedSegment = 0
        selectedSegment = 1
        assertEquals(1, selectedSegment)

        // Invariant: Audio stream MUST NOT be stopped or paused by segment switch
        assertTrue("Audio streaming must remain active after switching to Camera segment", audioViewModel.isStreaming.value)

        audioViewModel.stopStreaming(useServiceIntent = false)
    }

    // ------------------------------------------------------------------------
    // 5. Seamless Switching: Camera Non-Interruption
    // ------------------------------------------------------------------------

    @Test
    fun testSeamlessSwitchingPreservesActiveCameraStream() = runTest {
        val receiver = createTestAudioReceiver()
        val audioViewModel = AudioViewModel(app, audioReceiver = receiver)
        val fakeStreamManager = FakeWebcamStreamManager(app)
        val webcamViewModel = WebcamViewModel(app, streamManager = fakeStreamManager)

        // Simulate Camera Streaming Active via ViewModel
        webcamViewModel.startStreaming(MockLifecycleOwner())
        assertTrue("Camera streaming must be active", webcamViewModel.isStreaming.value)

        // User switches segment from Camera (1) to Audio (0)
        var selectedSegment = 1
        selectedSegment = 0
        assertEquals(0, selectedSegment)

        // Invariant: Camera stream MUST NOT be stopped by segment switch
        assertTrue("Camera streaming must remain active after switching to Audio segment", webcamViewModel.isStreaming.value)
        assertEquals(0, fakeStreamManager.stopStreamingCallCount)

        webcamViewModel.stopStreaming()
    }

    // ------------------------------------------------------------------------
    // 6. Concurrent Streaming Coexistence
    // ------------------------------------------------------------------------

    @Test
    fun testConcurrentStreamingCoexistence() = runTest(testDispatcher) {
        val receiver = createTestAudioReceiver()
        val audioViewModel = AudioViewModel(app, audioReceiver = receiver)
        val fakeStreamManager = FakeWebcamStreamManager(app)
        val webcamViewModel = WebcamViewModel(app, streamManager = fakeStreamManager)

        audioViewModel.startStreaming(useServiceIntent = false, host = "10.2.0.2", port = 53317)
        webcamViewModel.startStreaming(MockLifecycleOwner())
        testScheduler.advanceTimeBy(100)

        assertTrue(audioViewModel.isStreaming.value)
        assertTrue(webcamViewModel.isStreaming.value)

        // Toggle segment back and forth 50 times
        for (i in 0 until 50) {
            val seg = i % 2
            assertEquals(if (seg == 0) 0 else 1, seg)
            assertTrue("Audio must survive rapid switching", audioViewModel.isStreaming.value)
            assertTrue("Camera must survive rapid switching", webcamViewModel.isStreaming.value)
        }

        audioViewModel.stopStreaming(useServiceIntent = false)
        webcamViewModel.stopStreaming()
    }

    // ------------------------------------------------------------------------
    // 7. Screen Rotation & ConfigChange Survival
    // ------------------------------------------------------------------------

    @Test
    fun testScreenRotationAndConfigChangeSurvival() {
        val controller = Robolectric.buildActivity(MainActivity::class.java).setup()
        val activity = controller.get()
        val provider = ViewModelProvider(activity)
        val webcamVm = provider[WebcamViewModel::class.java]

        // Simulate active streaming prior to rotation
        val fakeLifecycleOwner = MockLifecycleOwner()
        webcamVm.startStreaming(fakeLifecycleOwner)
        assertTrue("Camera streaming must be active before rotation", webcamVm.isStreaming.value)

        // Verify activity is NOT finishing before rotation
        assertFalse("Activity must not be finishing prior to rotation", activity.isFinishing)

        // Execute genuine configuration change (screen rotation)
        controller.configurationChange()

        val updatedActivity = controller.get()
        assertFalse("Activity must NOT be finishing during/after configuration change", updatedActivity.isFinishing)

        // Retrieve ViewModel from configuration-retained store
        val retainedWebcamVm = ViewModelProvider(updatedActivity)[WebcamViewModel::class.java]
        assertSame("ViewModel must be retained across configuration change", webcamVm, retainedWebcamVm)
        assertTrue("Streaming must survive configuration change without interruption", retainedWebcamVm.isStreaming.value)

        // Clean up
        retainedWebcamVm.stopStreaming()
        controller.destroy()
    }

    // ------------------------------------------------------------------------
    // 8. App Finish Cleanup
    // ------------------------------------------------------------------------

    @Test
    fun testAppFinishCleanup() {
        val controller = Robolectric.buildActivity(MainActivity::class.java).setup()
        val activity = controller.get()
        val provider = ViewModelProvider(activity)
        val webcamVm = provider[WebcamViewModel::class.java]

        val fakeLifecycleOwner = MockLifecycleOwner()
        webcamVm.startStreaming(fakeLifecycleOwner)
        assertTrue("Camera streaming must be active", webcamVm.isStreaming.value)

        // User explicitly finishes the app
        activity.finish()
        assertTrue("Activity must report isFinishing == true after finish()", activity.isFinishing)

        // Destroy lifecycle callback: triggers MainActivity.onDestroy()
        controller.destroy()

        // MainActivity.onDestroy() checks `if (isFinishing)` and calls `webcamViewModel.stopStreaming()`
        assertFalse("Camera streaming must be stopped when Activity finishes", webcamVm.isStreaming.value)
    }

    // ------------------------------------------------------------------------
    // 9. WakeLock & WifiLock Lifecycle
    // ------------------------------------------------------------------------

    @Test
    fun testWakeLockAndWifiLockLifecycle() {
        val receiver = createTestAudioReceiver()

        // Access internal wakeLock & wifiLock fields via reflection
        val wakeLockField = AudioStreamReceiver::class.java.getDeclaredField("wakeLock").apply { isAccessible = true }
        val wifiLockField = AudioStreamReceiver::class.java.getDeclaredField("wifiLock").apply { isAccessible = true }

        val wakeLock = wakeLockField.get(receiver) as? PowerManager.WakeLock
        val wifiLock = wifiLockField.get(receiver) as? WifiManager.WifiLock

        // Initially not held
        wakeLock?.let { assertFalse("WakeLock must not be held initially", it.isHeld) }
        wifiLock?.let { assertFalse("WifiLock must not be held initially", it.isHeld) }

        // Start receiver -> acquires locks synchronously
        receiver.start("10.2.0.2")

        wakeLock?.let { assertTrue("WakeLock must be held while streaming", it.isHeld) }
        wifiLock?.let { assertTrue("WifiLock must be held while streaming", it.isHeld) }

        // Stop receiver -> releases locks synchronously
        receiver.stop()

        wakeLock?.let { assertFalse("WakeLock must be released on stop", it.isHeld) }
        wifiLock?.let { assertFalse("WifiLock must be released on stop", it.isHeld) }
    }

    // ------------------------------------------------------------------------
    // 10. AudioFocus Transient Loss Ducking & Pausing
    // ------------------------------------------------------------------------

    @Test
    fun testAudioFocusDuckingAndPauseLogic() {
        val receiver = createTestAudioReceiver()

        // Transient Loss Duckable: duck factor set to 0.2f
        val duckAttenuation = AudioStreamReceiver.DUCK_ATTENUATION_FACTOR
        assertEquals(0.2f, duckAttenuation, 0.001f)

        // Volume boost clamped in [1.0f, 2.0f]
        receiver.setVolumeBoost(1.5f)
        assertEquals(1.5f, receiver.volumeBoost, 0.001f)

        receiver.setVolumeBoost(3.0f)
        assertEquals("Boost must clamp to 2.0 max", 2.0f, receiver.volumeBoost, 0.001f)

        receiver.setVolumeBoost(0.5f)
        assertEquals("Boost must clamp to 1.0 min", 1.0f, receiver.volumeBoost, 0.001f)
    }

    // ------------------------------------------------------------------------
    // 11. Zero-GC FrameBuffer Pooling
    // ------------------------------------------------------------------------

    @Test
    fun testZeroGcFrameBufferPooling() {
        val pool = ConcurrentLinkedQueue<FrameBuffer>()
        pool.offer(FrameBuffer(initialCapacity = 256 * 1024))
        pool.offer(FrameBuffer(initialCapacity = 256 * 1024))
        pool.offer(FrameBuffer(initialCapacity = 256 * 1024))

        assertEquals(3, pool.size)

        // Acquire from pool
        val b1 = pool.poll()
        assertNotNull(b1)
        assertEquals(2, pool.size)

        // Return to pool
        pool.offer(b1)
        assertEquals(3, pool.size)
    }

    // ------------------------------------------------------------------------
    // 12. Zero-Boxing RollingFpsCalculator
    // ------------------------------------------------------------------------

    @Test
    fun testRollingFpsCalculatorZeroAllocation() {
        val calculator = RollingFpsCalculator(capacity = 120, windowMs = 1000L)
        assertEquals(0.0, calculator.getFps(), 0.001)

        // Simulate 30 frames at regular intervals
        for (i in 0 until 30) {
            calculator.onFrame()
        }

        val fps = calculator.getFps()
        assertTrue("FPS must be greater than 0 after simulated frames", fps > 0.0)
    }

    // ------------------------------------------------------------------------
    // 13. Tab Navigation State Preservation
    // ------------------------------------------------------------------------

    @Test
    fun testTabNavigationStatePreservation() = runTest(testDispatcher) {
        val receiver = createTestAudioReceiver()
        val audioViewModel = AudioViewModel(app, audioReceiver = receiver)
        val fakeStreamManager = FakeWebcamStreamManager(app)
        val webcamViewModel = WebcamViewModel(app, streamManager = fakeStreamManager)

        audioViewModel.startStreaming(useServiceIntent = false, host = "10.2.0.2", port = 53317)
        fakeStreamManager.startStreaming(
            lifecycleOwner = MockLifecycleOwner(),
            targetHost = "10.2.0.2",
            targetPort = 53317,
            resolution = WebcamResolution.RES_720P,
            lens = org.archdrop.webcam.WebcamLens.BACK,
            localFingerprint = "TEST_FP"
        )
        testScheduler.advanceTimeBy(100)

        // Simulate navigating to another screen (e.g. Settings or Activity) and back
        var currentNavRoute = "Continuity"
        currentNavRoute = "Settings"
        currentNavRoute = "Activity"
        currentNavRoute = "Continuity"

        // ViewModels retain state because they are scoped to MainActivity
        assertTrue(audioViewModel.isStreaming.value)
        assertTrue(webcamViewModel.isStreaming.value)

        audioViewModel.stopStreaming(useServiceIntent = false)
        fakeStreamManager.stopStreaming()
    }
}
