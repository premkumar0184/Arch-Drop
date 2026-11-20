package org.archdrop.ui

import android.content.Intent
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.viewModels
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.Modifier
import org.archdrop.db.ArchDropDatabase
import org.archdrop.service.ArchDropSyncService
import org.archdrop.ui.audio.AudioViewModel
import org.archdrop.ui.dashboard.DashboardViewModel
import org.archdrop.ui.navigation.MainAppNavigation
import org.archdrop.ui.theme.ArchDropTheme
import org.archdrop.ui.webcam.WebcamViewModel

class MainActivity : ComponentActivity() {

    private val webcamViewModel: WebcamViewModel by viewModels()
    private val audioViewModel: AudioViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)

        val database = ArchDropDatabase.getInstance(applicationContext)
        val dashboardViewModel = DashboardViewModel(application, database)

        // Ensure background foreground service is initiated
        startSyncService()

        setContent {
            ArchDropTheme {
                Box(
                    modifier = Modifier
                        .fillMaxSize()
                        .background(ArchDropTheme.colors.canvasBackground)
                ) {
                    MainAppNavigation(
                        database = database,
                        dashboardViewModel = dashboardViewModel,
                        webcamViewModel = webcamViewModel,
                        audioViewModel = audioViewModel
                    )
                }
            }
        }
    }

    private fun startSyncService() {
        val intent = Intent(this, ArchDropSyncService::class.java).apply {
            action = ArchDropSyncService.ACTION_START
        }
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                startForegroundService(intent)
            } else {
                startService(intent)
            }
        } catch (_: Exception) {
            // Background start restrictions may apply during some states; gracefully ignored
        }
    }

    override fun onDestroy() {
        super.onDestroy()
        if (isFinishing) {
            webcamViewModel.stopStreaming()
            audioViewModel.stopStreaming()
        }
    }
}
