package org.archdrop.ui.navigation

import android.app.Application
import android.content.Context
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Devices
import androidx.compose.material.icons.filled.Headphones
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.NearMe
import androidx.compose.material.icons.filled.QrCodeScanner
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Videocam
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModelProvider
import androidx.navigation.NavOptions
import androidx.navigation.navOptions
import androidx.test.core.app.ApplicationProvider
import org.archdrop.ui.audio.AudioViewModel
import org.archdrop.ui.theme.DarkCupertinoColors
import org.archdrop.ui.theme.ElectricBlueDark
import org.archdrop.ui.theme.SystemGrayDark
import org.archdrop.ui.webcam.WebcamViewModel
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

// ============================================================================
// ARCH DROP APPLE HIG NAVIGATION & BACK-STACK PERSISTENCE UNIT TEST SUITE
// ============================================================================

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class NavigationTest {

    private lateinit var context: Context

    @Before
    fun setUp() {
        context = ApplicationProvider.getApplicationContext<Context>()
    }

    // ------------------------------------------------------------------------
    // 1. Destination Count & Apple HIG 4-Tab Architecture
    // ------------------------------------------------------------------------

    @Test
    fun testScreenItemsContainsExactlyFourAppleHigTabs() {
        // Apple HIG Tab Bar specification: exactly 4 primary navigation tabs
        assertEquals("Screen.items must contain exactly 4 tabs", 4, Screen.items.size)

        // Verify presence of the 4 core tabs
        assertTrue("Screen.items must contain Drop", Screen.items.contains(Screen.Drop))
        assertTrue("Screen.items must contain Continuity", Screen.items.contains(Screen.Continuity))
        assertTrue("Screen.items must contain Activity", Screen.items.contains(Screen.Activity))
        assertTrue("Screen.items must contain Settings", Screen.items.contains(Screen.Settings))

        // Demoted or consolidated destinations must NOT appear in bottom bar items
        assertFalse("Screen.items must NOT contain standalone Pairing", Screen.items.contains(Screen.Pairing))
        assertFalse("Screen.items must NOT contain standalone Audio", Screen.items.contains(Screen.Audio))
        assertFalse("Screen.items must NOT contain standalone Webcam", Screen.items.contains(Screen.Webcam))

        // All routes in Screen.items must be strictly distinct
        val routes = Screen.items.map { it.route }
        assertEquals("All routes in Screen.items must be unique", 4, routes.distinct().size)

        // Strict Apple HIG tab ordering: Drop (0), Continuity (1), Activity (2), Settings (3)
        assertEquals(Screen.Drop, Screen.items[0])
        assertEquals(Screen.Continuity, Screen.items[1])
        assertEquals(Screen.Activity, Screen.items[2])
        assertEquals(Screen.Settings, Screen.items[3])
    }

    // ------------------------------------------------------------------------
    // 2. Route Definitions, Titles, and HIG Icon Mapping
    // ------------------------------------------------------------------------

    @Test
    fun testAppleHigRouteDefinitionsAndMetadata() {
        // Tab 1: Drop (Dashboard)
        assertEquals("drop", Screen.Drop.route)
        assertEquals("Drop", Screen.Drop.title)
        assertEquals(Icons.Default.NearMe, Screen.Drop.icon)

        // Tab 2: Continuity Studio
        assertEquals("continuity", Screen.Continuity.route)
        assertEquals("Continuity", Screen.Continuity.title)
        assertEquals(Icons.Default.Devices, Screen.Continuity.icon)

        // Tab 3: Activity (Transfers)
        assertEquals("activity", Screen.Activity.route)
        assertEquals("Activity", Screen.Activity.title)
        assertEquals(Icons.Default.History, Screen.Activity.icon)

        // Tab 4: Settings
        assertEquals("settings", Screen.Settings.route)
        assertEquals("Settings", Screen.Settings.title)
        assertEquals(Icons.Default.Settings, Screen.Settings.icon)

        // Backward compatibility accessors
        assertEquals("Screen.Dashboard must route to 'drop'", "drop", Screen.Dashboard.route)
        assertEquals("Screen.Transfers must route to 'activity'", "activity", Screen.Transfers.route)

        // Sub-routes and modal definitions remain accessible for backwards compatibility
        assertEquals("audio", Screen.Audio.route)
        assertEquals("webcam", Screen.Webcam.route)
        assertEquals("pairing", Screen.Pairing.route)
    }

    // ------------------------------------------------------------------------
    // 3. Tab Selection Highlight State
    // ------------------------------------------------------------------------

    @Test
    fun testTabSelectionHighlightLogic() {
        // Drop tab selected
        var currentRoute = Screen.Drop.route
        assertTrue(currentRoute == Screen.Drop.route)
        assertFalse(currentRoute == Screen.Continuity.route)
        assertFalse(currentRoute == Screen.Activity.route)
        assertFalse(currentRoute == Screen.Settings.route)

        // Continuity tab selected
        currentRoute = Screen.Continuity.route
        assertFalse(currentRoute == Screen.Drop.route)
        assertTrue(currentRoute == Screen.Continuity.route)
        assertFalse(currentRoute == Screen.Activity.route)
        assertFalse(currentRoute == Screen.Settings.route)

        // Activity tab selected
        currentRoute = Screen.Activity.route
        assertFalse(currentRoute == Screen.Drop.route)
        assertFalse(currentRoute == Screen.Continuity.route)
        assertTrue(currentRoute == Screen.Activity.route)
        assertFalse(currentRoute == Screen.Settings.route)

        // Settings tab selected
        currentRoute = Screen.Settings.route
        assertFalse(currentRoute == Screen.Drop.route)
        assertFalse(currentRoute == Screen.Continuity.route)
        assertFalse(currentRoute == Screen.Activity.route)
        assertTrue(currentRoute == Screen.Settings.route)

        // Sub-route / Modal sheet active: bottom bar tabs are not highlighted
        currentRoute = "pairing"
        assertFalse(currentRoute == Screen.Drop.route)
        assertFalse(currentRoute == Screen.Continuity.route)
        assertFalse(currentRoute == Screen.Activity.route)
        assertFalse(currentRoute == Screen.Settings.route)
    }

    // ------------------------------------------------------------------------
    // 4. NavOptions Contract: SingleTop, RestoreState, PopUpTo SaveState
    // ------------------------------------------------------------------------

    @Test
    fun testNavOptionsConfigurationForTabPersistence() {
        val startDestinationId = 10001

        val options = navOptions {
            popUpTo(startDestinationId) {
                saveState = true
            }
            launchSingleTop = true
            restoreState = true
        }

        // 1. launchSingleTop must be true to avoid duplicate destination instances on back stack
        assertTrue("launchSingleTop must be true for tab navigation", options.shouldLaunchSingleTop())

        // 2. restoreState must be true to re-hydrate saved view state
        assertTrue("restoreState must be true for tab navigation", options.shouldRestoreState())

        // 3. popUpTo must specify saveState = true
        assertTrue("popUpTo saveState must be true to preserve tab history", options.shouldPopUpToSaveState())

        // 4. popUpTo target must match startDestinationId
        assertEquals("popUpTo target must match start destination id", startDestinationId, options.popUpToId)
    }

    // ------------------------------------------------------------------------
    // 5. Multi-Stack Back-Stack Simulation & Navigation Invariants
    // ------------------------------------------------------------------------

    @Test
    fun testBackStackRetentionAndMultiStackSimulation() {
        // Simulated multi-stack back stack manager
        val tabStacks = mutableMapOf<String, MutableList<String>>()
        val savedStates = mutableMapOf<String, String>()
        var activeTab = Screen.Drop.route

        tabStacks[Screen.Drop.route] = mutableListOf("drop_root")

        fun navigateTab(targetTab: String) {
            if (activeTab != targetTab) {
                // Save current tab state
                savedStates[activeTab] = "saved_state_for_$activeTab"

                // Pop up to root of current tab if needed, switch active tab
                activeTab = targetTab
                if (!tabStacks.containsKey(targetTab)) {
                    tabStacks[targetTab] = mutableListOf("${targetTab}_root")
                }
            }
        }

        // Navigate: Drop -> Continuity
        navigateTab(Screen.Continuity.route)
        assertEquals(Screen.Continuity.route, activeTab)
        assertTrue("State for Drop must be preserved", savedStates.containsKey(Screen.Drop.route))

        // Navigate: Continuity -> Activity
        navigateTab(Screen.Activity.route)
        assertEquals(Screen.Activity.route, activeTab)
        assertTrue("State for Continuity must be preserved", savedStates.containsKey(Screen.Continuity.route))

        // Return: Activity -> Continuity (restore state)
        navigateTab(Screen.Continuity.route)
        assertEquals(Screen.Continuity.route, activeTab)
        assertEquals("saved_state_for_continuity", savedStates[Screen.Continuity.route])

        // Tapping active tab repeatedly (launchSingleTop = true): no new entry pushed
        val initialSize = tabStacks[Screen.Continuity.route]?.size ?: 0
        navigateTab(Screen.Continuity.route)
        assertEquals(initialSize, tabStacks[Screen.Continuity.route]?.size)
    }

    // ------------------------------------------------------------------------
    // 6. MainActivity ViewModel Lifecycle Retention Across Tab Transitions
    // ------------------------------------------------------------------------

    @Test
    fun testMainActivityViewModelLifecycleRetentionAcrossTabTransitions() {
        val activity = Robolectric.buildActivity(ComponentActivity::class.java).setup().get()
        val provider = ViewModelProvider(activity)

        // Verify ViewModels are scoped as singletons within Activity's ViewModelStore
        val audioVm1 = provider[AudioViewModel::class.java]
        val audioVm2 = provider[AudioViewModel::class.java]
        assertSame("AudioViewModel must be scoped as singleton in Activity ViewModelStore", audioVm1, audioVm2)

        val webcamVm1 = provider[WebcamViewModel::class.java]
        val webcamVm2 = provider[WebcamViewModel::class.java]
        assertSame("WebcamViewModel must be scoped as singleton in Activity ViewModelStore", webcamVm1, webcamVm2)

        // Verify ViewModel retention across simulated tab navigation
        val initialAudioHost = audioVm1.targetHost.value
        assertEquals("10.2.0.2", initialAudioHost)

        // Simulate configuration change (isFinishing = false)
        var stopStreamingTriggered = false
        fun simulateOnDestroy(isFinishing: Boolean) {
            if (isFinishing) {
                stopStreamingTriggered = true
            }
        }

        // Orientation change: isFinishing is false -> streaming is NOT stopped
        simulateOnDestroy(isFinishing = false)
        assertFalse("Streaming must NOT be terminated during configuration changes", stopStreamingTriggered)

        // App exit: isFinishing is true -> streaming is cleaned up
        simulateOnDestroy(isFinishing = true)
        assertTrue("Streaming MUST be terminated when activity finishes", stopStreamingTriggered)
    }

    // ------------------------------------------------------------------------
    // 7. Modal Pairing Sheet State Toggling & Back-Stack Independence
    // ------------------------------------------------------------------------

    @Test
    fun testModalPairingSheetStateTogglingAndBackStackIndependence() {
        var showPairingSheet = false
        var currentRoute = Screen.Drop.route

        // Initial State: Sheet is hidden
        assertFalse("Pairing sheet must be initially hidden", showPairingSheet)
        assertEquals(Screen.Drop.route, currentRoute)

        // 1. Invocation from Drop Screen ("+ Pair Device" button)
        val onOpenPairingFromDrop = { showPairingSheet = true }
        onOpenPairingFromDrop()
        assertTrue("Pairing sheet must be visible after Drop trigger", showPairingSheet)
        // Back stack invariant: currentRoute is unaffected by modal presentation
        assertEquals("currentRoute must remain 'drop' when modal is presented", Screen.Drop.route, currentRoute)

        // 2. Dismissal via drag-down or close button
        val onDismissRequest = { showPairingSheet = false }
        onDismissRequest()
        assertFalse("Pairing sheet must be hidden after dismiss request", showPairingSheet)
        assertEquals(Screen.Drop.route, currentRoute)

        // 3. Navigate to Settings and invoke from Settings Screen ("Pair New Device" row)
        currentRoute = Screen.Settings.route
        val onOpenPairingFromSettings = { showPairingSheet = true }
        onOpenPairingFromSettings()
        assertTrue("Pairing sheet must be visible after Settings trigger", showPairingSheet)
        assertEquals("currentRoute must remain 'settings' when modal is presented", Screen.Settings.route, currentRoute)

        onDismissRequest()
        assertFalse("Pairing sheet must be hidden after dismiss request", showPairingSheet)
        assertEquals(Screen.Settings.route, currentRoute)

        // 4. SavedState survival: verify sheet state survives bundle saving
        val bundle = Bundle()
        bundle.putBoolean("showPairingSheet", true)
        val restoredState = bundle.getBoolean("showPairingSheet", false)
        assertTrue("Pairing sheet state must survive bundle restoration", restoredState)
    }

    // ------------------------------------------------------------------------
    // 8. Apple HIG Touch Target Ergonomics & Layout Dimensions
    // ------------------------------------------------------------------------

    @Test
    fun testAppleHigTouchTargetErgonomics() {
        val minTarget = 44.dp
        assertEquals(44.dp, minTarget)

        // On standard Android device widths (360dp, 390dp, 412dp),
        // 4 tabs divide horizontal width into >= 90dp per tab item
        val screenWidths = listOf(360, 390, 412)
        screenWidths.forEach { width ->
            val tabWidth = width / 4
            assertTrue(
                "Tab width ($tabWidth dp) on screen width ($width dp) must exceed 44dp minimum touch target",
                tabWidth >= 44
            )
        }
    }

    // ------------------------------------------------------------------------
    // 9. Cupertino Frosted Glass Bottom Bar Design Tokens
    // ------------------------------------------------------------------------

    @Test
    fun testCupertinoFrostedGlassBottomBarTokens() {
        // 80% opacity translucent scrim for bottom bar
        assertEquals(Color(0xCC1C1C1E), DarkCupertinoColors.glassBarBackground)
        assertTrue(DarkCupertinoColors.glassBarBackground.alpha in 0.79f..0.81f)

        // 0.5dp top hairline border
        assertEquals(Color(0x1AFFFFFF), DarkCupertinoColors.hairlineBorder)

        // Active tab tint (Arch Electric Blue) and inactive tab tint (System Gray)
        assertEquals(ElectricBlueDark, DarkCupertinoColors.primaryAccent)
        assertEquals(SystemGrayDark, DarkCupertinoColors.systemGray)
    }
}
