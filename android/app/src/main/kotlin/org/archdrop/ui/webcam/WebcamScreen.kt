package org.archdrop.ui.webcam

import android.Manifest
import android.app.Activity
import android.os.Build
import android.content.Context
import android.content.ContextWrapper
import android.content.pm.PackageManager
import android.view.WindowManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.view.PreviewView
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Cameraswitch
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.DarkMode
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Videocam
import androidx.compose.material.icons.filled.VideocamOff
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import org.archdrop.ui.theme.CupertinoButton
import org.archdrop.ui.theme.CupertinoButtonStyle
import org.archdrop.ui.theme.CupertinoDivider
import org.archdrop.ui.theme.CupertinoModalBottomSheet
import org.archdrop.ui.theme.CupertinoTextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LocalLifecycleOwner
import kotlinx.coroutines.launch
import org.archdrop.ui.theme.ArchDropTheme
import org.archdrop.ui.theme.cupertinoGlassPill
import org.archdrop.ui.theme.cupertinoHairline
import org.archdrop.ui.theme.cupertinoTouchTarget
import org.archdrop.webcam.WebcamConnectionState
import org.archdrop.webcam.WebcamLens
import org.archdrop.webcam.WebcamResolution
import java.util.Locale

private val isRobolectric = Build.FINGERPRINT == "robolectric" || Build.HARDWARE == "robolectric"

/**
 * Finds the host [Activity] traversing possible [ContextWrapper] layers.
 */
private tailrec fun Context.findActivity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.findActivity()
    else -> null
}

/**
 * Continuity Camera View for Arch Drop inspired by macOS Continuity Camera.
 *
 * Adheres strictly to Apple Human Interface Guidelines (HIG):
 * - Rounded squircle viewfinder preview (20dp corner radius, 16:9 aspect ratio, letterboxed on OLED true black canvas)
 * - Floating frosted glass pill controls:
 *   - Resolution selector modal/popup (1080p FHD, 720p HD default, 480p SD)
 *   - Lens selector toggle with haptic touch target (Front vs Back camera)
 *   - Rolling FPS and bitrate telemetry capsule
 * - OLED blackout low-power mode with tap-to-wake gesture & window backlight override
 * - Inset grouped preferences card for peer target and connection diagnostics
 * - Screen awake ([WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON]) lifecycle management
 */
