package org.archdrop.ui.audio

import android.app.Application
import android.content.Context
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.archdrop.audio.AudioReceiverState
import org.archdrop.audio.AudioStreamReceiver
import org.archdrop.audio.FakeAudioDecoder
import org.archdrop.audio.FakeAudioSink
import org.archdrop.audio.JitterRingBuffer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * Empirical adversarial challenge test harness for Milestone 3 (AudioViewModel & Jetpack Compose integration).
 *
 * Stress-tests:
 * 1. Extreme volume boost inputs: 0.0f, -1.0f, 1.5f, 2.0f, 2.5f, 100.0f, Float.POSITIVE_INFINITY, Float.NEGATIVE_INFINITY, Float.NaN.
 * 2. Rapid sequential start/stop calls (toggleStreaming()) ensuring no race conditions, coroutine leaks, or illegal state exceptions.
 * 3. Concurrent toggleStreaming() calls across multiple coroutines.
 * 4. Device selection with null and custom devices, verifying fallback to system default.
 */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], manifest = Config.NONE)
class AudioViewModelChallengerStressTest {

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

    private fun createTestReceiver(fakeSink: FakeAudioSink = FakeAudioSink()): AudioStreamReceiver {
        return HermeticAudioTestHelper.createHermeticAudioReceiver(
            context = app,
            decoder = FakeAudioDecoder(),
            sink = fakeSink,
            ringBuffer = JitterRingBuffer()
        )
    }

    /**
     * Verifies that standard extreme inputs (0.0f, -1.0f, 1.5f, 2.0f, 2.5f, 100.0f, +/- Infinity)
     * are properly clamped to [1.0f, 2.0f].
     */
    @Test
    fun testVolumeBoost_extremeInputsAndClamping() = runTest {
        val receiver = createTestReceiver()
        val viewModel = AudioViewModel(app, audioReceiver = receiver)

        val testCases = listOf(
            0.0f to 1.0f,
            -1.0f to 1.0f,
            1.5f to 1.5f,
            2.0f to 2.0f,
            2.5f to 2.0f,
            100.0f to 2.0f,
            Float.POSITIVE_INFINITY to 2.0f,
            Float.NEGATIVE_INFINITY to 1.0f
        )

        for ((input, expected) in testCases) {
            viewModel.setVolumeBoost(input)
            val actual = viewModel.volumeBoost.value
            assertEquals("Input $input failed to clamp to $expected", expected, actual, 0.0001f)
            assertEquals("Receiver input $input failed to clamp to $expected", expected, receiver.volumeBoost, 0.0001f)
            assertTrue("Volume boost must be >= 1.0f", actual >= 1.0f)
            assertTrue("Volume boost must be <= 2.0f", actual <= 2.0f)
        }
    }

    /**
     * EMPIRICAL CHALLENGE: Float.NaN volume boost input.
     *
     * In IEEE 754, NaN < 1.0f is false and NaN > 2.0f is false.
     * Consequently, Kotlin stdlib's `Float.coerceIn(1.0f, 2.0f)` returns Float.NaN!
     *
     * If Float.NaN is allowed through:
     * 1. viewModel.volumeBoost.value becomes Float.NaN.
     * 2. viewModel.volumeBoostPercent throws IllegalArgumentException: Cannot round NaN value.
     * 3. AudioScreen.kt (line 1024) crashes with IllegalArgumentException on recomposition.
     * 4. AudioStreamReceiver.kt (line 349) crashes startPlaybackLoop() with IllegalArgumentException.
     *
     * This test enforces that Float.NaN MUST be strictly sanitized and clamped into [1.0f, 2.0f].
     */
    @Test
    fun testVolumeBoost_nanInputMustBeSanitizedAndClamped() = runTest {
        val receiver = createTestReceiver()
        val viewModel = AudioViewModel(app, audioReceiver = receiver)

        // Set to known valid state first
        viewModel.setVolumeBoost(1.5f)
        assertEquals(1.5f, viewModel.volumeBoost.value, 0.001f)

        // Pass Float.NaN
        viewModel.setVolumeBoost(Float.NaN)
        val actual = viewModel.volumeBoost.value

        // Assert strictly clamped to [1.0f, 2.0f] and not NaN
        assertFalse("Volume boost must not be NaN when Float.NaN is passed", actual.isNaN())
        assertTrue("Volume boost must be strictly clamped within [1.0f, 2.0f], was $actual", actual in 1.0f..2.0f)

        // Verify volumeBoostPercent does not throw IllegalArgumentException
        val percent = viewModel.volumeBoostPercent
        assertTrue("Volume boost percent must be in 100..200, was $percent", percent in 100..200)
    }

