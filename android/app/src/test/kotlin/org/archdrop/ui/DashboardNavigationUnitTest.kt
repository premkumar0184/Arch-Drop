package org.archdrop.ui

import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Headphones
import org.archdrop.audio.AudioReceiverState
import org.archdrop.audio.JitterTelemetry
import org.archdrop.ui.dashboard.getAudioStatusSummary
import org.archdrop.ui.navigation.Screen
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class DashboardNavigationUnitTest {

    @Test
    fun testAudioRouteRegistration() {
        assertEquals("audio", Screen.Audio.route)
        assertEquals("Audio", Screen.Audio.title)
        assertEquals(Icons.Default.Headphones, Screen.Audio.icon)
    }

    @Test
    fun testNavigationScreenItemsContainsContinuity() {
        assertTrue("Screen.items must contain Screen.Continuity", Screen.items.contains(Screen.Continuity))
        assertEquals("Screen.items must contain exactly 4 tabs per Apple HIG", 4, Screen.items.size)
        val routes = Screen.items.map { it.route }
        assertEquals("All routes in Screen.items must be unique", routes.distinct().size, routes.size)
    }

    @Test
    fun testGetAudioStatusSummaryFormatting() {
        val dummyTelemetry = JitterTelemetry(
            bufferDepthMs = 42.0,
            targetDepthMs = 40.0,
            jitterMs = 2.4,
            bitrateKbps = 96.0,
            packetsReceived = 1000,
            packetsLost = 0,
            expansions = 0,
            contractions = 0,
            underruns = 0,
            overruns = 0
        )

        // Idle state
        val idleSummary = getAudioStatusSummary(AudioReceiverState.Idle, dummyTelemetry)
        assertTrue(idleSummary.contains("wireless headphone receiver"))

        // Connecting state
        val connectingSummary = getAudioStatusSummary(AudioReceiverState.Connecting, dummyTelemetry)
        assertTrue(connectingSummary.contains("Connecting to desktop"))

        // Streaming state
        val streamingSummary = getAudioStatusSummary(AudioReceiverState.Streaming, dummyTelemetry)
        assertTrue(streamingSummary.contains("Streaming · 42ms buffer · ±2.4ms jitter"))

        // Paused state
        val pausedSummary = getAudioStatusSummary(AudioReceiverState.Paused, dummyTelemetry)
        assertTrue(pausedSummary.contains("Audio focus paused"))

        // Error state
        val errorSummary = getAudioStatusSummary(AudioReceiverState.Error("Server timeout occurred"), dummyTelemetry)
        assertTrue(errorSummary.contains("Server timeout occurred"))

        // Disconnected state
        val disconnectedSummary = getAudioStatusSummary(AudioReceiverState.Disconnected, dummyTelemetry)
        assertTrue(disconnectedSummary.contains("disconnected"))
    }
}