@Composable
fun WebcamScreen(
    viewModel: WebcamViewModel,
    modifier: Modifier = Modifier,
    manageScreenAwake: Boolean = true
) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val coroutineScope = rememberCoroutineScope()

    val isStreaming by viewModel.isStreaming.collectAsState()
    val fps by viewModel.fps.collectAsState()
    val connectionState by viewModel.connectionState.collectAsState()
    val selectedResolution by viewModel.selectedResolution.collectAsState()
    val selectedLens by viewModel.selectedLens.collectAsState()
    val isLowPowerMode by viewModel.isLowPowerMode.collectAsState()
    val targetHost by viewModel.targetHost.collectAsState()
    val hasCameraPermission by viewModel.hasCameraPermission.collectAsState()

    var showResolutionModal by rememberSaveable { mutableStateOf(false) }
    var showSettingsCard by rememberSaveable { mutableStateOf(false) }
    var hostInput by remember(targetHost) { mutableStateOf(targetHost) }

    // Screen awake management: keep screen on while streaming to prevent OS sleep.
    // When embedded in ContinuityScreen, manageScreenAwake is passed as false because
    // ContinuityScreen hoists window screen-awake management at the studio hub level,
    // preventing crossfade disposal from clearing FLAG_KEEP_SCREEN_ON when switching tabs.
    if (manageScreenAwake) {
        DisposableEffect(isStreaming) {
            val activity = context.findActivity()
            if (isStreaming) {
                activity?.window?.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            } else {
                activity?.window?.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
            onDispose {
                activity?.window?.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
        }
    }

    // OLED Blackout hardware brightness management: dims panel to minimum backlight
    DisposableEffect(isLowPowerMode) {
        val activity = context.findActivity()
        if (isLowPowerMode) {
            activity?.window?.attributes = activity?.window?.attributes?.apply {
                screenBrightness = 0.01f
            }
        } else {
            activity?.window?.attributes = activity?.window?.attributes?.apply {
                screenBrightness = WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_NONE
            }
        }
        onDispose {
            activity?.window?.attributes = activity?.window?.attributes?.apply {
                screenBrightness = WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_NONE
            }
        }
    }

    // Camera runtime permission launcher
    val permissionLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.RequestPermission()
    ) { isGranted ->
        viewModel.updateCameraPermission(isGranted)
        if (isGranted) {
            viewModel.startStreaming(lifecycleOwner)
        }
    }

    // Check camera permission on first composition
    LaunchedEffect(Unit) {
        val granted = ContextCompat.checkSelfPermission(
            context,
            Manifest.permission.CAMERA
        ) == PackageManager.PERMISSION_GRANTED
        viewModel.updateCameraPermission(granted)
    }

    Box(
        modifier = modifier
            .fillMaxSize()
            .background(ArchDropTheme.colors.canvasBackground)
    ) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
            horizontalAlignment = Alignment.CenterHorizontally
        ) {
            // Viewfinder Section: 16:9 Aspect Ratio, 20dp Squircle, Letterboxed on True Black Canvas
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .aspectRatio(16f / 9f)
                    .clip(ArchDropTheme.shapes.heroSquircle)
                    .background(Color(0xFF0D0D0E))
                    .cupertinoHairline(shape = ArchDropTheme.shapes.heroSquircle),
                contentAlignment = Alignment.Center
            ) {
                if (isStreaming && hasCameraPermission) {
                    // Live CameraX PreviewView with safe fallback
                    AndroidView(
                        factory = { ctx ->
                            PreviewView(ctx).apply {
                                scaleType = PreviewView.ScaleType.FILL_CENTER
                                implementationMode = PreviewView.ImplementationMode.COMPATIBLE
                            }
                        },
                        modifier = Modifier.fillMaxSize()
                    )
                } else {
                    // Viewfinder Standby State
                    StandbyViewfinderPlaceholder(
                        hasPermission = hasCameraPermission,
                        onGrantPermission = { permissionLauncher.launch(Manifest.permission.CAMERA) }
                    )
                }

                // Top Floating Telemetry Capsule (Rolling FPS, Bitrate, Connection Dot)
                Box(
                    modifier = Modifier
                        .align(Alignment.TopCenter)
                        .padding(top = 10.dp)
                ) {
                    CameraTelemetryCapsule(
                        connectionState = connectionState,
                        fps = fps,
                        resolution = selectedResolution,
                        isStreaming = isStreaming
                    )
                }

                // Bottom Floating Glass Pills: Resolution Picker & Lens Switcher
                Row(
                    modifier = Modifier
                        .align(Alignment.BottomCenter)
                        .fillMaxWidth()
                        .padding(horizontal = 12.dp, vertical = 10.dp),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    // Resolution Selector Pill
                    Box(
                        modifier = Modifier
                            .cupertinoGlassPill(alpha = 0.55f)
                            .cupertinoTouchTarget()
                            .clickable { showResolutionModal = true }
                            .padding(horizontal = 12.dp, vertical = 6.dp),
                        contentAlignment = Alignment.Center
                    ) {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(4.dp)
                        ) {
                            Text(
                                text = selectedResolution.label.substringBefore(" "),
                                style = ArchDropTheme.typography.subheadline,
                                fontWeight = FontWeight.SemiBold,
                                color = ArchDropTheme.colors.label
                            )
                            Text(
                                text = "▾",
                                style = ArchDropTheme.typography.caption1,
                                color = ArchDropTheme.colors.secondaryLabel
                            )
                        }
                    }

                    // Lens Selector Pill (Front / Back camera switch)
                    Box(
                        modifier = Modifier
                            .cupertinoGlassPill(alpha = 0.55f)
                            .cupertinoTouchTarget()
                            .clickable {
                                val nextLens = if (selectedLens == WebcamLens.BACK) WebcamLens.FRONT else WebcamLens.BACK
                                viewModel.setLens(nextLens, lifecycleOwner)
                            }
                            .padding(horizontal = 12.dp, vertical = 6.dp),
                        contentAlignment = Alignment.Center
                    ) {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(6.dp)
                        ) {
                            Icon(
                                imageVector = Icons.Default.Cameraswitch,
                                contentDescription = "Switch Camera Lens",
                                tint = ArchDropTheme.colors.label,
                                modifier = Modifier.size(16.dp)
                            )
                            Text(
                                text = if (selectedLens == WebcamLens.BACK) "Back" else "Front",
                                style = ArchDropTheme.typography.subheadline,
                                fontWeight = FontWeight.SemiBold,
                                color = ArchDropTheme.colors.label
                            )
                        }
                    }
                }
            }

            // Camera Permission Warning Banner (if permission denied)
            if (!hasCameraPermission) {
                CameraPermissionCard(onGrantPermission = { permissionLauncher.launch(Manifest.permission.CAMERA) })
            }

            // Primary Stream Action Button (Apple HIG Prominent Pill Button)
            CupertinoButton(
                onClick = {
                    if (isStreaming) {
                        viewModel.stopStreaming()
                    } else {
                        if (hasCameraPermission) {
                            viewModel.startStreaming(lifecycleOwner)
                        } else {
                            permissionLauncher.launch(Manifest.permission.CAMERA)
                        }
                    }
                },
                style = if (isStreaming) CupertinoButtonStyle.DESTRUCTIVE else CupertinoButtonStyle.FILLED,
                shape = ArchDropTheme.shapes.buttonLarge,
                modifier = Modifier
                    .fillMaxWidth()
                    .height(52.dp)
            ) {
                Icon(
                    imageVector = if (isStreaming) Icons.Default.VideocamOff else Icons.Default.Videocam,
                    contentDescription = null,
                    modifier = Modifier.size(20.dp)
                )
                Spacer(modifier = Modifier.width(10.dp))
                Text(
                    text = if (isStreaming) "Disconnect Camera" else "Start Camera Stream",
                    style = ArchDropTheme.typography.headline,
                    fontWeight = FontWeight.SemiBold
                )
            }

            // Secondary Floating Actions Row: OLED Blackout & Settings Toggle
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(10.dp)
            ) {
                // OLED Blackout Mode Pill Button
                Box(
                    modifier = Modifier
                        .weight(1f)
                        .clip(ArchDropTheme.shapes.card)
                        .background(ArchDropTheme.colors.secondarySystemGroupedBackground)
                        .cupertinoHairline(shape = ArchDropTheme.shapes.card)
                        .cupertinoTouchTarget()
                        .clickable { viewModel.setLowPowerMode(true) }
                        .padding(horizontal = 14.dp, vertical = 12.dp),
                    contentAlignment = Alignment.Center
                ) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp)
                    ) {
                        Icon(
                            imageVector = Icons.Default.DarkMode,
                            contentDescription = "OLED Blackout",
                            tint = ArchDropTheme.colors.primaryAccent,
                            modifier = Modifier.size(18.dp)
                        )
                        Text(
                            text = "OLED Blackout",
                            style = ArchDropTheme.typography.subheadline,
                            fontWeight = FontWeight.Medium,
                            color = ArchDropTheme.colors.label
                        )
                    }
                }

                // Diagnostics & Settings Pill Button
                Box(
                    modifier = Modifier
                        .weight(1f)
                        .clip(ArchDropTheme.shapes.card)
                        .background(ArchDropTheme.colors.secondarySystemGroupedBackground)
                        .cupertinoHairline(shape = ArchDropTheme.shapes.card)
                        .cupertinoTouchTarget()
                        .clickable { showSettingsCard = !showSettingsCard }
                        .padding(horizontal = 14.dp, vertical = 12.dp),
                    contentAlignment = Alignment.Center
                ) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp)
                    ) {
                        Icon(
                            imageVector = Icons.Default.Settings,
                            contentDescription = "Host Settings",
                            tint = ArchDropTheme.colors.systemGray,
                            modifier = Modifier.size(18.dp)
                        )
                        Text(
                            text = if (showSettingsCard) "Hide Settings" else "Settings",
                            style = ArchDropTheme.typography.subheadline,
                            fontWeight = FontWeight.Medium,
                            color = ArchDropTheme.colors.label
                        )
                    }
                }
            }

            // Expandable Inset Grouped Camera Settings & Diagnostics Card
            AnimatedVisibility(
                visible = showSettingsCard,
                enter = fadeIn(),
                exit = fadeOut()
            ) {
                InsetGroupedSettingsCard(
                    hostInput = hostInput,
                    onHostInputChange = {
                        hostInput = it
                        viewModel.setTargetHost(it)
                    },
                    targetPort = viewModel.targetPort,
                    selectedResolution = selectedResolution,
                    selectedLens = selectedLens
                )
            }

            Spacer(modifier = Modifier.height(16.dp))
        }

        // Apple HIG Resolution Selector Modal Bottom Sheet
        if (showResolutionModal) {
            CupertinoModalBottomSheet(
                onDismissRequest = { showResolutionModal = false }
            ) {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 20.dp, vertical = 8.dp)
                        .padding(bottom = 24.dp),
                    verticalArrangement = Arrangement.spacedBy(16.dp)
                ) {
                    Text(
                        text = "Camera Resolution",
                        style = ArchDropTheme.typography.title2,
                        fontWeight = FontWeight.Bold,
                        color = ArchDropTheme.colors.label
                    )
                    Text(
                        text = "Select capture and streaming resolution for the Linux V4L2 virtual device.",
                        style = ArchDropTheme.typography.footnote,
                        color = ArchDropTheme.colors.secondaryLabel
                    )

                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .clip(ArchDropTheme.shapes.card)
                            .background(ArchDropTheme.colors.tertiarySystemGroupedBackground)
                            .cupertinoHairline(shape = ArchDropTheme.shapes.card)
                    ) {
                        WebcamResolution.values().forEachIndexed { index, res ->
                            val isSelected = selectedResolution == res
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable {
                                        viewModel.setResolution(res, lifecycleOwner)
                                        showResolutionModal = false
                                    }
                                    .padding(horizontal = 16.dp, vertical = 14.dp),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.CenterVertically
                            ) {
                                Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                    Text(
                                        text = res.label,
                                        style = ArchDropTheme.typography.body,
                                        fontWeight = if (isSelected) FontWeight.SemiBold else FontWeight.Normal,
                                        color = if (isSelected) ArchDropTheme.colors.primaryAccent else ArchDropTheme.colors.label
                                    )
                                    Text(
                                        text = when (res) {
                                            WebcamResolution.RES_1080P -> "1920×1080 • Best quality, higher bandwidth"
                                            WebcamResolution.RES_720P -> "1280×720 • Recommended default, <80ms latency"
                                            WebcamResolution.RES_480P -> "640×480 • Low bandwidth mode"
                                        },
                                        style = ArchDropTheme.typography.caption1,
                                        color = ArchDropTheme.colors.secondaryLabel
                                    )
                                }
                                if (isSelected) {
                                    Icon(
                                        imageVector = Icons.Default.Check,
                                        contentDescription = "Selected",
                                        tint = ArchDropTheme.colors.primaryAccent,
                                        modifier = Modifier.size(20.dp)
                                    )
                                }
                            }
                            if (index < WebcamResolution.values().size - 1) {
                                CupertinoDivider(
                                    modifier = Modifier.padding(start = 16.dp)
                                )
                            }
                        }
                    }
                }
            }
        }

        // Full Screen OLED Blackout Low-Power Overlay with Tap-to-Wake
        if (isLowPowerMode) {
            CameraBlackoutOverlay(
                isStreaming = isStreaming,
                fps = fps,
                targetHost = targetHost,
                targetPort = viewModel.targetPort,
                resolution = selectedResolution,
                onWake = { viewModel.setLowPowerMode(false) }
            )
        }
    }
}

