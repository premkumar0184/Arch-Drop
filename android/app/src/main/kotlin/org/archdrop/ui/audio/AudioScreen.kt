package org.archdrop.ui.audio

import android.os.Build
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.VolumeDown
import androidx.compose.material.icons.automirrored.filled.VolumeOff
import androidx.compose.material.icons.automirrored.filled.VolumeUp
import androidx.compose.material.icons.filled.Airplay
import androidx.compose.material.icons.filled.BluetoothAudio
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.CloudOff
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Error
import androidx.compose.material.icons.filled.GraphicEq
import androidx.compose.material.icons.filled.Headphones
import androidx.compose.material.icons.filled.HighQuality
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.SpeakerGroup
import androidx.compose.material.icons.filled.Speed
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.filled.Sync
import androidx.compose.material.icons.filled.Tune
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import org.archdrop.ui.theme.CupertinoActivityIndicator
import org.archdrop.ui.theme.CupertinoAlertDialog
import org.archdrop.ui.theme.CupertinoButton
import org.archdrop.ui.theme.CupertinoButtonStyle
import org.archdrop.ui.theme.CupertinoCard
import org.archdrop.ui.theme.CupertinoColors
import org.archdrop.ui.theme.CupertinoDivider
import org.archdrop.ui.theme.CupertinoIconButton
import org.archdrop.ui.theme.CupertinoSlider
import org.archdrop.ui.theme.CupertinoSwitch
import org.archdrop.ui.theme.CupertinoTextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.archdrop.audio.AudioCodec
import org.archdrop.audio.AudioReceiverState
import org.archdrop.audio.JitterTelemetry
import org.archdrop.ui.theme.ArchDropTheme
import org.archdrop.ui.theme.cupertinoGlassPill
import org.archdrop.ui.theme.cupertinoHairline
import org.archdrop.ui.theme.cupertinoTouchTarget
import java.util.Locale
import kotlin.math.roundToInt
import kotlin.math.sin

/**
 * Stateful entrypoint for Arch Drop AirPlay-Inspired Audio Streaming Screen.
 */
@Composable
fun AudioScreen(
    viewModel: AudioViewModel,
    onNavigateBack: (() -> Unit)? = null,
    modifier: Modifier = Modifier
) {
    val isStreaming by viewModel.isStreaming.collectAsState()
    val receiverState by viewModel.receiverState.collectAsState()
    val telemetry by viewModel.telemetry.collectAsState()
    val targetHost by viewModel.targetHost.collectAsState()
    val targetPort by viewModel.targetPort.collectAsState()
    val selectedCodec by viewModel.selectedCodec.collectAsState()
    val availableDevices by viewModel.availableDevices.collectAsState()
    val selectedDevice by viewModel.selectedDevice.collectAsState()
    val volumeBoost by viewModel.volumeBoost.collectAsState()
    val isMuted by viewModel.isMuted.collectAsState()

    var showHostDialog by remember { mutableStateOf(false) }
    var hostInput by remember(targetHost) { mutableStateOf(targetHost) }

    AudioScreenContent(
        isStreaming = isStreaming,
        receiverState = receiverState,
        telemetry = telemetry,
        targetHost = targetHost,
        targetPort = targetPort,
        selectedCodec = selectedCodec,
        availableDevices = availableDevices,
        selectedDevice = selectedDevice,
        volumeBoost = volumeBoost,
        isMuted = isMuted,
        onStartStreaming = { viewModel.startStreaming() },
        onStopStreaming = { viewModel.stopStreaming() },
        onEditHostClicked = {
            hostInput = targetHost
            showHostDialog = true
        },
        onCodecSelected = { viewModel.setCodec(it) },
        onDeviceSelected = { viewModel.selectDevice(it) },
        onVolumeBoostChanged = { viewModel.setVolumeBoost(it) },
        onMuteToggled = { viewModel.setMute(it) },
        onNavigateBack = onNavigateBack,
        modifier = modifier
    )

    if (showHostDialog) {
        CupertinoAlertDialog(
            onDismissRequest = { showHostDialog = false },
            title = "Configure Desktop Host",
            message = "Defaults to host WireGuard IP (10.2.0.2) on port $targetPort.",
            content = {
                CupertinoTextField(
                    value = hostInput,
                    onValueChange = { hostInput = it },
                    placeholder = "10.2.0.2",
                    singleLine = true
                )
            },
            confirmTitle = "Save",
            onConfirm = {
                viewModel.setTargetHost(hostInput)
                showHostDialog = false
            },
            cancelTitle = "Cancel",
            onCancel = { showHostDialog = false }
        )
    }
}

/**
 * Pure stateless AirPlay-inspired Audio View Compose layout.
 * When embedded in ContinuityScreen ([onNavigateBack] is null), avoids rendering nested Scaffolds or redundant TopAppBars.
 */
