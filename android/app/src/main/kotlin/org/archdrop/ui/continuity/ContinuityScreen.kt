package org.archdrop.ui.continuity

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.view.WindowManager
import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import org.archdrop.ui.audio.AudioScreen
import org.archdrop.ui.audio.AudioViewModel
import org.archdrop.ui.theme.ArchDropTheme
import org.archdrop.ui.theme.CupertinoSegmentedControl
import org.archdrop.ui.theme.cupertinoHairlineBottom
import org.archdrop.ui.webcam.WebcamScreen
import org.archdrop.ui.webcam.WebcamViewModel

/**
 * Finds the host [Activity] traversing possible [ContextWrapper] layers.
 */
tailrec fun Context.findActivity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.findActivity()
    else -> null
}

/**
 * Available tabs in the Continuity Studio Hub.
 */
enum class ContinuitySegment(val index: Int, val title: String) {
    AUDIO(0, "Wireless Audio"),
    CAMERA(1, "Continuity Camera")
}

/**
 * Continuity Studio Screen uniting Wireless Audio and Continuity Camera under
 * an authentic iOS Segmented Control with concentric squircle geometry (10dp track, 8dp thumb).
 *
 * Scoped across MainActivity to retain ViewModel streaming state without interruption.
 * Hoists screen-awake management ([WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON]) so
 * the screen remains active while the camera is streaming even if the user switches to the audio tab.
 */
@Composable
fun ContinuityScreen(
    audioViewModel: AudioViewModel,
    webcamViewModel: WebcamViewModel,
    modifier: Modifier = Modifier
) {
    val context = LocalContext.current
    var selectedSegmentIndex by rememberSaveable { mutableIntStateOf(ContinuitySegment.AUDIO.index) }
    val segments = listOf(ContinuitySegment.AUDIO.title, ContinuitySegment.CAMERA.title)

    val isCameraStreaming by webcamViewModel.isStreaming.collectAsState()

    // Hoisted Screen Awake Management:
    // Keeps screen awake whenever Camera streaming is active, even if switched to Audio tab
    DisposableEffect(isCameraStreaming) {
        val activity = context.findActivity()
        if (isCameraStreaming) {
            activity?.window?.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        } else {
            activity?.window?.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
        onDispose {
            activity?.window?.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
    }

    Column(
        modifier = modifier
            .fillMaxSize()
            .background(ArchDropTheme.colors.canvasBackground)
    ) {
        // iOS Cupertino Segmented Control Bar Header
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .background(ArchDropTheme.colors.glassBarBackground)
                .cupertinoHairlineBottom(ArchDropTheme.colors.hairlineBorder)
                .padding(horizontal = 16.dp, vertical = 10.dp)
        ) {
            CupertinoSegmentedControl(
                items = segments,
                selectedIndex = selectedSegmentIndex,
                onItemSelected = { selectedSegmentIndex = it }
            )
        }

        // Segmented Content Views with Seamless 200ms Crossfade Transition
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .weight(1f)
        ) {
            Crossfade(
                targetState = selectedSegmentIndex,
                animationSpec = tween(durationMillis = 200),
                label = "ContinuitySegmentCrossfade"
            ) { segmentIndex ->
                when (segmentIndex) {
                    ContinuitySegment.AUDIO.index -> {
                        AudioScreen(viewModel = audioViewModel)
                    }
                    ContinuitySegment.CAMERA.index -> {
                        WebcamScreen(
                            viewModel = webcamViewModel,
                            manageScreenAwake = false
                        )
                    }
                }
            }
        }
    }
}