/**
 * Floating glass telemetry capsule displaying live connection state dot, rolling FPS, and resolution.
 */
@Composable
private fun CameraTelemetryCapsule(
    connectionState: WebcamConnectionState,
    fps: Double,
    resolution: WebcamResolution,
    isStreaming: Boolean
) {
    val (statusDotColor, statusText) = when (connectionState) {
        is WebcamConnectionState.Streaming -> Pair(ArchDropTheme.colors.systemGreen, "Streaming")
        is WebcamConnectionState.Connecting -> Pair(ArchDropTheme.colors.systemOrange, "Connecting...")
        is WebcamConnectionState.Error -> Pair(ArchDropTheme.colors.systemRed, "Error")
        is WebcamConnectionState.Disconnected -> Pair(ArchDropTheme.colors.systemGray, "Disconnected")
        is WebcamConnectionState.Idle -> Pair(ArchDropTheme.colors.primaryAccent, "Ready")
    }

    // Breathing pulse animation for active streaming dot
    val dotAlpha: Float
    if (isRobolectric) {
        dotAlpha = 1.0f
    } else {
        val transition = rememberInfiniteTransition(label = "pulse")
        val animatedAlpha by transition.animateFloat(
            initialValue = 0.4f,
            targetValue = 1.0f,
            animationSpec = infiniteRepeatable(
                animation = tween(800),
                repeatMode = RepeatMode.Reverse
            ),
            label = "dotAlpha"
        )
        dotAlpha = animatedAlpha
    }

    Box(
        modifier = Modifier
            .cupertinoGlassPill(alpha = 0.65f)
            .padding(horizontal = 12.dp, vertical = 6.dp)
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            // Live status dot
            Box(
                modifier = Modifier
                    .size(8.dp)
                    .clip(CircleShape)
                    .background(statusDotColor.copy(alpha = if (isStreaming) dotAlpha else 1.0f))
            )

            // Rolling FPS and telemetry
            val telemetryString = if (isStreaming && fps > 0.0) {
                val estimatedKbps = (fps * 55.0 * 8.0).toInt() // typical frame size ~55KB at 720p
                val bitrateText = if (estimatedKbps >= 1000) {
                    String.format(Locale.US, "%.1f Mbps", estimatedKbps / 1000.0)
                } else {
                    "$estimatedKbps kbps"
                }
                String.format(Locale.US, "%.1f FPS • %s • %s", fps, bitrateText, resolution.label.substringBefore(" "))
            } else {
                "${statusText} • ${resolution.label.substringBefore(" ")}"
            }

            Text(
                text = telemetryString,
                style = ArchDropTheme.typography.caption1,
                fontFamily = FontFamily.Monospace,
                fontWeight = FontWeight.SemiBold,
                color = ArchDropTheme.colors.label
            )
        }
    }
}