@Composable
fun AudioScreenContent(
    isStreaming: Boolean,
    receiverState: AudioReceiverState,
    telemetry: JitterTelemetry,
    targetHost: String,
    targetPort: Int,
    selectedCodec: AudioCodec,
    availableDevices: List<AudioDeviceUiModel>,
    selectedDevice: AudioDeviceUiModel?,
    volumeBoost: Float,
    isMuted: Boolean,
    onStartStreaming: () -> Unit,
    onStopStreaming: () -> Unit,
    onEditHostClicked: () -> Unit,
    onCodecSelected: (AudioCodec) -> Unit,
    onDeviceSelected: (AudioDeviceUiModel?) -> Unit,
    onVolumeBoostChanged: (Float) -> Unit,
    onMuteToggled: (Boolean) -> Unit,
    onNavigateBack: (() -> Unit)? = null,
    modifier: Modifier = Modifier
) {
    val scrollableContent = @Composable {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp)
        ) {
            // 1. Hero Connection & Equalizer Card
            AudioHeroConnectionCard(
                isStreaming = isStreaming,
                receiverState = receiverState,
                telemetry = telemetry,
                targetHost = targetHost,
                targetPort = targetPort,
                onStartStreaming = onStartStreaming,
                onStopStreaming = onStopStreaming,
                onEditHostClicked = onEditHostClicked
            )

            // 2. AirPlay-Inspired Route Picker Card
            AirPlayRoutePickerCard(
                devices = availableDevices,
                selectedDevice = selectedDevice,
                onDeviceSelected = onDeviceSelected
            )

            // 3. Cupertino Volume Booster & Codec Controls Card
            CupertinoVolumeBoosterCard(
                volumeBoost = volumeBoost,
                isMuted = isMuted,
                selectedCodec = selectedCodec,
                isStreaming = isStreaming,
                onVolumeBoostChanged = onVolumeBoostChanged,
                onMuteToggled = onMuteToggled,
                onCodecSelected = onCodecSelected
            )

            Spacer(modifier = Modifier.height(16.dp))
        }
    }

    if (onNavigateBack != null) {
        Column(
            modifier = modifier
                .fillMaxSize()
                .background(ArchDropTheme.colors.canvasBackground)
        ) {
            // Authentic iOS Navigation Bar
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .height(44.dp)
                    .padding(horizontal = 8.dp),
                verticalAlignment = Alignment.CenterVertically
            ) {
                CupertinoIconButton(
                    onClick = onNavigateBack,
                    modifier = Modifier.size(44.dp)
                ) {
                    Icon(
                        imageVector = Icons.AutoMirrored.Filled.ArrowBack,
                        contentDescription = "Back",
                        tint = ArchDropTheme.colors.primaryAccent,
                        modifier = Modifier.size(22.dp)
                    )
                }
                Spacer(modifier = Modifier.width(4.dp))
                Column {
                    Text(
                        text = "Laptop Audio",
                        style = ArchDropTheme.typography.headline,
                        fontWeight = FontWeight.Bold,
                        color = ArchDropTheme.colors.label
                    )
                    Text(
                        text = "Wireless PipeWire Receiver",
                        style = ArchDropTheme.typography.caption1,
                        color = ArchDropTheme.colors.secondaryLabel
                    )
                }
            }
            Box(modifier = Modifier.fillMaxSize()) {
                scrollableContent()
            }
        }
    } else {
        // Embedded in Continuity Studio Hub: Clean layout without redundant TopAppBar
        Box(
            modifier = modifier
                .fillMaxSize()
                .background(ArchDropTheme.colors.canvasBackground)
        ) {
            scrollableContent()
        }
    }
}

/**
 * Inset Grouped Hero Connection Card combining status, live waveform equalizer,
 * telemetry capsule, and prominent pill start/stop button.
 */
