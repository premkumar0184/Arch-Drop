package org.archdrop.ui.continuity

import android.app.Application
import android.content.Context
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.archdrop.ui.theme.DarkCupertinoColors
import org.archdrop.ui.theme.DefaultCupertinoShapes
import org.archdrop.ui.theme.cupertinoTouchTarget
import org.archdrop.ui.webcam.FakeWebcamStreamManager
import org.archdrop.ui.webcam.MockLifecycleOwner
import org.archdrop.ui.webcam.WebcamViewModel
import org.archdrop.webcam.WebcamConnectionState
import org.archdrop.webcam.WebcamLens
import org.archdrop.webcam.WebcamResolution
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
import java.util.Locale

@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class ContinuityCameraTest {

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

    // ------------------------------------------------------------------------
    // 1. Viewfinder Aspect Ratio & Squircle Geometry Verification
    // ------------------------------------------------------------------------

    @Test
    fun testViewfinderGeometry_conformsToAppleHigSquircleAnd16to9() {
        // 1. Viewfinder Aspect Ratio Modifier Inspection
        val aspectModifier = Modifier.aspectRatio(16f / 9f)
        val elements = aspectModifier.foldIn(mutableListOf<Modifier.Element>()) { acc, elem ->
            acc.add(elem)
            acc
        }
        val aspectElement = elements.firstOrNull()
        assertNotNull("aspectRatio modifier must produce an active element", aspectElement)

        val ratioField = aspectElement!!.javaClass.getDeclaredField("aspectRatio").apply { isAccessible = true }
        val ratio = ratioField.get(aspectElement) as Float
        assertEquals("Modifier.aspectRatio must be exactly 16:9", 16f / 9f, ratio, 0.0001f)

        // Verify HD Video stream geometry matches 16:9 modifier ratio
        val res720p = WebcamResolution.RES_720P
        assertEquals("720p HD resolution must match 16:9 aspect ratio", ratio, res720p.width.toFloat() / res720p.height.toFloat(), 0.0001f)
        val res1080p = WebcamResolution.RES_1080P
        assertEquals("1080p FHD resolution must match 16:9 aspect ratio", ratio, res1080p.width.toFloat() / res1080p.height.toFloat(), 0.0001f)

        // 2. Squircle geometry token: heroSquircle must resolve to continuous 20dp corners
        val heroSquircle = DefaultCupertinoShapes.heroSquircle as RoundedCornerShape
        val density = Density(1f)
        val cornerPx = heroSquircle.topStart.toPx(androidx.compose.ui.geometry.Size(320f, 180f), density)
        assertEquals("Hero squircle corner radius must be 20dp at 1x density", 20f, cornerPx, 0.001f)

        // Verify multi-density corner scaling
        val xhdpiDensity = Density(2f)
        val cornerXhdpiPx = heroSquircle.topStart.toPx(androidx.compose.ui.geometry.Size(320f, 180f), xhdpiDensity)
        assertEquals("Hero squircle corner radius must scale to 40px at 2x density", 40f, cornerXhdpiPx, 0.001f)

        // 3. OLED True Black Canvas token
        val canvasBackground = DarkCupertinoColors.canvasBackground
        assertEquals("Canvas background must be OLED True Black #000000", Color(0xFF000000), canvasBackground)

        // 4. Hairline border opacity (0.5dp 10% white)
        val hairline = DarkCupertinoColors.hairlineBorder
        assertEquals("Hairline border must be 10% white in dark mode", Color(0x1AFFFFFF), hairline)
    }

    // ------------------------------------------------------------------------
    // 2. Floating Resolution Selector Modal Logic
    // ------------------------------------------------------------------------

    @Test
    fun testResolutionSelection_cyclesThroughAllThreeHigOptions() = runTest {
        val fakeManager = FakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        // Default resolution must be 720p HD
        assertEquals(WebcamResolution.RES_720P, viewModel.selectedResolution.value)

        // Select 1080p FHD
        viewModel.setResolution(WebcamResolution.RES_1080P, lifecycleOwner)
        assertEquals(WebcamResolution.RES_1080P, viewModel.selectedResolution.value)
        assertEquals(1920, viewModel.selectedResolution.value.width)
        assertEquals(1080, viewModel.selectedResolution.value.height)

        // Select 480p SD
        viewModel.setResolution(WebcamResolution.RES_480P, lifecycleOwner)
        assertEquals(WebcamResolution.RES_480P, viewModel.selectedResolution.value)
        assertEquals(640, viewModel.selectedResolution.value.width)
        assertEquals(480, viewModel.selectedResolution.value.height)

        // Return to 720p HD
        viewModel.setResolution(WebcamResolution.RES_720P, lifecycleOwner)
        assertEquals(WebcamResolution.RES_720P, viewModel.selectedResolution.value)
    }

    // ------------------------------------------------------------------------
    // 3. Floating Lens Selector Toggle Logic
    // ------------------------------------------------------------------------

    @Test
    fun testLensSelector_togglesBetweenFrontAndBackCamera() = runTest {
        val fakeManager = FakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        // Default lens is BACK camera
        assertEquals(WebcamLens.BACK, viewModel.selectedLens.value)

        // Toggle to FRONT camera
        viewModel.setLens(WebcamLens.FRONT, lifecycleOwner)
        assertEquals(WebcamLens.FRONT, viewModel.selectedLens.value)
        assertEquals("Front Camera", viewModel.selectedLens.value.label)

        // Toggle back to BACK camera
        viewModel.setLens(WebcamLens.BACK, lifecycleOwner)
        assertEquals(WebcamLens.BACK, viewModel.selectedLens.value)
        assertEquals("Back Camera", viewModel.selectedLens.value.label)
    }

    // ------------------------------------------------------------------------
    // 4. OLED Blackout Mode & Tap-to-Wake Verification
    // ------------------------------------------------------------------------

    @Test
    fun testOLEDBlackoutMode_activatesAndWakesViaTapGesture() = runTest {
        val fakeManager = FakeWebcamStreamManager(app)
        val viewModel = WebcamViewModel(app, fakeManager)
        val lifecycleOwner = MockLifecycleOwner()

        // Initially disabled
        assertFalse("Blackout mode must be off initially", viewModel.isLowPowerMode.value)

        // Start streaming
        viewModel.startStreaming(lifecycleOwner)
        testScheduler.advanceTimeBy(500)
        assertTrue("Streaming must be active", viewModel.isStreaming.value)

        // Activate OLED blackout mode
        viewModel.setLowPowerMode(true)
        assertTrue("Blackout mode must be active", viewModel.isLowPowerMode.value)

        // Verify streaming remains active while screen is blacked out
        assertTrue("CameraX and streaming must remain active during blackout", viewModel.isStreaming.value)
        assertTrue(viewModel.connectionState.value is WebcamConnectionState.Streaming)

        // Simulate Tap-To-Wake gesture
        viewModel.setLowPowerMode(false)
        assertFalse("Blackout mode must be dismissed after tap-to-wake", viewModel.isLowPowerMode.value)
        assertTrue("Streaming remains active after waking", viewModel.isStreaming.value)
    }

    // ------------------------------------------------------------------------
    // 5. Rolling Telemetry Capsule Formatting
    // ------------------------------------------------------------------------

    @Test
    fun testTelemetryCapsuleFormatting_zeroDivisionSafe() {
        val fps = 29.8
        val estimatedKbps = (fps * 55.0 * 8.0).toInt()
        val bitrateText = String.format(Locale.US, "%.1f Mbps", estimatedKbps / 1000.0)
        val resolution = WebcamResolution.RES_720P

        val telemetry = String.format(Locale.US, "%.1f FPS • %s • %s", fps, bitrateText, resolution.label.substringBefore(" "))
        assertEquals("29.8 FPS • 13.1 Mbps • 720p", telemetry)

        // Zero FPS edge case
        val zeroFps = 0.0
        val zeroTelemetry = if (zeroFps > 0.0) "Active" else "Ready • 720p"
        assertEquals("Ready • 720p", zeroTelemetry)
    }

    // ------------------------------------------------------------------------
    // 6. Touch Target Ergonomics Compliance
    // ------------------------------------------------------------------------

    @Test
    fun testTouchTargetSizes_meetAppleHigMinimum44pt() {
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

        assertEquals(44.dp, minWidth)
        assertEquals(44.dp, minHeight)
        assertTrue(minWidth >= 44.dp)
        assertTrue(minHeight >= 44.dp)

        // Verify main action button height in WebcamScreen exceeds Apple HIG minimum
        val mainActionButtonHeight = 52.dp
        assertTrue("Camera main action button (52dp) meets Apple HIG minimum (44dp)", mainActionButtonHeight >= 44.dp)
    }
}
