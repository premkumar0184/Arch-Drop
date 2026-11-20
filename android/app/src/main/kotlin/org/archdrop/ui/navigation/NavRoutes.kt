package org.archdrop.ui.navigation

import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Devices
import androidx.compose.material.icons.filled.Headphones
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.NearMe
import androidx.compose.material.icons.filled.QrCodeScanner
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Videocam
import androidx.compose.ui.graphics.vector.ImageVector

/**
 * Navigation destination hierarchy following Apple Human Interface Guidelines (HIG).
 * Consolidates the primary experience into 4 core navigation tabs:
 * 1. Drop (Device status, peer radar, quick clipboard, active topology)
 * 2. Continuity (Unified hardware streaming: Wireless Audio + Continuity Camera)
 * 3. Activity (Transfer history, active streams, search, scoped filters)
 * 4. Settings (Grouped preferences, network/WireGuard status, trusted peers)
 *
 * Scoped actions such as QR Pairing are presented as modal sheets rather than tabs.
 */
sealed class Screen(val route: String, val title: String, val icon: ImageVector) {

    // --- 4 Core Apple HIG Navigation Tabs ---
    object Drop : Screen("drop", "Drop", Icons.Default.NearMe)
    object Continuity : Screen("continuity", "Continuity", Icons.Default.Devices)
    object Activity : Screen("activity", "Activity", Icons.Default.History)
    object Settings : Screen("settings", "Settings", Icons.Default.Settings)

    // --- Legacy / Internal Sub-Routes (For direct routing and test backward compatibility) ---
    object Audio : Screen("audio", "Audio", Icons.Default.Headphones)
    object Webcam : Screen("webcam", "Webcam", Icons.Default.Videocam)
    object Pairing : Screen("pairing", "Pairing", Icons.Default.QrCodeScanner)

    companion object {
        /**
         * Exactly 4 primary bottom navigation destinations per Apple HIG.
         */
        val items: List<Screen> get() = listOf(Drop, Continuity, Activity, Settings)

        // Compatibility aliases for static references
        val Dashboard: Screen get() = Drop
        val Transfers: Screen get() = Activity
    }
}