@Composable
fun AudioHeroConnectionCard(
    isStreaming: Boolean,
    receiverState: AudioReceiverState,
    telemetry: JitterTelemetry,
    targetHost: String,
    targetPort: Int,
    onStartStreaming: () -> Unit,
    onStopStreaming: () -> Unit,
    onEditHostClicked: () -> Unit,
    modifier: Modifier = Modifier
) {
    CupertinoCard(
        shape = ArchDropTheme.shapes.card,
        modifier = modifier
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp)
        ) {
            // Header: Glyphs + Title + Connection State Badge
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp)
                ) {
                    Icon(
                        imageVector = Icons.Default.Airplay,
                        contentDescription = null,
                        tint = ArchDropTheme.colors.primaryAccent,
                        modifier = Modifier.size(20.dp)
                    )
                    Text(
                        text = "Desktop Audio Stream",
                        style = ArchDropTheme.typography.headline,
                        fontWeight = FontWeight.SemiBold,
                        color = ArchDropTheme.colors.label
                    )
                }

                AudioConnectionStatusBadge(state = receiverState)
            }

            // Target host & port display chip
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(ArchDropTheme.shapes.buttonSmall)
                    .background(ArchDropTheme.colors.tertiarySystemGroupedBackground)
                    .cupertinoHairline(shape = ArchDropTheme.shapes.buttonSmall)
                    .clickable(onClick = onEditHostClicked)
                    .padding(horizontal = 12.dp, vertical = 8.dp),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Column {
                    Text(
                        text = "Target Ingest Host",
                        style = ArchDropTheme.typography.caption2,
                        color = ArchDropTheme.colors.secondaryLabel
                    )
                    Text(
                        text = "$targetHost:$targetPort",
                        style = ArchDropTheme.typography.footnote,
                        fontFamily = FontFamily.Monospace,
                        fontWeight = FontWeight.SemiBold,
                        color = ArchDropTheme.colors.label
                    )
                }
                Icon(
                    imageVector = Icons.Default.Edit,
                    contentDescription = "Edit Host",
                    tint = ArchDropTheme.colors.secondaryLabel,
                    modifier = Modifier.size(16.dp)
                )
            }

            // 11-Bar Sinusoidal Equalizer Visualizer
            AirPlayWaveformVisualizer(
                isStreaming = isStreaming,
                telemetry = telemetry,
                modifier = Modifier.fillMaxWidth()
            )

            // Compact Telemetry Capsule
            AirPlayTelemetryCapsule(
                telemetry = telemetry,
                isStreaming = isStreaming,
                modifier = Modifier.fillMaxWidth()
            )

            if (receiverState is AudioReceiverState.Error) {
                Text(
                    text = receiverState.message,
                    style = ArchDropTheme.typography.caption1,
                    color = ArchDropTheme.colors.systemRed,
                    modifier = Modifier.padding(horizontal = 4.dp)
                )
            }

            // Prominent Apple HIG Pill Action Button
            CupertinoButton(
                onClick = {
                    if (isStreaming || receiverState is AudioReceiverState.Connecting) {
                        onStopStreaming()
                    } else {
                        onStartStreaming()
                    }
                },
                style = if (isStreaming) CupertinoButtonStyle.DESTRUCTIVE else CupertinoButtonStyle.FILLED,
                shape = ArchDropTheme.shapes.buttonLarge,
                modifier = Modifier
                    .fillMaxWidth()
                    .height(52.dp)
            ) {
                if (receiverState is AudioReceiverState.Connecting) {
                    CupertinoActivityIndicator(
                        modifier = Modifier.size(20.dp),
                        color = Color.White,
                        strokeWidth = 2.dp
                    )
                    Spacer(modifier = Modifier.width(10.dp))
                    Text(
                        text = "Connecting to Desktop...",
                        style = ArchDropTheme.typography.headline,
                        fontWeight = FontWeight.SemiBold
                    )
                } else if (isStreaming) {
                    Icon(
                        imageVector = Icons.Default.Stop,
                        contentDescription = null,
                        modifier = Modifier.size(20.dp)
                    )
                    Spacer(modifier = Modifier.width(10.dp))
                    Text(
                        text = "Stop Listening",
                        style = ArchDropTheme.typography.headline,
                        fontWeight = FontWeight.SemiBold
                    )
                } else {
                    Icon(
                        imageVector = Icons.Default.Headphones,
                        contentDescription = null,
                        modifier = Modifier.size(20.dp)
                    )
                    Spacer(modifier = Modifier.width(10.dp))
                    Text(
                        text = "Listen to Laptop Audio",
                        style = ArchDropTheme.typography.headline,
                        fontWeight = FontWeight.SemiBold
                    )
                }
            }
        }
    }
}

/**
 * 11-bar sinusoidal equalizer visualizer inspired by Apple AirPlay and Siri.
 *
 * Renders exactly 11 continuous squircle capsule bars (`RoundedCornerShape(percent = 50)`),
 * traveling wave with 1200ms phase animation, interpolating from Arch Electric Blue (`#0A84FF`)
 * to Apple Mint Green (`#30D158`).
 */
@Composable
fun AirPlayWaveformVisualizer(
    isStreaming: Boolean,
    telemetry: JitterTelemetry? = null,
    modifier: Modifier = Modifier
) {
    val barCount = 11
    val isRobolectric = Build.FINGERPRINT == "robolectric" || Build.HARDWARE == "robolectric"
    val phase: Float
    if (isRobolectric) {
        phase = 0f
    } else {
        val infiniteTransition = rememberInfiniteTransition(label = "AirPlayWaveform")
        val animatedPhase by infiniteTransition.animateFloat(
            initialValue = 0f,
            targetValue = (2 * Math.PI).toFloat(),
            animationSpec = infiniteRepeatable(
                animation = tween(1200, easing = LinearEasing),
                repeatMode = RepeatMode.Restart
            ),
            label = "WavePhase"
        )
        phase = animatedPhase
    }

    Box(
        modifier = modifier
            .fillMaxWidth()
            .height(56.dp)
            .clip(ArchDropTheme.shapes.card)
            .background(ArchDropTheme.colors.tertiarySystemGroupedBackground)
            .cupertinoHairline(shape = ArchDropTheme.shapes.card)
            .padding(horizontal = 16.dp, vertical = 8.dp),
        contentAlignment = Alignment.Center
    ) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically
        ) {
            for (i in 0 until barCount) {
                val heightFraction = if (isStreaming) {
                    val normalized = (sin(phase + i * 0.55f) + 1f) / 2f
                    0.18f + 0.82f * normalized
                } else {
                    0.12f
                }

                val lerpFraction = i.toFloat() / (barCount - 1).toFloat()
                val barColor = if (isStreaming) {
                    lerp(ArchDropTheme.colors.primaryAccent, ArchDropTheme.colors.systemGreen, lerpFraction)
                } else {
                    ArchDropTheme.colors.quaternaryLabel
                }

                Box(
                    modifier = Modifier
                        .weight(1f)
                        .padding(horizontal = 3.dp)
                        .height((40 * heightFraction).dp)
                        .clip(RoundedCornerShape(percent = 50))
                        .background(barColor)
                )
            }
        }
    }
}