/**
 * Viewfinder standby placeholder when camera is not actively streaming.
 */
@Composable
private fun StandbyViewfinderPlaceholder(
    hasPermission: Boolean,
    onGrantPermission: () -> Unit
) {
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(8.dp),
        modifier = Modifier.padding(24.dp)
    ) {
        Icon(
            imageVector = if (hasPermission) Icons.Default.Videocam else Icons.Default.Warning,
            contentDescription = null,
            tint = if (hasPermission) ArchDropTheme.colors.systemGray else ArchDropTheme.colors.systemOrange,
            modifier = Modifier.size(36.dp)
        )
        Text(
            text = if (hasPermission) "Continuity Camera Ready" else "Camera Permission Required",
            style = ArchDropTheme.typography.subheadline,
            fontWeight = FontWeight.SemiBold,
            color = ArchDropTheme.colors.label
        )
        Text(
            text = if (hasPermission) {
                "Tap 'Start Camera Stream' to connect to Arch Linux"
            } else {
                "Tap to grant camera access"
            },
            style = ArchDropTheme.typography.caption1,
            color = ArchDropTheme.colors.secondaryLabel,
            textAlign = TextAlign.Center
        )
    }
}

/**
 * Camera permission required card.
 */
@Composable
private fun CameraPermissionCard(onGrantPermission: () -> Unit) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(ArchDropTheme.shapes.card)
            .background(ArchDropTheme.colors.secondarySystemGroupedBackground)
            .cupertinoHairline(shape = ArchDropTheme.shapes.card)
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp)
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            Icon(
                imageVector = Icons.Default.Warning,
                contentDescription = null,
                tint = ArchDropTheme.colors.systemOrange,
                modifier = Modifier.size(20.dp)
            )
            Text(
                text = "Camera Access Required",
                style = ArchDropTheme.typography.headline,
                fontWeight = FontWeight.SemiBold,
                color = ArchDropTheme.colors.label
            )
        }
        Text(
            text = "Arch Drop requires camera access to stream live video frames to your Arch Linux desktop as a virtual webcam.",
            style = ArchDropTheme.typography.subheadline,
            color = ArchDropTheme.colors.secondaryLabel
        )
        CupertinoButton(
            onClick = onGrantPermission,
            style = CupertinoButtonStyle.FILLED,
            shape = ArchDropTheme.shapes.buttonMedium
        ) {
            Text("Grant Camera Permission", style = ArchDropTheme.typography.subheadline)
        }
    }
}

