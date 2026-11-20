package org.archdrop.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import androidx.core.content.ContextCompat

class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_BOOT_COMPLETED) {
            val serviceIntent = Intent(context, ArchDropSyncService::class.java).apply {
                action = ArchDropSyncService.ACTION_START
            }
            ContextCompat.startForegroundService(context, serviceIntent)
        } else if (intent.action == ArchDropSyncService.ACTION_BROADCAST_CLIPBOARD) {
            val serviceIntent = Intent(context, ArchDropSyncService::class.java).apply {
                action = ArchDropSyncService.ACTION_BROADCAST_CLIPBOARD
                putExtras(intent)
            }
            ContextCompat.startForegroundService(context, serviceIntent)
        }
    }
}