/**
 * Backward compatibility alias for legacy call sites.
 */
@Composable
fun PulsatingWaveform(
    isStreaming: Boolean,
    modifier: Modifier = Modifier
) {
    AirPlayWaveformVisualizer(
        isStreaming = isStreaming,
        modifier = modifier
    )
}

/**
 * Connection status pill indicator.
 */
@Composable
fun AudioConnectionStatusBadge(state: AudioReceiverState) {
    val (bgColor, textColor, label, icon) = when (state) {
        is AudioReceiverState.Streaming -> StatusPill(
            ArchDropTheme.colors.systemGreen.copy(alpha = 0.2f),
            ArchDropTheme.colors.systemGreen,
            "Live Streaming",
            Icons.Default.GraphicEq
        )
        is AudioReceiverState.Connecting -> StatusPill(
            ArchDropTheme.colors.systemOrange.copy(alpha = 0.2f),
            ArchDropTheme.colors.systemOrange,
            "Connecting...",
            Icons.Default.Sync
        )
        is AudioReceiverState.Paused -> StatusPill(
            ArchDropTheme.colors.systemOrange.copy(alpha = 0.2f),
            ArchDropTheme.colors.systemOrange,
            "Paused",
            Icons.Default.Info
        )
        is AudioReceiverState.Error -> StatusPill(
            ArchDropTheme.colors.systemRed.copy(alpha = 0.2f),
            ArchDropTheme.colors.systemRed,
            "Error",
            Icons.Default.Error
        )
        is AudioReceiverState.Disconnected -> StatusPill(
            ArchDropTheme.colors.tertiarySystemGroupedBackground,
            ArchDropTheme.colors.secondaryLabel,
            "Disconnected",
            Icons.Default.CloudOff
        )
        is AudioReceiverState.Idle -> StatusPill(
            ArchDropTheme.colors.primaryAccent.copy(alpha = 0.15f),
            ArchDropTheme.colors.primaryAccent,
            "Ready",
            Icons.Default.Headphones
        )
    }

    Box(
        modifier = Modifier
            .clip(ArchDropTheme.shapes.buttonSmall)
            .background(bgColor)
    ) {
        Row(
            modifier = Modifier.padding(horizontal = 10.dp, vertical = 5.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp)
        ) {
            Icon(
                imageVector = icon,
                contentDescription = null,
                tint = textColor,
                modifier = Modifier.size(14.dp)
            )
            Text(
                text = label,
                style = ArchDropTheme.typography.caption1,
                fontWeight = FontWeight.Bold,
                color = textColor
            )
        }
    }
}

private data class StatusPill(
    val bg: Color,
    val text: Color,
    val label: String,
    val icon: ImageVector
)

/**
 * Resolves telemetry status color and description based on streaming state and buffer depth.
 * - Standby: Gray when not streaming
 * - Optimal: 30.0ms – 60.0ms (Green)
 * - Warning: 20.0ms – 30.0ms or > 60.0ms (Orange)
 * - Starvation: < 20.0ms (Red)
 */
fun getTelemetryStatus(
    isStreaming: Boolean,
    bufferMs: Double,
    colors: CupertinoColors
): Pair<Color, String> = when {
    !isStreaming -> Pair(colors.systemGray, "Standby")
    bufferMs in 30.0..60.0 -> Pair(colors.systemGreen, "Optimal")
    bufferMs in 20.0..30.0 || bufferMs > 60.0 -> Pair(colors.systemOrange, "Warning")
    else -> Pair(colors.systemRed, "Starvation")
}

/**
 * Compact Frosted Glass Telemetry Capsule displaying buffer sweet spot (30-60ms green),
 * jitter, bitrate, and packet drops.
 */
@Composable
fun AirPlayTelemetryCapsule(
    telemetry: JitterTelemetry,
    isStreaming: Boolean,
    modifier: Modifier = Modifier
) {
    val bufferMs = telemetry.bufferDepthMs
    val (statusDotColor, _) = getTelemetryStatus(isStreaming, bufferMs, ArchDropTheme.colors)

    Box(
        modifier = modifier
            .fillMaxWidth()
            .cupertinoGlassPill(alpha = 0.5f)
            .padding(horizontal = 14.dp, vertical = 8.dp)
    ) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically
        ) {
            // Buffer Depth with Status Dot
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp)
            ) {
                Box(
                    modifier = Modifier
                        .size(8.dp)
                        .clip(CircleShape)
                        .background(statusDotColor)
                )
                Text(
                    text = "${bufferMs.roundToInt()} ms buffer",
                    style = ArchDropTheme.typography.caption1,
                    fontWeight = FontWeight.SemiBold,
                    color = ArchDropTheme.colors.label
                )
            }

            // Jitter / Latency
            Text(
                text = String.format(Locale.US, "±%.1f ms jitter", if (isStreaming) telemetry.jitterMs else 0.0),
                style = ArchDropTheme.typography.caption1,
                color = ArchDropTheme.colors.secondaryLabel
            )

            // Bitrate
            Text(
                text = String.format(Locale.US, "%.0f kbps", if (isStreaming) telemetry.bitrateKbps else 0.0),
                style = ArchDropTheme.typography.caption1,
                color = ArchDropTheme.colors.secondaryLabel
            )

            // Packet Loss
            Text(
                text = if (telemetry.packetsLost > 0) "${telemetry.packetsLost} drops" else "0 drops",
                style = ArchDropTheme.typography.caption1,
                fontWeight = if (telemetry.packetsLost > 0) FontWeight.Bold else FontWeight.Normal,
                color = if (telemetry.packetsLost > 0) ArchDropTheme.colors.systemRed else ArchDropTheme.colors.systemGreen
            )
        }
    }
}