/**
 * Inset grouped camera settings and network diagnostics card.
 */
@Composable
private fun InsetGroupedSettingsCard(
    hostInput: String,
    onHostInputChange: (String) -> Unit,
    targetPort: Int,
    selectedResolution: WebcamResolution,
    selectedLens: WebcamLens
) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(ArchDropTheme.shapes.card)
            .background(ArchDropTheme.colors.secondarySystemGroupedBackground)
            .cupertinoHairline(shape = ArchDropTheme.shapes.card)
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp)
    ) {
        Text(
            text = "Connection Diagnostics & Host",
            style = ArchDropTheme.typography.headline,
            fontWeight = FontWeight.SemiBold,
            color = ArchDropTheme.colors.label
        )

        CupertinoTextField(
            value = hostInput,
            onValueChange = onHostInputChange,
            placeholder = "10.2.0.2 (Port $targetPort)",
            modifier = Modifier.fillMaxWidth()
        )

        CupertinoDivider()

        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween
        ) {
            Text("Virtual Sink", style = ArchDropTheme.typography.footnote, color = ArchDropTheme.colors.secondaryLabel)
            Text("/dev/videoX (v4l2loopback)", style = ArchDropTheme.typography.footnote, color = ArchDropTheme.colors.label)
        }

        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween
        ) {
            Text("Frame Format", style = ArchDropTheme.typography.footnote, color = ArchDropTheme.colors.secondaryLabel)
            Text("MJPEG over HTTP Chunked", style = ArchDropTheme.typography.footnote, color = ArchDropTheme.colors.label)
        }

        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween
        ) {
            Text("Active Lens", style = ArchDropTheme.typography.footnote, color = ArchDropTheme.colors.secondaryLabel)
            Text(selectedLens.label, style = ArchDropTheme.typography.footnote, color = ArchDropTheme.colors.label)
        }
    }
}

