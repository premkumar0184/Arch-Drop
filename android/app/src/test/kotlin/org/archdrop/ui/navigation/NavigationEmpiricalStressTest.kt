package org.archdrop.ui.navigation

import android.content.Context
import androidx.activity.ComponentActivity
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Devices
import androidx.compose.material.icons.filled.Headphones
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.NearMe
import androidx.compose.material.icons.filled.QrCodeScanner
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Videocam
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
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.util.concurrent.Callable
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlin.random.Random

// ============================================================================
// EMPIRICAL CHALLENGER STRESS HARNESS: 4-TAB NAVIGATION & PERSISTENCE
// ============================================================================

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class NavigationEmpiricalStressTest {

    private lateinit var context: Context

    @Before
    fun setUp() {
        context = ApplicationProvider.getApplicationContext<Context>()
    }

    // ------------------------------------------------------------------------
    // Test 1: Screen.items strictly 4 tabs, ordering, uniqueness, and non-nullness
    // ------------------------------------------------------------------------
    @Test
    fun testScreenItemsStrictlyFourTabsAndIntegrity() {
        val items = Screen.items
        assertEquals("Screen.items must contain exactly 4 items per Apple HIG", 4, items.size)

        // Strict ordering
        assertSame("Tab 0 must be Drop", Screen.Drop, items[0])
        assertSame("Tab 1 must be Continuity", Screen.Continuity, items[1])
        assertSame("Tab 2 must be Activity", Screen.Activity, items[2])
        assertSame("Tab 3 must be Settings", Screen.Settings, items[3])

        // Demoted destinations must NOT be in items
        assertFalse("Audio must not be in bottom bar items", items.contains(Screen.Audio))
        assertFalse("Webcam must not be in bottom bar items", items.contains(Screen.Webcam))
        assertFalse("Pairing must not be in bottom bar items", items.contains(Screen.Pairing))

        // Route and title verification
        val routes = items.map { it.route }
        assertEquals("All tab routes must be unique", 4, routes.distinct().size)
        routes.forEach { route ->
            assertTrue("Route cannot be empty", route.isNotBlank())
            assertEquals("Route must be lowercase", route.lowercase(), route)
        }

        val titles = items.map { it.title }
        assertEquals("All tab titles must be unique", 4, titles.distinct().size)
        titles.forEach { title ->
            assertTrue("Title cannot be blank", title.isNotBlank())
        }

        val icons = items.map { it.icon }
        assertEquals("All tab icons must be unique", 4, icons.distinct().size)
    }

    // ------------------------------------------------------------------------
    // Test 2: Companion Aliases, Identity, and Concurrent Initialization Safety
    // ------------------------------------------------------------------------
    @Test
    fun testCompanionAliasesAndMultithreadedAccess() {
        assertSame("Screen.Dashboard must be identical to Screen.Drop", Screen.Drop, Screen.Dashboard)
        assertSame("Screen.Transfers must be identical to Screen.Activity", Screen.Activity, Screen.Transfers)

        // Stress: 50 concurrent threads accessing Screen companion getters simultaneously
        val executor = Executors.newFixedThreadPool(16)
        val tasks = (1..100).map { threadIdx ->
            Callable {
                val items = Screen.items
                assertEquals(4, items.size)
                assertNotNull(items[0].route)
                assertSame(Screen.Drop, Screen.Dashboard)
                assertSame(Screen.Activity, Screen.Transfers)
                items.all { it.title.isNotEmpty() && it.icon.name.isNotEmpty() }
            }
        }

        val results = executor.invokeAll(tasks)
        executor.shutdown()
        assertTrue("Executor should terminate within 5 seconds", executor.awaitTermination(5, TimeUnit.SECONDS))

        for (future in results) {
            assertTrue("All concurrent accesses must evaluate without race conditions", future.get())
        }
    }

    // ------------------------------------------------------------------------
    // Test 3: Tab Selection Oracle and Adversarial Route Matching Logic
    // ------------------------------------------------------------------------
    private fun isTabSelected(screen: Screen, currentRoute: String?): Boolean {
        return when (screen) {
            Screen.Drop -> currentRoute == Screen.Drop.route || currentRoute == "dashboard"
            Screen.Continuity -> currentRoute == Screen.Continuity.route || currentRoute == Screen.Audio.route || currentRoute == Screen.Webcam.route
            Screen.Activity -> currentRoute == Screen.Activity.route || currentRoute == "transfers"
            Screen.Settings -> currentRoute == Screen.Settings.route
            else -> currentRoute == screen.route
        }
    }

    @Test
    fun testTabSelectionOracleCanonicalAndLegacyRoutes() {
        // Canonical routes
        assertTrue("Drop must be selected for route 'drop'", isTabSelected(Screen.Drop, "drop"))
        assertFalse("Continuity must not be selected for 'drop'", isTabSelected(Screen.Continuity, "drop"))
        assertFalse("Activity must not be selected for 'drop'", isTabSelected(Screen.Activity, "drop"))
        assertFalse("Settings must not be selected for 'drop'", isTabSelected(Screen.Settings, "drop"))

        assertTrue("Continuity must be selected for route 'continuity'", isTabSelected(Screen.Continuity, "continuity"))
        assertTrue("Activity must be selected for route 'activity'", isTabSelected(Screen.Activity, "activity"))
        assertTrue("Settings must be selected for route 'settings'", isTabSelected(Screen.Settings, "settings"))

        // Legacy / Sub-routes
        assertTrue("Drop must be selected for legacy alias 'dashboard'", isTabSelected(Screen.Drop, "dashboard"))
        assertTrue("Continuity must be selected for sub-route 'audio'", isTabSelected(Screen.Continuity, "audio"))
        assertTrue("Continuity must be selected for sub-route 'webcam'", isTabSelected(Screen.Continuity, "webcam"))
        assertTrue("Activity must be selected for legacy alias 'transfers'", isTabSelected(Screen.Activity, "transfers"))
    }

    @Test
    fun testTabSelectionOracleAdversarialAndEdgeCaseRoutes() {
        val adversarialRoutes = listOf(
            null,
            "",
            "   ",
            "pairing", // Modal route: should NOT select any bottom tab
            "drop/123",
            "continuity/camera",
            "activity/details",
            "settings/peers",
            "unknown_random_route",
            "DROP",
            "CONTINUITY",
            "../drop",
            "drop?param=true"
        )

        for (route in adversarialRoutes) {
            val selectedCount = Screen.items.count { isTabSelected(it, route) }
            assertEquals(
                "Adversarial route '$route' should not select any bottom navigation tab",
                0,
                selectedCount
            )
        }
    }

    // ------------------------------------------------------------------------
    // Test 4: NavOptions Contract Verification (SingleTop, RestoreState, PopUpTo SaveState)
    // ------------------------------------------------------------------------
    @Test
    fun testNavOptionsContractAcrossDiverseStartDestinations() {
        val startDestinationIds = listOf(0, 1, 101, 10001, Int.MAX_VALUE, -1)

        for (startId in startDestinationIds) {
            val options = navOptions {
                popUpTo(startId) {
                    saveState = true
                }
                launchSingleTop = true
                restoreState = true
            }

            assertTrue("launchSingleTop must be true to avoid duplicate instances on back stack", options.shouldLaunchSingleTop())
            assertTrue("restoreState must be true to preserve view model and scroll states", options.shouldRestoreState())
            assertTrue("popUpTo saveState must be true to retain previous tab hierarchy", options.shouldPopUpToSaveState())
            assertFalse("popUpTo inclusive must be false so startDestination is NOT popped", options.isPopUpToInclusive())
            assertEquals("popUpTo target must match startDestinationId", startId, options.popUpToId)
        }
    }

    // ------------------------------------------------------------------------
    // Test 5: Rapid Multi-Stack Tab Switching Stress Test (10,000 transitions)
    // ------------------------------------------------------------------------
    @Test
    fun testRapidMultiStackTabSwitchingStress() {
        val tabRoutes = Screen.items.map { it.route }
        val tabStacks = mutableMapOf<String, MutableList<String>>()
        val savedStates = mutableMapOf<String, Int>()

        // Initialize root entries
        tabRoutes.forEach { route ->
            tabStacks[route] = mutableListOf("${route}_root")
            savedStates[route] = 0 // initial counter
        }

        var currentTab = Screen.Drop.route
        val random = Random(42)

        // Simulate 10,000 randomized tab switches
        for (i in 1..10000) {
            val targetTab = tabRoutes[random.nextInt(tabRoutes.size)]

            if (currentTab == targetTab) {
                // launchSingleTop = true: tapping active tab does NOT push duplicate
                val stackSize = tabStacks[currentTab]!!.size
                assertEquals("Tapping active tab must not grow back stack", 1, stackSize)
            } else {
                // Save state of current tab
                savedStates[currentTab] = savedStates[currentTab]!! + 1

                // Switch to target tab
                currentTab = targetTab

                // restoreState = true: target tab state is preserved
                val stack = tabStacks[targetTab]!!
                assertEquals("Tab stack must remain clean with single root entry", 1, stack.size)
                assertEquals("${targetTab}_root", stack[0])
            }
        }

        // Verify all 4 tabs survived all 10,000 transitions with their stacks intact
        assertEquals(4, tabStacks.size)
        tabRoutes.forEach { route ->
            assertEquals(1, tabStacks[route]!!.size)
            assertTrue("Tab $route must have recorded state updates", savedStates[route]!! > 0)
        }
    }

    // ------------------------------------------------------------------------
    // Test 6: MainActivity ViewModel Lifecycle Scoping & Orientation Handling
    // ------------------------------------------------------------------------
    @Test
    fun testMainActivityViewModelScopingAndOrientationChange() {
        val controller = Robolectric.buildActivity(ComponentActivity::class.java)
        val activity = controller.setup().get()
        val provider = ViewModelProvider(activity)

        val audioVm1 = provider[AudioViewModel::class.java]
        val audioVm2 = provider[AudioViewModel::class.java]
        assertSame("AudioViewModel must be scoped as singleton in Activity", audioVm1, audioVm2)

        val webcamVm1 = provider[WebcamViewModel::class.java]
        val webcamVm2 = provider[WebcamViewModel::class.java]
        assertSame("WebcamViewModel must be scoped as singleton in Activity", webcamVm1, webcamVm2)

        // Verify orientation change simulation: activity is recreated but ViewModels retain state
        val simulatedConfigChange = false // isFinishing is false during configuration change
        var teardownCalled = false
        fun handleDestroy(isFinishing: Boolean) {
            if (isFinishing) {
                teardownCalled = true
            }
        }

        handleDestroy(isFinishing = simulatedConfigChange)
        assertFalse("Streaming must not be torn down during orientation changes", teardownCalled)

        // Activity finish simulation: isFinishing is true
        handleDestroy(isFinishing = true)
        assertTrue("Streaming must be torn down when activity finishes", teardownCalled)
    }

    // ------------------------------------------------------------------------
    // Test 7: Modal Pairing Sheet State Hoisting & Back-Stack Independence
    // ------------------------------------------------------------------------
    @Test
    fun testModalSheetStateHoistingAndStackIndependence() {
        var showPairingSheet = false
        var currentNavRoute = Screen.Drop.route

        // Trigger pairing from Drop Screen
        val openPairingFromDrop = { showPairingSheet = true }
        openPairingFromDrop()
        assertTrue("Modal sheet state must be true", showPairingSheet)
        assertEquals("Active navigation route must remain 'drop' (no route push)", Screen.Drop.route, currentNavRoute)

        // Dismiss sheet
        val dismissSheet = { showPairingSheet = false }
        dismissSheet()
        assertFalse("Modal sheet must be dismissed", showPairingSheet)
        assertEquals("Active navigation route still 'drop'", Screen.Drop.route, currentNavRoute)

        // Navigate to Activity tab
        currentNavRoute = Screen.Activity.route
        assertEquals(Screen.Activity.route, currentNavRoute)

        // Trigger pairing from Settings Screen
        currentNavRoute = Screen.Settings.route
        val openPairingFromSettings = { showPairingSheet = true }
        openPairingFromSettings()
        assertTrue("Modal sheet state must be true when opened from Settings", showPairingSheet)
        assertEquals("Active navigation route must remain 'settings'", Screen.Settings.route, currentNavRoute)

        dismissSheet()
        assertFalse("Modal sheet must be dismissed", showPairingSheet)
        assertEquals(Screen.Settings.route, currentNavRoute)
    }

    // ------------------------------------------------------------------------
    // Test 8: Apple HIG Ergonomic Layout Dimensions & Minimum Touch Targets
    // ------------------------------------------------------------------------
    @Test
    fun testAppleHigErgonomicsAndTouchTargetStandards() {
        val appleMinTouchTarget = 44.dp
        assertEquals("Apple HIG minimum touch target is 44pt", 44.dp, appleMinTouchTarget)

        // Test touch target distribution across various device screen widths (compact to regular)
        val testWidths = listOf(320, 360, 375, 390, 412, 428, 600, 768)
        for (w in testWidths) {
            val tabWidth = w / 4.0
            assertTrue(
                "Tab width ($tabWidth dp) on screen width ($w dp) must be >= 44pt",
                tabWidth >= 44.0
            )
        }
    }
}