/**
 * Backward compatibility alias for legacy tests/callsites.
 */
@Composable
fun LiveTelemetrySection(
    telemetry: JitterTelemetry,
    isStreaming: Boolean,
    modifier: Modifier = Modifier
) {
    AirPlayTelemetryCapsule(
        telemetry = telemetry,
        isStreaming = isStreaming,
        modifier = modifier
    )
}

/**
 * AirPlay-inspired Inset Grouped Route Picker Card.
 *
 * Displays physical hardware output sinks with min 44pt touch targets,
 * an iconic Apple Electric Blue checkmark on the active route, and Bluetooth latency advisory callout.
 */
@Composable
fun AirPlayRoutePickerCard(
    devices: List<AudioDeviceUiModel>,
    selectedDevice: AudioDeviceUiModel?,
    onDeviceSelected: (AudioDeviceUiModel) -> Unit,
    modifier: Modifier = Modifier
) {
    CupertinoCard(
        shape = ArchDropTheme.shapes.card,
        modifier = modifier
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp)
        ) {
            // Header
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp)
            ) {
                Icon(
                    imageVector = Icons.AutoMirrored.Filled.VolumeUp,
                    contentDescription = null,
                    tint = ArchDropTheme.colors.primaryAccent,
                    modifier = Modifier.size(20.dp)
                )
                Column {
                    Text(
                        text = "Audio Output Route",
                        style = ArchDropTheme.typography.headline,
                        color = ArchDropTheme.colors.label
                    )
                    Text(
                        text = "Route decoded audio to hardware sinks",
                        style = ArchDropTheme.typography.caption1,
                        color = ArchDropTheme.colors.secondaryLabel
                    )
                }
            }

            // Inset Grouped Device Rows
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(ArchDropTheme.shapes.buttonMedium)
                    .background(ArchDropTheme.colors.tertiarySystemGroupedBackground)
            ) {
                devices.forEachIndexed { index, device ->
                    val isSelected = (selectedDevice?.id == device.id)
                    val icon = when (device.type) {
                        AudioDeviceType.SPEAKER -> Icons.AutoMirrored.Filled.VolumeUp
                        AudioDeviceType.WIRED -> Icons.Default.Headphones
                        AudioDeviceType.BLUETOOTH -> Icons.Default.BluetoothAudio
                        AudioDeviceType.USB -> Icons.Default.Headphones
                        AudioDeviceType.DEFAULT -> Icons.Default.SpeakerGroup
                    }

                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .cupertinoTouchTarget()
                            .clickable { onDeviceSelected(device) }
                            .padding(horizontal = 14.dp, vertical = 12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.SpaceBetween
                    ) {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(12.dp),
                            modifier = Modifier.weight(1f)
                        ) {
                            Icon(
                                imageVector = icon,
                                contentDescription = null,
                                tint = if (isSelected) ArchDropTheme.colors.primaryAccent else ArchDropTheme.colors.secondaryLabel,
                                modifier = Modifier.size(20.dp)
                            )
                            Column {
                                Text(
                                    text = device.name,
                                    style = ArchDropTheme.typography.body,
                                    color = if (isSelected) ArchDropTheme.colors.primaryAccent else ArchDropTheme.colors.label,
                                    fontWeight = if (isSelected) FontWeight.SemiBold else FontWeight.Normal
                                )
                                Text(
                                    text = when (device.type) {
                                        AudioDeviceType.SPEAKER -> "Phone Speaker"
                                        AudioDeviceType.WIRED -> "Wired Headphones"
                                        AudioDeviceType.BLUETOOTH -> "Bluetooth Accessory"
                                        AudioDeviceType.USB -> "USB DAC"
                                        AudioDeviceType.DEFAULT -> "System Default"
                                    },
                                    style = ArchDropTheme.typography.caption1,
                                    color = ArchDropTheme.colors.secondaryLabel
                                )
                            }
                        }

                        // AirPlay Iconic Electric Blue Checkmark
                        if (isSelected) {
                            Icon(
                                imageVector = Icons.Default.Check,
                                contentDescription = "Active Route",
                                tint = ArchDropTheme.colors.primaryAccent,
                                modifier = Modifier.size(20.dp)
                            )
                        }
                    }

                    if (index < devices.size - 1) {
                        CupertinoDivider(
                            modifier = Modifier.padding(start = 46.dp),
                            color = ArchDropTheme.colors.separator,
                            thickness = 0.5.dp
                        )
                    }
                }
            }

            // Bluetooth Latency Advisory Callout
            if (selectedDevice?.isBluetooth == true) {
                AirPlayBluetoothAdvisoryCallout()
            }
        }
    }
}