/**
 * Full screen OLED true black (#000000) low power overlay.
 *
 * Emits zero light on AMOLED panels to conserve battery and eliminate thermal throttling.
 * Features an anti-burn-in ultra-dim indicator and tap-to-wake gesture.
 */
@Composable
private fun CameraBlackoutOverlay(
    isStreaming: Boolean,
    fps: Double,
    targetHost: String,
    targetPort: Int,
    resolution: WebcamResolution,
    onWake: () -> Unit
) {
    // Anti-burn-in breathing pulse animation (alpha 0.25f..0.85f, 1200ms Reverse)
    val dotAlpha: Float
    if (isRobolectric) {
        dotAlpha = 0.55f
    } else {
        val transition = rememberInfiniteTransition(label = "blackoutDotPulse")
        val animatedAlpha by transition.animateFloat(
            initialValue = 0.25f,
            targetValue = 0.85f,
            animationSpec = infiniteRepeatable(
                animation = tween(durationMillis = 1200),
                repeatMode = RepeatMode.Reverse
            ),
            label = "blackoutDotAlpha"
        )
        dotAlpha = animatedAlpha
    }

    val dotColor = if (isStreaming) {
        Color(0xFF30D158).copy(alpha = dotAlpha)
    } else {
        Color(0x33FFFFFF)
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(Color.Black)
            .clickable { onWake() },
        contentAlignment = Alignment.Center
    ) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(14.dp),
            modifier = Modifier.padding(32.dp)
        ) {
            // Anti-burn-in animated breathing dot (pulsing between 0.25f and 0.85f alpha)
            Box(
                modifier = Modifier
                    .size(10.dp)
                    .clip(CircleShape)
                    .background(dotColor)
            )

            Text(
                text = "OLED Blackout Active",
                style = ArchDropTheme.typography.headline,
                fontWeight = FontWeight.SemiBold,
                color = Color(0x66FFFFFF) // 40% dim white
            )

            Text(
                text = if (isStreaming) {
                    "Streaming to $targetHost:$targetPort\n${String.format(Locale.US, "%.1f", fps)} FPS • ${resolution.label.substringBefore(" ")}\n\nTap anywhere on screen to wake"
                } else {
                    "Continuity Camera Standby\n\nTap anywhere on screen to wake"
                },
                style = ArchDropTheme.typography.footnote,
                color = Color(0x33FFFFFF), // 20% dim white for burn-in prevention
                textAlign = TextAlign.Center
            )
        }
    }
}
