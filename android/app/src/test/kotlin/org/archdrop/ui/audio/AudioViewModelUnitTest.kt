package org.archdrop.ui.audio

import android.app.Application
import android.content.Context
import android.content.Intent
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
import org.archdrop.service.ArchDropSyncService
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

@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class AudioViewModelUnitTest {

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
        return AudioStreamReceiver(
            context = app,
            decoder = FakeAudioDecoder(),
            sink = FakeAudioSink(),
            ringBuffer = JitterRingBuffer()
        )
    }

    @Test
    fun testInitialState_defaultsAreCompliant() = runTest {
        val receiver = createTestReceiver()
        val viewModel = AudioViewModel(app, audioReceiver = receiver)

        assertFalse("Streaming must be idle initially", viewModel.isStreaming.value)
        assertEquals(AudioReceiverState.Idle, viewModel.receiverState.value)
        assertEquals("Default fallback host must be 10.2.0.2", "10.2.0.2", viewModel.targetHost.value)
        assertEquals(53317, viewModel.targetPort.value)
        assertEquals(1.0f, viewModel.volumeBoost.value, 0.001f)
        assertEquals(100, viewModel.volumeBoostPercent)
        assertFalse("Mute must be false initially", viewModel.isMuted.value)
        assertEquals(AudioDeviceUiModel.SYSTEM_DEFAULT, viewModel.selectedOutputDevice.value)
        assertTrue("Available devices must contain System Default", viewModel.availableOutputDevices.value.any { it.isDefault })
        assertEquals(AudioCodec.OPUS, viewModel.selectedCodec.value)
        assertEquals(96000, viewModel.selectedBitrate.value)
        assertEquals("monitor", viewModel.sinkMode.value)
        assertNotNull(viewModel.telemetry.value)
        assertTrue(viewModel.peerFingerprint.isNotEmpty())
    }

    @Test
    fun testInitialState_loadsSavedHostPreference() = runTest {
        app.getSharedPreferences("archdrop_prefs", Context.MODE_PRIVATE)
            .edit()
            .putString("last_peer_host", "192.168.1.188")
            .apply()

        val viewModel = AudioViewModel(app, audioReceiver = createTestReceiver())
        assertEquals("192.168.1.188", viewModel.targetHost.value)
    }

    @Test
    fun testSetTargetHost_persistsAndTrims() = runTest {
        val viewModel = AudioViewModel(app, audioReceiver = createTestReceiver())

        viewModel.setTargetHost("   10.2.0.77   ")
        assertEquals("10.2.0.77", viewModel.targetHost.value)

        val saved = app.getSharedPreferences("archdrop_prefs", Context.MODE_PRIVATE)
            .getString("last_peer_host", null)
        assertEquals("10.2.0.77", saved)

        // Empty/blank inputs should be ignored
        viewModel.setTargetHost("   ")
        assertEquals("10.2.0.77", viewModel.targetHost.value)
    }

    @Test
    fun testVolumeBoost_clampingAndPropagation() = runTest {
        val receiver = createTestReceiver()
        val viewModel = AudioViewModel(app, audioReceiver = receiver)

        // Lower bound clamp (1.0f)
        viewModel.setVolumeBoost(0.4f)
        assertEquals(1.0f, viewModel.volumeBoost.value, 0.001f)
        assertEquals(1.0f, receiver.volumeBoost, 0.001f)
        assertEquals(100, viewModel.volumeBoostPercent)

        // Upper bound clamp (2.0f)
        viewModel.setVolumeBoost(2.7f)
        assertEquals(2.0f, viewModel.volumeBoost.value, 0.001f)
        assertEquals(2.0f, receiver.volumeBoost, 0.001f)
        assertEquals(200, viewModel.volumeBoostPercent)

        // Mid-range value
        viewModel.setVolumeBoost(1.65f)
        assertEquals(1.65f, viewModel.volumeBoost.value, 0.001f)
        assertEquals(1.65f, receiver.volumeBoost, 0.001f)
        assertEquals(165, viewModel.volumeBoostPercent)
    }

    @Test
    fun testMute_togglingAndPropagation() = runTest {
        val receiver = createTestReceiver()
        val viewModel = AudioViewModel(app, audioReceiver = receiver)

        assertFalse(viewModel.isMuted.value)
        assertFalse(receiver.isMuted)

        viewModel.toggleMute()
        assertTrue(viewModel.isMuted.value)
        assertTrue(receiver.isMuted)

        viewModel.toggleMute()
        assertFalse(viewModel.isMuted.value)
        assertFalse(receiver.isMuted)

        viewModel.setMute(true)
        assertTrue(viewModel.isMuted.value)
        assertTrue(receiver.isMuted)
    }

    @Test
    fun testOutputDeviceSelection_systemDefaultAndCustomDevice() = runTest {
        val fakeSink = FakeAudioSink()
        val receiver = AudioStreamReceiver(context = app, sink = fakeSink)
        val viewModel = AudioViewModel(app, audioReceiver = receiver)

        val customDevice = AudioDeviceUiModel(
            id = 42,
            name = "Sony WH-1000XM4",
            type = AudioDeviceType.BLUETOOTH,
            isDefault = false,
            rawDeviceInfo = null
        )

        viewModel.selectOutputDevice(customDevice)
        assertEquals(customDevice, viewModel.selectedOutputDevice.value)
        assertEquals(null, fakeSink.preferredDevice)

        // Revert to System Default
        viewModel.selectOutputDevice(AudioDeviceUiModel.SYSTEM_DEFAULT)
        assertTrue(viewModel.selectedOutputDevice.value.isDefault)
        assertEquals(null, fakeSink.preferredDevice)
    }

    @Test
    fun testStreamParameters_codecAndBitrateUpdates() = runTest {
        val viewModel = AudioViewModel(app, audioReceiver = createTestReceiver())

        viewModel.setCodec(AudioCodec.PCM)
        assertEquals(AudioCodec.PCM, viewModel.selectedCodec.value)

        viewModel.setBitrate(128000)
        assertEquals(128000, viewModel.selectedBitrate.value)

        viewModel.setBitrate(256000) // Clamped to 128000
        assertEquals(128000, viewModel.selectedBitrate.value)

        viewModel.setSinkMode("virtual")
        assertEquals("virtual", viewModel.sinkMode.value)
    }

    @Test
    fun testStartAndStopStreaming_directReceiverControl() = runTest {
        val fakeSink = FakeAudioSink()
        val fakeDecoder = FakeAudioDecoder()
        val receiver = AudioStreamReceiver(
            context = app,
            sink = fakeSink,
            decoder = fakeDecoder
        )
        val viewModel = AudioViewModel(app, audioReceiver = receiver)

        viewModel.startStreaming(useServiceIntent = false, host = "127.0.0.1", port = 53317)
        assertTrue(viewModel.isStreaming.value)
        assertTrue(fakeSink.playCount >= 1)

        viewModel.stopStreaming(useServiceIntent = false)
        assertFalse(viewModel.isStreaming.value)
        assertEquals(AudioReceiverState.Idle, viewModel.receiverState.value)
        assertTrue(fakeSink.stopCount >= 1)
    }

    @Test
    fun testStartAndStopStreaming_viaServiceIntent() = runTest {
        val dispatchedIntents = mutableListOf<Intent>()
        val viewModel = AudioViewModel(
            application = app,
            audioReceiver = createTestReceiver(),
            serviceIntentSender = { dispatchedIntents.add(it) }
        )

        viewModel.setCodec(AudioCodec.OPUS)
        viewModel.setBitrate(128000)
        viewModel.setTargetHost("10.2.0.5")

        viewModel.startStreaming(useServiceIntent = true)
        assertEquals(1, dispatchedIntents.size)

        val startIntent = dispatchedIntents.first()
        assertEquals(ArchDropSyncService.ACTION_START_AUDIO_STREAM, startIntent.action)
        assertEquals("10.2.0.5", startIntent.getStringExtra(ArchDropSyncService.EXTRA_AUDIO_HOST))
        assertEquals(53317, startIntent.getIntExtra(ArchDropSyncService.EXTRA_AUDIO_PORT, 0))
        assertEquals("opus", startIntent.getStringExtra(ArchDropSyncService.EXTRA_AUDIO_CODEC))
        assertEquals(128000, startIntent.getIntExtra(ArchDropSyncService.EXTRA_AUDIO_BITRATE, 0))

        viewModel.stopStreaming(useServiceIntent = true)
        assertEquals(2, dispatchedIntents.size)

        val stopIntent = dispatchedIntents.last()
        assertEquals(ArchDropSyncService.ACTION_STOP_AUDIO_STREAM, stopIntent.action)
    }

    @Test
    fun testTelemetryFlowEmissions() = runTest {
        val ringBuffer = JitterRingBuffer()
        val receiver = AudioStreamReceiver(
            context = app,
            sink = FakeAudioSink(),
            decoder = FakeAudioDecoder(),
            ringBuffer = ringBuffer
        )
        val viewModel = AudioViewModel(app, audioReceiver = receiver)

        val initialTelemetry = viewModel.telemetry.value
        assertEquals(0.0, initialTelemetry.jitterMs, 0.001)

        // Write PCM to ring buffer to advance buffer depth
        val testPcm = ByteArray(1920) // 10 ms at 48kHz stereo
        ringBuffer.writePcm(testPcm, 0, testPcm.size, 1000L)

        val updatedTelemetry = ringBuffer.getTelemetry()
        assertEquals(12.0, updatedTelemetry.bufferDepthMs, 0.5)
        assertEquals(1L, updatedTelemetry.expansions)
    }

    @Test
    fun testLifecycleCleanup_onClearedStopsStreaming() = runTest {
        val fakeSink = FakeAudioSink()
        val receiver = AudioStreamReceiver(context = app, sink = fakeSink)
        val viewModel = AudioViewModel(app, audioReceiver = receiver)

        viewModel.startStreaming(useServiceIntent = false)
        assertTrue(viewModel.isStreaming.value)

        // Simulate ViewModel teardown via reflection on onCleared()
        val onClearedMethod = viewModel.javaClass.getDeclaredMethod("onCleared")
        onClearedMethod.isAccessible = true
        onClearedMethod.invoke(viewModel)

        assertFalse(viewModel.isStreaming.value)
        assertEquals(AudioReceiverState.Idle, viewModel.receiverState.value)
    }

    @Test
    fun testToggleStreaming() = runTest {
        val fakeSink = FakeAudioSink()
        val fakeDecoder = FakeAudioDecoder()
        val receiver = AudioStreamReceiver(
            context = app,
            sink = fakeSink,
            decoder = fakeDecoder
        )
        val viewModel = AudioViewModel(app, audioReceiver = receiver)

        assertFalse(viewModel.isStreaming.value)
        viewModel.toggleStreaming()
        assertTrue(viewModel.isStreaming.value)

        viewModel.toggleStreaming()
        assertFalse(viewModel.isStreaming.value)
    }
}
