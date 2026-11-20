package org.archdrop

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager
import android.os.Build
import androidx.camera.camera2.Camera2Config
import androidx.camera.core.CameraXConfig
import org.archdrop.companion.R
import org.bouncycastle.jce.provider.BouncyCastleProvider
import java.security.Security

class ArchDropApp : Application(), CameraXConfig.Provider {

    override fun getCameraXConfig(): CameraXConfig {
        return Camera2Config.defaultConfig()
    }

    companion object {
        const val CHANNEL_ID = "archdrop_sync_channel"
    }

    override fun onCreate() {
        super.onCreate()
        initSecurityProviders()
        createNotificationChannel()
    }

    private fun initSecurityProviders() {
        // Register Bouncy Castle provider first to ensure robust software Ed25519 support on Android
        Security.removeProvider(BouncyCastleProvider.PROVIDER_NAME)
        Security.insertProviderAt(BouncyCastleProvider(), 1)
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                getString(R.string.sync_service_channel_name),
                NotificationManager.IMPORTANCE_LOW
            ).apply {
                description = getString(R.string.sync_service_channel_desc)
                setShowBadge(false)
            }

            val notificationManager = getSystemService(NotificationManager::class.java)
            notificationManager?.createNotificationChannel(channel)
        }
    }
}