/**
 * Backward compatibility alias for device selector card.
 */
@Composable
fun AudioOutputDeviceCard(
    devices: List<AudioDeviceUiModel>,
    selectedDevice: AudioDeviceUiModel?,
    onDeviceSelected: (AudioDeviceUiModel) -> Unit,
    modifier: Modifier = Modifier
) {
    AirPlayRoutePickerCard(
        devices = devices,
        selectedDevice = selectedDevice,
        onDeviceSelected = onDeviceSelected,
        modifier = modifier
    )
}

/**
 * Bluetooth A2DP Latency Advisory Callout.
 */
@Composable
fun AirPlayBluetoothAdvisoryCallout(modifier: Modifier = Modifier) {
    Row(
        modifier = modifier
            .fillMaxWidth()
            .clip(ArchDropTheme.shapes.buttonSmall)
            .background(ArchDropTheme.colors.tertiarySystemGroupedBackground)
            .cupertinoHairline(shape = ArchDropTheme.shapes.buttonSmall)
            .padding(12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp)
    ) {
        Icon(
            imageVector = Icons.Default.Info,
            contentDescription = null,
            tint = ArchDropTheme.colors.primaryAccent,
            modifier = Modifier.size(18.dp)
        )
        Text(
            text = "Bluetooth A2DP introduces ~100–200 ms physical over-the-air latency. For low-latency sync (<60 ms), use the phone speaker or wired headphones.",
            style = ArchDropTheme.typography.caption1,
            color = ArchDropTheme.colors.secondaryLabel
        )
    }
}

/**
 * Cupertino Volume Booster and Codec Card with Haptic Detent Ticks.
 *
 * Supports 100%–200% digital gain with tactile ticks at 100% unity gain and 25% steps,
 * soft-saturation clipping warning callout, and clean mute toggle.
 */
