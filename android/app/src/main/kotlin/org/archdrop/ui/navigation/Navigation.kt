package org.archdrop.ui.navigation

import android.app.Application
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import org.archdrop.ui.theme.CupertinoScaffold
import org.archdrop.ui.theme.CupertinoTabBar
import org.archdrop.ui.theme.CupertinoTabItem
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.NavGraph.Companion.findStartDestination
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import org.archdrop.db.ArchDropDatabase
import org.archdrop.ui.audio.AudioScreen
import org.archdrop.ui.audio.AudioViewModel
import org.archdrop.ui.continuity.ContinuityScreen
import org.archdrop.ui.dashboard.DashboardScreen
import org.archdrop.ui.dashboard.DashboardViewModel
import org.archdrop.ui.pairing.PairingBottomSheet
import org.archdrop.ui.pairing.PairingScreen
import org.archdrop.ui.settings.SettingsScreen
import org.archdrop.ui.theme.ArchDropTheme
import org.archdrop.ui.theme.cupertinoHairlineTop
import org.archdrop.ui.theme.cupertinoTouchTarget
import org.archdrop.ui.transfers.TransfersScreen
import org.archdrop.ui.webcam.WebcamScreen
import org.archdrop.ui.webcam.WebcamViewModel

@Composable
fun MainAppNavigation(
    database: ArchDropDatabase,
    dashboardViewModel: DashboardViewModel,
    webcamViewModel: WebcamViewModel? = null,
    audioViewModel: AudioViewModel? = null,
    navController: NavHostController = rememberNavController()
) {
    val navBackStackEntry by navController.currentBackStackEntryAsState()
    val currentRoute = navBackStackEntry?.destination?.route

    val context = LocalContext.current
    val haptic = LocalHapticFeedback.current
    val resolvedWebcamViewModel = webcamViewModel ?: viewModel {
        WebcamViewModel(context.applicationContext as Application)
    }
    val resolvedAudioViewModel = audioViewModel ?: viewModel {
        AudioViewModel(context.applicationContext as Application)
    }

    // Hoisted modal pairing bottom sheet state
    var showPairingSheet by rememberSaveable { mutableStateOf(false) }

    CupertinoScaffold(
        containerColor = ArchDropTheme.colors.canvasBackground,
        bottomBar = {
            CupertinoTabBar {
                Screen.items.forEach { screen ->
                    val isSelected = when (screen) {
                        Screen.Drop -> currentRoute == Screen.Drop.route || currentRoute == "dashboard"
                        Screen.Continuity -> currentRoute == Screen.Continuity.route || currentRoute == Screen.Audio.route || currentRoute == Screen.Webcam.route
                        Screen.Activity -> currentRoute == Screen.Activity.route || currentRoute == "transfers"
                        Screen.Settings -> currentRoute == Screen.Settings.route
                        else -> currentRoute == screen.route
                    }

                    CupertinoTabItem(
                        selected = isSelected,
                        onClick = {
                            if (currentRoute != screen.route) {
                                navController.navigate(screen.route) {
                                    popUpTo(navController.graph.findStartDestination().id) {
                                        saveState = true
                                    }
                                    launchSingleTop = true
                                    restoreState = true
                                }
                            }
                        },
                        icon = screen.icon,
                        label = screen.title
                    )
                }
            }
        }
    ) { innerPadding ->
        NavHost(
            navController = navController,
            startDestination = Screen.Drop.route,
            modifier = Modifier.padding(innerPadding)
        ) {
            // Canonical Apple HIG 4-Tab Destinations
            composable(Screen.Drop.route) {
                DashboardScreen(
                    viewModel = dashboardViewModel,
                    audioViewModel = resolvedAudioViewModel,
                    onNavigateToAudio = {
                        navController.navigate(Screen.Continuity.route) {
                            launchSingleTop = true
                        }
                    },
                    onOpenPairing = { showPairingSheet = true }
                )
            }
            composable(Screen.Continuity.route) {
                ContinuityScreen(
                    audioViewModel = resolvedAudioViewModel,
                    webcamViewModel = resolvedWebcamViewModel
                )
            }
            composable(Screen.Activity.route) {
                TransfersScreen(database = database)
            }
            composable(Screen.Settings.route) {
                SettingsScreen(
                    database = database,
                    onOpenPairing = { showPairingSheet = true }
                )
            }

            // Backward-Compatible Alias & Sub-Routes
            composable("dashboard") {
                DashboardScreen(
                    viewModel = dashboardViewModel,
                    audioViewModel = resolvedAudioViewModel,
                    onNavigateToAudio = {
                        navController.navigate(Screen.Continuity.route) {
                            launchSingleTop = true
                        }
                    },
                    onOpenPairing = { showPairingSheet = true }
                )
            }
            composable("transfers") {
                TransfersScreen(database = database)
            }
            composable(Screen.Audio.route) {
                AudioScreen(
                    viewModel = resolvedAudioViewModel,
                    onNavigateBack = {
                        navController.popBackStack()
                    }
                )
            }
            composable(Screen.Webcam.route) {
                WebcamScreen(viewModel = resolvedWebcamViewModel)
            }
            composable(Screen.Pairing.route) {
                PairingScreen(database = database)
            }
        }
    }

    // Hoisted Apple HIG Modal Pairing Bottom Sheet
    PairingBottomSheet(
        isVisible = showPairingSheet,
        onDismissRequest = { showPairingSheet = false },
        database = database
    )
}