    /**
     * Verifies output device selection and fallback to SYSTEM_DEFAULT on null or device disconnection.
     */
    @Test
    fun testDeviceSelection_nullAndCustomDeviceFallback() = runTest {
        val fakeSink = FakeAudioSink()
        val receiver = createTestReceiver(fakeSink)
        val viewModel = AudioViewModel(app, audioReceiver = receiver)

        // Initial default
        assertEquals(AudioDeviceUiModel.SYSTEM_DEFAULT, viewModel.selectedOutputDevice.value)

        // Custom device selection
        val customDevice = AudioDeviceUiModel(
            id = 99,
            name = "Bose QuietComfort 45",
            type = AudioDeviceType.BLUETOOTH,
            isDefault = false,
            rawDeviceInfo = null
        )
        viewModel.selectOutputDevice(customDevice)
        assertEquals(customDevice, viewModel.selectedOutputDevice.value)
        assertEquals(customDevice, viewModel.selectedDevice.value)

        // Select null device via selectDevice(null) -> must fallback to SYSTEM_DEFAULT
        viewModel.selectDevice(null)
        assertEquals(AudioDeviceUiModel.SYSTEM_DEFAULT, viewModel.selectedOutputDevice.value)
        assertEquals(AudioDeviceUiModel.SYSTEM_DEFAULT, viewModel.selectedDevice.value)
        assertTrue(viewModel.selectedOutputDevice.value.isDefault)

        // Select custom device again
        viewModel.selectOutputDevice(customDevice)
        assertEquals(customDevice, viewModel.selectedOutputDevice.value)

        // Trigger refreshAudioDevices(): since customDevice is not in physical AudioManager list, it must fallback
        viewModel.refreshAudioDevices()
        assertEquals(AudioDeviceUiModel.SYSTEM_DEFAULT, viewModel.selectedOutputDevice.value)
        assertTrue(viewModel.selectedOutputDevice.value.isDefault)
    }

    /**
     * Stress-tests rapid sequential toggling (100 times) to ensure no state desynchronization.
     */
    @Test
    fun testRapidSequentialToggles_noIllegalStateOrLeaks() = runTest {
        val fakeSink = FakeAudioSink()
        val receiver = createTestReceiver(fakeSink)
        val viewModel = AudioViewModel(app, audioReceiver = receiver)

        assertFalse("Initially idle", viewModel.isStreaming.value)

        // Rapid sequential toggle: 100 times
        for (i in 1..100) {
            viewModel.toggleStreaming()
            val expectedStreaming = (i % 2 != 0)
            assertEquals("Mismatch at iteration $i", expectedStreaming, viewModel.isStreaming.value)
        }

        // Even number of toggles -> must be stopped
        assertFalse(viewModel.isStreaming.value)

        // 101st toggle -> must start
        viewModel.toggleStreaming()
        assertTrue(viewModel.isStreaming.value)

        // Clean stop
        viewModel.stopStreaming()
        assertFalse(viewModel.isStreaming.value)
        assertEquals(AudioReceiverState.Idle, receiver.state.value)
    }

    /**
     * Stress-tests concurrent toggles across 20 coroutines to verify thread safety and exception-free termination.
     */
    @Test
    fun testConcurrentToggles_threadSafety() = runTest {
        val fakeSink = FakeAudioSink()
        val receiver = createTestReceiver(fakeSink)
        val viewModel = AudioViewModel(app, audioReceiver = receiver)

        // Launch 20 concurrent toggle calls
        val jobs = (1..20).map {
            async(Dispatchers.Default) {
                viewModel.toggleStreaming()
            }
        }
        jobs.awaitAll()
        testScheduler.advanceTimeBy(500)

        // ViewModel must be able to cleanly stop without exceptions
        viewModel.stopStreaming()
        testScheduler.advanceTimeBy(500)
        assertFalse(viewModel.isStreaming.value)
        assertEquals(AudioReceiverState.Idle, receiver.state.value)
    }
}
