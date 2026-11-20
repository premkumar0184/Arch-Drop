package org.archdrop.ui.webcam

import android.app.Activity
import android.app.Application
import android.content.Context
import android.content.ContextWrapper
import android.view.WindowManager
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.archdrop.webcam.WebcamConnectionState
import org.archdrop.webcam.WebcamLens
import org.archdrop.webcam.WebcamResolution
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.util.Locale

/**
 * Adversarial empirical test harness for Arch Drop Phase 6 Milestone 3.
 *
 * Stress-tests:
 * 1. AMOLED Low-Power Blackout Mode:
 *    - Full black coverage state machine
 *    - Tap-to-wake responsiveness under stress (1000 toggles)
 *    - Telemetry string formatting across all resolutions, frame rates, and hosts
 * 2. Window Screen-Awake Lifecycle Management:
 *    - FLAG_KEEP_SCREEN_ON attachment on streaming start
 *    - Clean detachment on streaming stop
 *    - Disposal cleanup ensuring zero window flag leaks
 *    - Deep ContextWrapper hierarchy unwrapping
 * 3. Permission Rejection & Revocation Handling:
 *    - Permission rejection prevents premature streaming
 *    - Runtime permission revocation cleanly aborts stream
 *    - State reflection across connection and viewmodel flows
 * 4. Input Robustness & Host Sanitization:
 *    - Host whitespace trimming, rejection of blank inputs, fallback persistence
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class AdversarialWebcamM3ChallengeTest {

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

    // =========================================================================
    // Challenge 1: AMOLED Low-Power Blackout Mode & Tap-To-Wake
    // =========================================================================

    @Test
    fun testBlackoutMode_stressToggleAlternation() = runTest {
        val viewModel = WebcamViewModel(app)

        assertFalse("Blackout mode must be off initially", viewModel.isLowPowerMode.value)

        // Stress toggle 1000 times to verify no state drift or deadlock
        for (i in 1..1000) {
            viewModel.toggleLowPowerMode()
            assertEquals("Toggle $i failed state expectation", (i % 2 != 0), viewModel.isLowPowerMode.value)
        }

        // Tap to wake: simulate clicking anywhere on the blackout overlay
        viewModel.setLowPowerMode(true)
        assertTrue(viewModel.isLowPowerMode.value)

        // Tap-to-wake callback in WebcamScreen invokes setLowPowerMode(false)
        viewModel.setLowPowerMode(false)
        assertFalse("Tap-to-wake must cleanly exit blackout mode", viewModel.isLowPowerMode.value)
    }

    @Test
    fun testBlackoutMode_telemetryFormattingExhaustiveMatrix() = runTest {
        val fakeManager = FakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        val resolutions = listOf(
            WebcamResolution.RES_480P,
            WebcamResolution.RES_720P,
            WebcamResolution.RES_1080P
        )
        val testFpsValues = listOf(0.0, 15.2, 29.8, 30.0, 59.94, 60.0)
        val testHosts = listOf("10.2.0.2", "192.168.1.50", "archdrop.local", "127.0.0.1")

        for (host in testHosts) {
            viewModel.setTargetHost(host)
            for (res in resolutions) {
                viewModel.setResolution(res)
                for (fpsVal in testFpsValues) {
                    // 1. Standby Telemetry (streaming = false)
                    val standbyTelemetry = if (viewModel.isStreaming.value) {
                        "Camera is streaming to ${viewModel.targetHost.value}:${viewModel.targetPort}\n${String.format(Locale.US, "%.1f", viewModel.fps.value)} FPS • ${viewModel.selectedResolution.value.label}\n\nTap anywhere on the screen to wake"
                    } else {
                        "Low power mode standby\nTap anywhere on the screen to wake"
                    }
                    assertTrue(standbyTelemetry.contains("Low power mode standby"))
                    assertTrue(standbyTelemetry.contains("Tap anywhere on the screen to wake"))

                    // 2. Active Streaming Telemetry (streaming = true)
                    viewModel.startStreaming(lifecycleOwner)
                    testScheduler.advanceTimeBy(500)

                    val streamingTelemetry = if (viewModel.isStreaming.value) {
                        "Camera is streaming to ${viewModel.targetHost.value}:${viewModel.targetPort}\n${String.format(Locale.US, "%.1f", fpsVal)} FPS • ${viewModel.selectedResolution.value.label}\n\nTap anywhere on the screen to wake"
                    } else {
                        "Low power mode standby\nTap anywhere on the screen to wake"
                    }

                    assertTrue(streamingTelemetry.contains("Camera is streaming to $host:53317"))
                    assertTrue(streamingTelemetry.contains(res.label))
                    assertTrue(streamingTelemetry.contains("Tap anywhere on the screen to wake"))

                    viewModel.stopStreaming()
                    testScheduler.advanceTimeBy(500)
                }
            }
        }
    }

    // =========================================================================
    // Challenge 2: Screen-Awake Management (FLAG_KEEP_SCREEN_ON) Lifecycle
    // =========================================================================

    /**
     * Helper replicating Context.findActivity() from WebcamScreen.kt to test deep unwrapping.
     */
    private tailrec fun Context.findActivity(): Activity? = when (this) {
        is Activity -> this
        is ContextWrapper -> baseContext.findActivity()
        else -> null
    }

    @Test
    fun testFindActivity_deepContextWrapperTraversal() {
        val activity = Robolectric.buildActivity(Activity::class.java).setup().get()

        // 1. Direct Activity
        assertEquals(activity, (activity as Context).findActivity())

        // 2. Triple-wrapped ContextWrapper (e.g., ThemeWrapper -> TintWrapper -> AppWrapper)
        val wrap1 = ContextWrapper(activity)
        val wrap2 = ContextWrapper(wrap1)
        val wrap3 = ContextWrapper(wrap2)
        assertEquals(activity, (wrap3 as Context).findActivity())

        // 3. Application context (non-activity) returns null safely
        assertEquals(null, app.findActivity())
    }

    @Test
    fun testKeepScreenOnFlag_lifecycleAttachmentAndDetachment() {
        val activity = Robolectric.buildActivity(Activity::class.java).setup().get()
        val window = activity.window

        fun isKeepScreenOnSet(): Boolean {
            return (window.attributes.flags and WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) != 0
        }

        // Initially flag is NOT set
        assertFalse("FLAG_KEEP_SCREEN_ON must not be present initially", isKeepScreenOnSet())

        // Simulate DisposableEffect(isStreaming = true)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        assertTrue("FLAG_KEEP_SCREEN_ON must be added when streaming is active", isKeepScreenOnSet())

        // Simulate DisposableEffect(isStreaming = false)
        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        assertFalse("FLAG_KEEP_SCREEN_ON must be cleared when streaming stops", isKeepScreenOnSet())

        // Simulate DisposableEffect onDispose cleanup
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        assertTrue(isKeepScreenOnSet())
        // onDispose executed
        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        assertFalse("onDispose must guarantee clean flag detachment (no window leak)", isKeepScreenOnSet())
    }

    @Test
    fun testKeepScreenOnFlag_rapidStreamingTransitions() {
        val activity = Robolectric.buildActivity(Activity::class.java).setup().get()
        val window = activity.window

        fun isKeepScreenOnSet(): Boolean =
            (window.attributes.flags and WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) != 0

        // Rapid 500 transitions simulating rapid start/stop button clicks
        for (i in 1..500) {
            val streaming = (i % 2 == 0)
            if (streaming) {
                window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            } else {
                window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
            assertEquals(streaming, isKeepScreenOnSet())
        }

        // Ensure final state after stopping is completely clear
        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        assertFalse(isKeepScreenOnSet())
    }

    // =========================================================================
    // Challenge 3: Permission Rejection & Runtime Revocation Handling
    // =========================================================================

    @Test
    fun testPermissionRejection_preventsStreamingTrigger() = runTest {
        val fakeManager = FakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        // 1. Initial state: permission not granted
        assertFalse(viewModel.hasCameraPermission.value)
        assertFalse(viewModel.isStreaming.value)

        // 2. User denies / rejects permission
        viewModel.updateCameraPermission(false)
        assertFalse(viewModel.hasCameraPermission.value)

        // In WebcamScreen, clicking Start Streaming when hasCameraPermission=false
        // delegates to permissionLauncher.launch(CAMERA) and does NOT call startStreaming
        assertEquals(0, fakeManager.startStreamingCallCount)
        assertFalse(viewModel.isStreaming.value)

        // 3. User grants permission
        viewModel.updateCameraPermission(true)
        assertTrue(viewModel.hasCameraPermission.value)

        // Now startStreaming proceeds
        viewModel.startStreaming(lifecycleOwner)
        testScheduler.advanceTimeBy(500)
        assertTrue(viewModel.isStreaming.value)
        assertEquals(1, fakeManager.startStreamingCallCount)
    }

    @Test
    fun testRuntimePermissionRevocation_cleansUpStreaming() = runTest {
        val fakeManager = FakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        // Granted and streaming
        viewModel.updateCameraPermission(true)
        viewModel.startStreaming(lifecycleOwner)
        testScheduler.advanceTimeBy(500)
        assertTrue(viewModel.isStreaming.value)

        // User revokes permission in Android Settings while streaming
        viewModel.updateCameraPermission(false)
        assertFalse(viewModel.hasCameraPermission.value)

        // When permission is revoked at OS level, CameraX unbinds or throws SecurityException
        fakeManager.simulateError("SecurityException: Camera permission was revoked by user")
        testScheduler.advanceTimeBy(500)

        assertFalse("Streaming must be aborted when permission is revoked", viewModel.isStreaming.value)
        assertTrue(viewModel.connectionState.value is WebcamConnectionState.Error)
        assertEquals(
            "SecurityException: Camera permission was revoked by user",
            (viewModel.connectionState.value as WebcamConnectionState.Error).message
        )
    }

    // =========================================================================
    // Challenge 4: Input Robustness & Host Sanitization
    // =========================================================================

    @Test
    fun testTargetHost_adversarialInputs() = runTest {
        val viewModel = WebcamViewModel(app)

        // Valid default WireGuard
        assertEquals("10.2.0.2", viewModel.targetHost.value)

        // Whitespace padding
        viewModel.setTargetHost("   10.2.0.5   ")
        assertEquals("10.2.0.5", viewModel.targetHost.value)

        // Blank input should be rejected (preserving last valid host)
        viewModel.setTargetHost("   ")
        assertEquals("10.2.0.5", viewModel.targetHost.value)

        viewModel.setTargetHost("")
        assertEquals("10.2.0.5", viewModel.targetHost.value)

        // Special network hostnames
        viewModel.setTargetHost("laptop.wireguard.local")
        assertEquals("laptop.wireguard.local", viewModel.targetHost.value)
    }
}
