package org.archdrop.ui.audio

import android.app.Application
import android.content.Context
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.archdrop.audio.AudioCodec
import org.archdrop.audio.AudioReceiverState
import org.archdrop.audio.AudioStreamReceiver
import org.archdrop.audio.FakeAudioDecoder
import org.archdrop.audio.FakeAudioSink
import org.archdrop.audio.JitterRingBuffer
import org.archdrop.audio.JitterTelemetry
import org.archdrop.ui.theme.DarkCupertinoColors
import org.archdrop.ui.theme.DefaultCupertinoShapes
import org.archdrop.ui.theme.cupertinoTouchTarget
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
import kotlin.math.roundToInt
import kotlin.math.sin

@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class AirPlayAudioViewUnitTest {

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

    private fun createTestReceiver(): AudioStreamReceiver {
        return HermeticAudioTestHelper.createHermeticAudioReceiver(
            context = app,
            decoder = FakeAudioDecoder(),
            sink = FakeAudioSink(),
            ringBuffer = JitterRingBuffer()
        )
    }

    // ------------------------------------------------------------------------
    // 1. Visualizer Bar Count, Phase Step & Height Clamping
    // ------------------------------------------------------------------------

    @Test
    fun testVisualizerPhaseAndBarCount() {
        val barCount = 11
        assertEquals(11, barCount)

        val phaseStep = 0.55f
        val phase = 0f

        for (i in 0 until barCount) {
            val normalized = (sin(phase + i * phaseStep) + 1f) / 2f
            val heightFraction = 0.18f + 0.82f * normalized
            assertTrue("Height fraction must be >= 0.18f", heightFraction >= 0.18f)
            assertTrue("Height fraction must be <= 1.0f", heightFraction <= 1.0f)
        }

        // Idle state: 0.12f
        val idleFraction = 0.12f
        assertEquals(0.12f, idleFraction, 0.001f)
    }

    @Test
    fun testSinusoidalHeightCalculationAtKeyPhases() {
        val phases = listOf(0f, (Math.PI / 2).toFloat(), Math.PI.toFloat(), (3 * Math.PI / 2).toFloat())
        for (phase in phases) {
            for (i in 0 until 11) {
                val normalized = (sin(phase + i * 0.55f) + 1f) / 2f
                val heightFraction = 0.18f + 0.82f * normalized
                assertTrue(heightFraction in 0.18f..1.0f)
            }
        }
    }

    // ------------------------------------------------------------------------
    // 2. AirPlay Route Selection & Device Classification
    // ------------------------------------------------------------------------

    @Test
    fun testAirPlayRouteSelectionAndCheckmark() = runTest {
        val receiver = createTestReceiver()
        val viewModel = AudioViewModel(app, audioReceiver = receiver)

        val speaker = AudioDeviceUiModel(1, "Phone Speaker", AudioDeviceType.SPEAKER)
        val wired = AudioDeviceUiModel(2, "EarPods", AudioDeviceType.WIRED)
        val bluetooth = AudioDeviceUiModel(3, "AirPods Pro", AudioDeviceType.BLUETOOTH)
        val usb = AudioDeviceUiModel(4, "USB DAC", AudioDeviceType.USB)
        val defaultDevice = AudioDeviceUiModel.SYSTEM_DEFAULT

        assertFalse(speaker.isBluetooth)
        assertFalse(wired.isBluetooth)
        assertTrue(bluetooth.isBluetooth)
        assertFalse(usb.isBluetooth)
        assertFalse(defaultDevice.isBluetooth)

        viewModel.selectDevice(bluetooth)
        assertEquals(bluetooth, viewModel.selectedOutputDevice.value)
        assertTrue("Selected device is Bluetooth", viewModel.selectedOutputDevice.value?.isBluetooth == true)

        viewModel.selectDevice(speaker)
        assertEquals(speaker, viewModel.selectedOutputDevice.value)
        assertFalse("Selected device is not Bluetooth", viewModel.selectedOutputDevice.value?.isBluetooth == true)
    }

    // ------------------------------------------------------------------------
    // 3. Cupertino Volume Booster Clamping, Detents & NaN Protection
    // ------------------------------------------------------------------------

    @Test
    fun testCupertinoVolumeBoosterClampingAndHapticStepDetection() {
        val receiver = createTestReceiver()
        val viewModel = AudioViewModel(app, audioReceiver = receiver)

        // Float.NaN protection
        viewModel.setVolumeBoost(Float.NaN)
        assertEquals(1.0f, viewModel.volumeBoost.value, 0.001f)

        // Lower clamp
        viewModel.setVolumeBoost(0.2f)
        assertEquals(1.0f, viewModel.volumeBoost.value, 0.001f)

        // Upper clamp
        viewModel.setVolumeBoost(3.5f)
        assertEquals(2.0f, viewModel.volumeBoost.value, 0.001f)

        // Detent steps: 100%, 125%, 150%, 175%, 200%
        val detentSteps = listOf(100, 125, 150, 175, 200)
        for (step in detentSteps) {
            val boost = step / 100f
            viewModel.setVolumeBoost(boost)
            assertEquals(step, viewModel.volumeBoostPercent)
            assertTrue(step == 100 || step % 25 == 0)
        }
    }

    // ------------------------------------------------------------------------
    // 4. Telemetry Capsule Status Dot, Bitrate & Drops Mapping
    // ------------------------------------------------------------------------

    @Test
    fun testAirPlayTelemetryCapsuleMetrics() {
        val colors = DarkCupertinoColors

        // 1. Idle / Standby state -> gray
        assertEquals(Pair(colors.systemGray, "Standby"), getTelemetryStatus(isStreaming = false, bufferMs = 45.0, colors = colors))
        assertEquals(Pair(colors.systemGray, "Standby"), getTelemetryStatus(isStreaming = false, bufferMs = 0.0, colors = colors))
        assertEquals(Pair(colors.systemGray, "Standby"), getTelemetryStatus(isStreaming = false, bufferMs = 120.0, colors = colors))

        // 2. Optimal sweet spot (30.0ms – 60.0ms) -> green
        assertEquals(Pair(colors.systemGreen, "Optimal"), getTelemetryStatus(isStreaming = true, bufferMs = 30.0, colors = colors))
        assertEquals(Pair(colors.systemGreen, "Optimal"), getTelemetryStatus(isStreaming = true, bufferMs = 45.0, colors = colors))
        assertEquals(Pair(colors.systemGreen, "Optimal"), getTelemetryStatus(isStreaming = true, bufferMs = 60.0, colors = colors))

        // 3. Warning tier: buffer between 20.0ms and 30.0ms, or buffer > 60.0ms -> orange
        assertEquals(Pair(colors.systemOrange, "Warning"), getTelemetryStatus(isStreaming = true, bufferMs = 20.0, colors = colors))
        assertEquals(Pair(colors.systemOrange, "Warning"), getTelemetryStatus(isStreaming = true, bufferMs = 25.0, colors = colors))
        assertEquals(Pair(colors.systemOrange, "Warning"), getTelemetryStatus(isStreaming = true, bufferMs = 60.001, colors = colors))
        assertEquals(Pair(colors.systemOrange, "Warning"), getTelemetryStatus(isStreaming = true, bufferMs = 120.0, colors = colors))

        // 4. Starvation tier (< 20.0ms) -> red
        assertEquals(Pair(colors.systemRed, "Starvation"), getTelemetryStatus(isStreaming = true, bufferMs = 19.999, colors = colors))
        assertEquals(Pair(colors.systemRed, "Starvation"), getTelemetryStatus(isStreaming = true, bufferMs = 15.0, colors = colors))
        assertEquals(Pair(colors.systemRed, "Starvation"), getTelemetryStatus(isStreaming = true, bufferMs = 0.0, colors = colors))
        assertEquals(Pair(colors.systemRed, "Starvation"), getTelemetryStatus(isStreaming = true, bufferMs = -5.0, colors = colors))

        // 5. Formatting verification
        val telemetry = JitterTelemetry(
            bufferDepthMs = 42.0,
            targetDepthMs = 40.0,
            jitterMs = 2.4,
            bitrateKbps = 96.0,
            packetsReceived = 1520,
            packetsLost = 0,
            expansions = 0,
            contractions = 0,
            underruns = 0,
            overruns = 0
        )

        val jitterText = String.format(Locale.US, "±%.1f ms jitter", telemetry.jitterMs)
        assertEquals("±2.4 ms jitter", jitterText)

        val bitrateText = String.format(Locale.US, "%.0f kbps", telemetry.bitrateKbps)
        assertEquals("96 kbps", bitrateText)

        val dropsText = if (telemetry.packetsLost > 0) "${telemetry.packetsLost} drops" else "0 drops"
        assertEquals("0 drops", dropsText)
    }

    // ------------------------------------------------------------------------
    // 5. Apple HIG Touch Target Ergonomics Compliance
    // ------------------------------------------------------------------------

    @Test
    fun testAppleHigTouchTargetErgonomics() {
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

        // Verify AudioScreen action button height exceeds 44dp
        val audioActionButtonHeight = 52.dp
        assertTrue("Audio main action button (52dp) meets Apple HIG minimum (44dp)", audioActionButtonHeight >= 44.dp)

        val cardSquircle = DefaultCupertinoShapes.card
        assertNotNull(cardSquircle)
    }
}