@Composable
fun CupertinoVolumeBoosterCard(
    volumeBoost: Float,
    isMuted: Boolean,
    selectedCodec: AudioCodec,
    isStreaming: Boolean,
    onVolumeBoostChanged: (Float) -> Unit,
    onMuteToggled: (Boolean) -> Unit,
    onCodecSelected: (AudioCodec) -> Unit,
    modifier: Modifier = Modifier
) {
    val haptic = LocalHapticFeedback.current
    var lastHapticStep by remember { mutableIntStateOf((volumeBoost * 100).roundToInt()) }
    val boostPercent = (volumeBoost * 100).roundToInt()
    val isBoostActive = boostPercent > 100

    CupertinoCard(
        shape = ArchDropTheme.shapes.card,
        modifier = modifier
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp)
        ) {
            // Header
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp)
                ) {
                    Icon(
                        imageVector = if (isMuted) Icons.AutoMirrored.Filled.VolumeOff else Icons.AutoMirrored.Filled.VolumeUp,
                        contentDescription = null,
                        tint = if (isMuted) ArchDropTheme.colors.secondaryLabel else ArchDropTheme.colors.primaryAccent,
                        modifier = Modifier.size(20.dp)
                    )
                    Text(
                        text = "Volume & Digital Gain",
                        style = ArchDropTheme.typography.headline,
                        color = ArchDropTheme.colors.label
                    )
                }

                // Status pill
                Box(
                    modifier = Modifier
                        .clip(ArchDropTheme.shapes.buttonSmall)
                        .background(
                            when {
                                isMuted -> ArchDropTheme.colors.systemRed.copy(alpha = 0.2f)
                                isBoostActive -> ArchDropTheme.colors.systemOrange.copy(alpha = 0.2f)
                                else -> ArchDropTheme.colors.tertiarySystemGroupedBackground
                            }
                        )
                ) {
                    Text(
                        text = if (isMuted) "MUTED" else "$boostPercent%",
                        style = ArchDropTheme.typography.caption1,
                        fontWeight = FontWeight.Bold,
                        color = when {
                            isMuted -> ArchDropTheme.colors.systemRed
                            isBoostActive -> ArchDropTheme.colors.systemOrange
                            else -> ArchDropTheme.colors.primaryAccent
                        },
                        modifier = Modifier.padding(horizontal = 8.dp, vertical = 3.dp)
                    )
                }
            }

            // Slider with Haptic Detent Logic (Unity gain at 100%, and 25% increments)
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp)
            ) {
                Icon(
                    imageVector = Icons.AutoMirrored.Filled.VolumeDown,
                    contentDescription = null,
                    tint = ArchDropTheme.colors.secondaryLabel,
                    modifier = Modifier.size(16.dp)
                )

                CupertinoSlider(
                    value = volumeBoost,
                    onValueChange = { newBoost ->
                        val currentPercent = (newBoost * 100).roundToInt()
                        if (currentPercent != lastHapticStep) {
                            if (currentPercent == 100 || (currentPercent % 25 == 0 && currentPercent != lastHapticStep)) {
                                haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                            }
                            lastHapticStep = currentPercent
                        }
                        onVolumeBoostChanged(newBoost)
                    },
                    valueRange = 1.0f..2.0f,
                    enabled = !isMuted,
                    activeTrackColor = if (isBoostActive) ArchDropTheme.colors.systemOrange else ArchDropTheme.colors.primaryAccent,
                    inactiveTrackColor = ArchDropTheme.colors.tertiarySystemGroupedBackground,
                    modifier = Modifier.weight(1f)
                )

                Icon(
                    imageVector = Icons.AutoMirrored.Filled.VolumeUp,
                    contentDescription = null,
                    tint = ArchDropTheme.colors.secondaryLabel,
                    modifier = Modifier.size(16.dp)
                )
            }

            // Saturation Warning Callout
            AnimatedVisibility(
                visible = isBoostActive && !isMuted,
                enter = fadeIn() + expandVertically(),
                exit = fadeOut() + shrinkVertically()
            ) {
                AirPlaySaturationWarningCallout()
            }

            CupertinoDivider()

            // Mute Switch Row
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .cupertinoTouchTarget(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text(
                        text = "Mute Audio Output",
                        style = ArchDropTheme.typography.body,
                        color = ArchDropTheme.colors.label
                    )
                    Text(
                        text = "Zeroes audio output in real-time without terminating session",
                        style = ArchDropTheme.typography.footnote,
                        color = ArchDropTheme.colors.secondaryLabel
                    )
                }

                CupertinoSwitch(
                    checked = isMuted,
                    onCheckedChange = onMuteToggled,
                    activeColor = ArchDropTheme.colors.systemRed
                )
            }

            CupertinoDivider()

            // Codec Selection Section
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    text = "Capture & Transmission Codec",
                    style = ArchDropTheme.typography.subheadline,
                    fontWeight = FontWeight.SemiBold,
                    color = ArchDropTheme.colors.label
                )

                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp)
                ) {
                    // Opus Option
                    Box(
                        modifier = Modifier
                            .weight(1f)
                            .defaultMinSize(minHeight = 44.dp)
                            .clip(ArchDropTheme.shapes.buttonMedium)
                            .background(
                                if (selectedCodec == AudioCodec.OPUS) ArchDropTheme.colors.primaryAccent.copy(alpha = 0.15f)
                                else ArchDropTheme.colors.tertiarySystemGroupedBackground
                            )
                            .border(
                                width = if (selectedCodec == AudioCodec.OPUS) 1.5.dp else 0.5.dp,
                                color = if (selectedCodec == AudioCodec.OPUS) ArchDropTheme.colors.primaryAccent else ArchDropTheme.colors.hairlineBorder,
                                shape = ArchDropTheme.shapes.buttonMedium
                            )
                            .clickable { onCodecSelected(AudioCodec.OPUS) }
                            .padding(horizontal = 10.dp, vertical = 8.dp),
                        contentAlignment = Alignment.CenterStart
                    ) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Icon(
                                imageVector = if (selectedCodec == AudioCodec.OPUS) Icons.Default.CheckCircle else Icons.Default.Speed,
                                contentDescription = null,
                                modifier = Modifier.size(16.dp),
                                tint = if (selectedCodec == AudioCodec.OPUS) ArchDropTheme.colors.primaryAccent else ArchDropTheme.colors.secondaryLabel
                            )
                            Spacer(modifier = Modifier.width(6.dp))
                            Column {
                                Text("Opus (Low-Latency)", fontWeight = FontWeight.SemiBold, fontSize = 12.sp, color = ArchDropTheme.colors.label)
                                Text("96 kbps • Decoded", fontSize = 10.sp, color = ArchDropTheme.colors.secondaryLabel)
                            }
                        }
                    }

                    // PCM Option
                    Box(
                        modifier = Modifier
                            .weight(1f)
                            .defaultMinSize(minHeight = 44.dp)
                            .clip(ArchDropTheme.shapes.buttonMedium)
                            .background(
                                if (selectedCodec == AudioCodec.PCM) ArchDropTheme.colors.primaryAccent.copy(alpha = 0.15f)
                                else ArchDropTheme.colors.tertiarySystemGroupedBackground
                            )
                            .border(
                                width = if (selectedCodec == AudioCodec.PCM) 1.5.dp else 0.5.dp,
                                color = if (selectedCodec == AudioCodec.PCM) ArchDropTheme.colors.primaryAccent else ArchDropTheme.colors.hairlineBorder,
                                shape = ArchDropTheme.shapes.buttonMedium
                            )
                            .clickable { onCodecSelected(AudioCodec.PCM) }
                            .padding(horizontal = 10.dp, vertical = 8.dp),
                        contentAlignment = Alignment.CenterStart
                    ) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Icon(
                                imageVector = if (selectedCodec == AudioCodec.PCM) Icons.Default.CheckCircle else Icons.Default.HighQuality,
                                contentDescription = null,
                                modifier = Modifier.size(16.dp),
                                tint = if (selectedCodec == AudioCodec.PCM) ArchDropTheme.colors.primaryAccent else ArchDropTheme.colors.secondaryLabel
                            )
                            Spacer(modifier = Modifier.width(6.dp))
                            Column {
                                Text("PCM (Raw 48kHz)", fontWeight = FontWeight.SemiBold, fontSize = 12.sp, color = ArchDropTheme.colors.label)
                                Text("1536 kbps • Raw", fontSize = 10.sp, color = ArchDropTheme.colors.secondaryLabel)
                            }
                        }
                    }
                }

                if (isStreaming) {
                    Text(
                        text = "Note: Switching codecs during active playback automatically reconnects the stream.",
                        style = ArchDropTheme.typography.caption2,
                        color = ArchDropTheme.colors.secondaryLabel
                    )
                }
            }
        }
    }
}

/**
 * Backward compatibility alias for playback controls card.
 */
@Composable
fun PlaybackControlsCard(
    volumeBoost: Float,
    isMuted: Boolean,
    selectedCodec: AudioCodec,
    isStreaming: Boolean,
    onVolumeBoostChanged: (Float) -> Unit,
    onMuteToggled: (Boolean) -> Unit,
    onCodecSelected: (AudioCodec) -> Unit,
    modifier: Modifier = Modifier
) {
    CupertinoVolumeBoosterCard(
        volumeBoost = volumeBoost,
        isMuted = isMuted,
        selectedCodec = selectedCodec,
        isStreaming = isStreaming,
        onVolumeBoostChanged = onVolumeBoostChanged,
        onMuteToggled = onMuteToggled,
        onCodecSelected = onCodecSelected,
        modifier = modifier
    )
}

/**
 * AirPlay Saturation Warning Callout when volume boost > 1.0f.
 */
@Composable
fun AirPlaySaturationWarningCallout(modifier: Modifier = Modifier) {
    Row(
        modifier = modifier
            .fillMaxWidth()
            .clip(ArchDropTheme.shapes.buttonSmall)
            .background(ArchDropTheme.colors.systemOrange.copy(alpha = 0.15f))
            .cupertinoHairline(shape = ArchDropTheme.shapes.buttonSmall)
            .padding(10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp)
    ) {
        Icon(
            imageVector = Icons.Default.Warning,
            contentDescription = null,
            tint = ArchDropTheme.colors.systemOrange,
            modifier = Modifier.size(16.dp)
        )
        Text(
            text = "Saturation Clamping Active: Digital amplification applies soft saturation clamping [-32768, 32767]. High-amplitude audio peaks may experience soft clipping.",
            style = ArchDropTheme.typography.caption1,
            color = ArchDropTheme.colors.systemOrange
        )
    }
}

// =============================================================================
// Jetpack Compose Previews
// =============================================================================

@Preview(name = "AudioScreen - Live Streaming (Optimal)")
@Composable
fun PreviewAudioScreenStreaming() {
    ArchDropTheme {
        AudioScreenContent(
            isStreaming = true,
            receiverState = AudioReceiverState.Streaming,
            telemetry = JitterTelemetry(
                bufferDepthMs = 42.0,
                targetDepthMs = 40.0,
                jitterMs = 2.4,
                bitrateKbps = 96.0,
                packetsReceived = 1520,
                packetsLost = 0,
                expansions = 0,
                contractions = 0,
                underruns = 0,
                overruns = 0
            ),
            targetHost = "10.2.0.2",
            targetPort = 53317,
            selectedCodec = AudioCodec.OPUS,
            availableDevices = listOf(
                AudioDeviceUiModel.SYSTEM_DEFAULT,
                AudioDeviceUiModel(1, "Phone Speaker", AudioDeviceType.SPEAKER),
                AudioDeviceUiModel(2, "AirPods Pro", AudioDeviceType.BLUETOOTH)
            ),
            selectedDevice = AudioDeviceUiModel(1, "Phone Speaker", AudioDeviceType.SPEAKER),
            volumeBoost = 1.25f,
            isMuted = false,
            onStartStreaming = {},
            onStopStreaming = {},
            onEditHostClicked = {},
            onCodecSelected = {},
            onDeviceSelected = {},
            onVolumeBoostChanged = {},
            onMuteToggled = {}
        )
    }
}

@Preview(name = "AudioScreen - Idle State")
@Composable
fun PreviewAudioScreenIdle() {
    ArchDropTheme {
        AudioScreenContent(
            isStreaming = false,
            receiverState = AudioReceiverState.Idle,
            telemetry = JitterTelemetry(
                bufferDepthMs = 0.0,
                targetDepthMs = 40.0,
                jitterMs = 0.0,
                bitrateKbps = 0.0,
                packetsReceived = 0,
                packetsLost = 0,
                expansions = 0,
                contractions = 0,
                underruns = 0,
                overruns = 0
            ),
            targetHost = "10.2.0.2",
            targetPort = 53317,
            selectedCodec = AudioCodec.OPUS,
            availableDevices = listOf(AudioDeviceUiModel.SYSTEM_DEFAULT),
            selectedDevice = AudioDeviceUiModel.SYSTEM_DEFAULT,
            volumeBoost = 1.0f,
            isMuted = false,
            onStartStreaming = {},
            onStopStreaming = {},
            onEditHostClicked = {},
            onCodecSelected = {},
            onDeviceSelected = {},
            onVolumeBoostChanged = {},
            onMuteToggled = {}
        )
    }
}
