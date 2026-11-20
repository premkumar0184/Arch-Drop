package org.archdrop.ui.dashboard

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.NearMe
import org.archdrop.ui.theme.ArchDropTheme
import org.archdrop.ui.theme.cupertinoHairline
import org.archdrop.ui.theme.cupertinoTouchTarget
import androidx.compose.material.icons.automirrored.filled.ArrowForward
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.ArrowDownward
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.Bluetooth
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.CloudDone
import androidx.compose.material.icons.filled.CloudOff
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Error
import androidx.compose.material.icons.filled.Fingerprint
import androidx.compose.material.icons.filled.Headphones
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.Hub
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.QrCodeScanner
import androidx.compose.material.icons.filled.Security
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.filled.Wifi
import androidx.compose.material.icons.filled.WifiTethering
import androidx.compose.material3.Icon
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import org.archdrop.ui.theme.CupertinoActivityIndicator
import org.archdrop.ui.theme.CupertinoAlertDialog
import org.archdrop.ui.theme.CupertinoButton
import org.archdrop.ui.theme.CupertinoButtonStyle
import org.archdrop.ui.theme.CupertinoCard
import org.archdrop.transfer.RouteState
import org.archdrop.transfer.TransportRoute
import org.archdrop.ui.theme.CupertinoConnectionBadge
import org.archdrop.ui.theme.CupertinoIconButton
import org.archdrop.ui.theme.CupertinoSwitch
import org.archdrop.ui.theme.CupertinoTextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import org.archdrop.audio.AudioReceiverState
import org.archdrop.audio.JitterTelemetry
import org.archdrop.ui.audio.AudioViewModel
import org.archdrop.ui.pairing.formatHexBlocks
import java.util.Locale

@Composable
fun DashboardScreen(
    viewModel: DashboardViewModel,
    audioViewModel: AudioViewModel? = null,
    onNavigateToAudio: () -> Unit = {},
    onOpenPairing: () -> Unit = {},
    modifier: Modifier = Modifier
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val uiState by viewModel.uiState.collectAsState()
    val snackbarHostState = remember { SnackbarHostState() }

    // Observe audio stream state if audioViewModel is provided
    val isAudioStreaming = audioViewModel?.isStreaming?.collectAsState()?.value ?: false
    val audioReceiverState = audioViewModel?.receiverState?.collectAsState()?.value ?: AudioReceiverState.Idle
    val audioTelemetry = audioViewModel?.telemetry?.collectAsState()?.value ?: JitterTelemetry(
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
    )

    var showEditAliasDialog by remember { mutableStateOf(false) }
    var aliasInput by remember { mutableStateOf("") }

    Box(modifier = modifier.fillMaxSize()) {
        LazyColumn(
            modifier = Modifier
                .fillMaxSize()
                .padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp)
        ) {
            item {
                Spacer(modifier = Modifier.height(8.dp))
                // Local Device Identity Hero Card
                IdentityHeroCard(
                    alias = uiState.deviceAlias,
                    meshKey = uiState.meshKeyHex,
                    fingerprint = uiState.fingerprint,
                    activeRoute = uiState.activeRoute,
                    routeState = uiState.routeState,
                    onEditAlias = {
                        aliasInput = uiState.deviceAlias
                        showEditAliasDialog = true
                    },
                    onCopyKey = {
                        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                        val clip = ClipData.newPlainText("Ed25519 Mesh Public Key", uiState.meshKeyHex)
                        clipboard?.setPrimaryClip(clip)
                        scope.launch {
                            snackbarHostState.showSnackbar("Ed25519 Public Key copied to clipboard")
                        }
                    },
                    onOpenPairing = onOpenPairing
                )
            }

            item {
                // Desktop Wireless Audio Streaming Quick Action Card
                AudioQuickActionCard(
                    isStreaming = isAudioStreaming,
                    connectionState = audioReceiverState,
                    telemetry = audioTelemetry,
                    onToggleStream = {
                        if (isAudioStreaming) {
                            audioViewModel?.stopStreaming()
                        } else {
                            audioViewModel?.startStreaming()
                        }
                    },
                    onOpenAudioScreen = onNavigateToAudio
                )
            }

            item {
                // Dedicated Universal Clipboard Action Card
                ClipboardActionCard(
                    isSending = uiState.isSendingClipboard,
                    onSendClipboard = {
                        scope.launch {
                            val result = viewModel.sendClipboard(context)
                            val msg = when (result) {
                                is SendClipboardResult.Empty -> "Clipboard is empty"
                                is SendClipboardResult.Success -> "Sent to desktop: ${result.summary}"
                                is SendClipboardResult.Error -> "Failed to send clipboard: ${result.message}"
                            }
                            snackbarHostState.showSnackbar(msg)
                        }
                    }
                )
            }

            item {
                // Active Connection Topology Card (3-Tier Real-Time Indicators)
                TopologyCard(
                    lohsActive = uiState.lohsActive,
                    lohsSsid = uiState.lohsSsid,
                    bleAdvertising = uiState.bleAdvertising,
                    derpConnected = uiState.derpConnected,
                    derpLatencyMs = uiState.derpLatencyMs,
                    activeRoute = uiState.activeRoute,
                    routeState = uiState.routeState,
                    onToggleHotspot = { viewModel.toggleHotspot() }
                )
            }

            item {
                // Recent Transfer History Header
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 4.dp, vertical = 4.dp),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    Text(
                        text = "Recent Transfers",
                        style = ArchDropTheme.typography.title3,
                        color = ArchDropTheme.colors.label,
                        fontWeight = FontWeight.Bold
                    )
                    Text(
                        text = "${uiState.transfers.size} items",
                        style = ArchDropTheme.typography.footnote,
                        color = ArchDropTheme.colors.secondaryLabel
                    )
                }
            }

            if (uiState.transfers.isEmpty()) {
                item {
                    EmptyTransfersCard()
                }
            } else {
                items(uiState.transfers, key = { it.eventId }) { item ->
                    TransferItemCard(transfer = item)
                }
            }

            item {
                Spacer(modifier = Modifier.height(16.dp))
            }
        }

        SnackbarHost(
            hostState = snackbarHostState,
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .padding(bottom = 16.dp)
        )

        if (showEditAliasDialog) {
            CupertinoAlertDialog(
                onDismissRequest = { showEditAliasDialog = false },
                title = "Edit Device Alias",
                message = "Set a friendly name displayed to peers when transferring files or streaming audio.",
                content = {
                    CupertinoTextField(
                        value = aliasInput,
                        onValueChange = { aliasInput = it },
                        placeholder = "Device Name",
                        singleLine = true
                    )
                },
                confirmTitle = "Save",
                onConfirm = {
                    viewModel.updateAlias(aliasInput)
                    showEditAliasDialog = false
                },
                cancelTitle = "Cancel",
                onCancel = { showEditAliasDialog = false }
            )
        }
    }
}

@Composable
fun IdentityHeroCard(
    alias: String,
    meshKey: String,
    fingerprint: String,
    activeRoute: TransportRoute? = null,
    routeState: RouteState = RouteState.IDLE,
    onEditAlias: () -> Unit,
    onCopyKey: () -> Unit,
    onOpenPairing: () -> Unit = {}
) = AirDropRadarHeroCard(
    alias = alias,
    meshKey = meshKey,
    fingerprint = fingerprint,
    activeRoute = activeRoute,
    routeState = routeState,
    onEditAlias = onEditAlias,
    onCopyKey = onCopyKey,
    onOpenPairing = onOpenPairing
)

@Composable
fun AirDropRadarHeroCard(
    alias: String,
    meshKey: String,
    fingerprint: String,
    activeRoute: TransportRoute? = null,
    routeState: RouteState = RouteState.IDLE,
    onEditAlias: () -> Unit,
    onCopyKey: () -> Unit,
    onOpenPairing: () -> Unit = {}
) {
    var showDiagnostics by remember { mutableStateOf(false) }

    val isRobolectric = Build.FINGERPRINT == "robolectric" || Build.HARDWARE == "robolectric"
    val pulseAlpha: Float
    val pulseScale: Float
    if (isRobolectric) {
        pulseAlpha = 0.3f
        pulseScale = 1.0f
    } else {
        val infiniteTransition = rememberInfiniteTransition(label = "RadarPulse")
        val animatedAlpha by infiniteTransition.animateFloat(
            initialValue = 0.6f,
            targetValue = 0.0f,
            animationSpec = infiniteRepeatable(
                animation = tween(2400, easing = LinearEasing),
                repeatMode = RepeatMode.Restart
            ),
            label = "PulseAlpha"
        )
        val animatedScale by infiniteTransition.animateFloat(
            initialValue = 0.8f,
            targetValue = 1.35f,
            animationSpec = infiniteRepeatable(
                animation = tween(2400, easing = LinearEasing),
                repeatMode = RepeatMode.Restart
            ),
            label = "PulseScale"
        )
        pulseAlpha = animatedAlpha
        pulseScale = animatedScale
    }

    CupertinoCard(
        shape = ArchDropTheme.shapes.cardHero,
        modifier = Modifier.clickable { showDiagnostics = true }
    ) {
        Column(
            modifier = Modifier.padding(20.dp),
            horizontalAlignment = Alignment.CenterHorizontally
        ) {
            // AirDrop Radar Pulsing concentric visualization
            Box(
                modifier = Modifier
                    .size(110.dp)
                    .padding(8.dp),
                contentAlignment = Alignment.Center
            ) {
                // Outer Pulse Ring
                Box(
                    modifier = Modifier
                        .size((90 * pulseScale).dp)
                        .clip(CircleShape)
                        .border(
                            1.5.dp,
                            ArchDropTheme.colors.primaryAccent.copy(alpha = pulseAlpha),
                            CircleShape
                        )
                )

                // Middle Translucent Ring
                Box(
                    modifier = Modifier
                        .size(80.dp)
                        .clip(CircleShape)
                        .background(ArchDropTheme.colors.primaryAccent.copy(alpha = 0.08f))
                        .border(
                            0.5.dp,
                            ArchDropTheme.colors.hairlineBorder,
                            CircleShape
                        )
                )

                // Center Icon Avatar with Glowing Accent
                Box(
                    modifier = Modifier
                        .size(54.dp)
                        .clip(CircleShape)
                        .background(ArchDropTheme.colors.primaryAccent),
                    contentAlignment = Alignment.Center
                ) {
                    Icon(
                        imageVector = Icons.Default.NearMe,
                        contentDescription = "AirDrop Proximity",
                        tint = Color.White,
                        modifier = Modifier.size(28.dp)
                    )
                }

                // Active Status Indicator Dot
                Box(
                    modifier = Modifier
                        .align(Alignment.BottomEnd)
                        .padding(end = 10.dp, bottom = 10.dp)
                        .size(14.dp)
                        .clip(CircleShape)
                        .background(ArchDropTheme.colors.secondarySystemGroupedBackground)
                        .padding(2.dp)
                        .clip(CircleShape)
                        .background(ArchDropTheme.colors.systemGreen)
                )
            }

            Spacer(modifier = Modifier.height(10.dp))

            // Device Title & Edit
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.Center
            ) {
                Text(
                    text = alias.ifEmpty { "Arch Drop Laptop" },
                    style = ArchDropTheme.typography.title2,
                    color = ArchDropTheme.colors.label,
                    fontWeight = FontWeight.Bold
                )
                CupertinoIconButton(
                    onClick = onEditAlias,
                    modifier = Modifier.size(28.dp)
                ) {
                    Icon(
                        imageVector = Icons.Default.Edit,
                        contentDescription = "Edit Alias",
                        tint = ArchDropTheme.colors.primaryAccent,
                        modifier = Modifier.size(16.dp)
                    )
                }
            }

            Spacer(modifier = Modifier.height(2.dp))

            // Apple HIG Connection Telemetry Badge
            CupertinoConnectionBadge(
                tier = activeRoute?.tier,
                state = routeState,
                onClick = { showDiagnostics = true }
            )

            Spacer(modifier = Modifier.height(16.dp))

            // Actions Row: "+ Pair Device" pill button + "Diagnostics" pill
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(10.dp)
            ) {
                CupertinoButton(
                    onClick = onOpenPairing,
                    style = CupertinoButtonStyle.FILLED,
                    shape = ArchDropTheme.shapes.buttonMedium,
                    modifier = Modifier.weight(1f)
                ) {
                    Icon(
                        imageVector = Icons.Default.QrCodeScanner,
                        contentDescription = null,
                        modifier = Modifier.size(18.dp)
                    )
                    Spacer(modifier = Modifier.width(8.dp))
                    Text(
                        text = "+ Pair Device",
                        style = ArchDropTheme.typography.headline,
                        fontWeight = FontWeight.SemiBold
                    )
                }

                CupertinoButton(
                    onClick = { showDiagnostics = true },
                    style = CupertinoButtonStyle.TONAL,
                    shape = ArchDropTheme.shapes.buttonMedium
                ) {
                    Icon(
                        imageVector = Icons.Default.Info,
                        contentDescription = "Diagnostics",
                        modifier = Modifier.size(18.dp),
                        tint = ArchDropTheme.colors.primaryAccent
                    )
                    Spacer(modifier = Modifier.width(6.dp))
                    Text(
                        text = "Diagnostics",
                        style = ArchDropTheme.typography.subheadline
                    )
                }
            }
        }
    }

    // Progressive Disclosure: Connection Diagnostics Modal
    if (showDiagnostics) {
        CupertinoAlertDialog(
            onDismissRequest = { showDiagnostics = false },
            title = "Connection Diagnostics",
            message = "Cryptographic identity and mesh networking status for $alias.",
            content = {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    // Ed25519 Key
                    Box(
                        modifier = Modifier
                            .fillMaxWidth()
                            .clip(ArchDropTheme.shapes.buttonSmall)
                            .background(ArchDropTheme.colors.tertiarySystemGroupedBackground)
                            .border(0.5.dp, ArchDropTheme.colors.hairlineBorder, ArchDropTheme.shapes.buttonSmall)
                            .clickable { onCopyKey() }
                            .padding(10.dp)
                    ) {
                        Column {
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.CenterVertically
                            ) {
                                Text(
                                    text = "ED25519 MESH PUBLIC KEY",
                                    style = ArchDropTheme.typography.caption2,
                                    fontWeight = FontWeight.Bold,
                                    color = ArchDropTheme.colors.secondaryLabel
                                )
                                Icon(
                                    imageVector = Icons.Default.ContentCopy,
                                    contentDescription = "Copy",
                                    modifier = Modifier.size(14.dp),
                                    tint = ArchDropTheme.colors.primaryAccent
                                )
                            }
                            Spacer(modifier = Modifier.height(4.dp))
                            Text(
                                text = if (meshKey.isNotEmpty()) formatHexBlocks(meshKey) else "Generating...",
                                style = ArchDropTheme.typography.caption2,
                                color = ArchDropTheme.colors.primaryAccent,
                                fontFamily = FontFamily.Monospace,
                                lineHeight = 16.sp
                            )
                        }
                    }

                    if (fingerprint.isNotEmpty()) {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier
                                .fillMaxWidth()
                                .clip(ArchDropTheme.shapes.buttonSmall)
                                .background(ArchDropTheme.colors.tertiarySystemGroupedBackground)
                                .border(0.5.dp, ArchDropTheme.colors.hairlineBorder, ArchDropTheme.shapes.buttonSmall)
                                .padding(horizontal = 8.dp, vertical = 6.dp)
                        ) {
                            Icon(
                                imageVector = Icons.Default.Fingerprint,
                                contentDescription = null,
                                modifier = Modifier.size(16.dp),
                                tint = ArchDropTheme.colors.systemGreen
                            )
                            Spacer(modifier = Modifier.width(6.dp))
                            Text(
                                text = "SHA-256: ${fingerprint.take(16)}...${fingerprint.takeLast(8)}",
                                style = ArchDropTheme.typography.caption2,
                                color = ArchDropTheme.colors.label
                            )
                        }
                    }
                }
            },
            confirmTitle = "Done",
            confirmColor = ArchDropTheme.colors.systemBlue,
            onConfirm = { showDiagnostics = false },
            cancelTitle = null,
            onCancel = null
        )
    }
}

@Composable
fun TopologyCard(
    lohsActive: Boolean,
    lohsSsid: String?,
    bleAdvertising: Boolean,
    derpConnected: Boolean,
    derpLatencyMs: Long?,
    activeRoute: TransportRoute? = null,
    routeState: RouteState = RouteState.IDLE,
    onToggleHotspot: () -> Unit
) {
    CupertinoCard(
        shape = ArchDropTheme.shapes.cardHero
    ) {
        Column(modifier = Modifier.padding(18.dp)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(
                        imageVector = Icons.Default.Hub,
                        contentDescription = null,
                        tint = ArchDropTheme.colors.primaryAccent,
                        modifier = Modifier.size(20.dp)
                    )
                    Spacer(modifier = Modifier.width(10.dp))
                    Text(
                        text = "Connection Topology",
                        style = ArchDropTheme.typography.headline,
                        color = ArchDropTheme.colors.label,
                        fontWeight = FontWeight.Bold
                    )
                }
                CupertinoConnectionBadge(
                    tier = activeRoute?.tier,
                    state = routeState
                )
            }

            Spacer(modifier = Modifier.height(14.dp))

            // Tier 1: Direct P2P Link (Local-Only Hotspot)
            TopologyTierRow(
                icon = Icons.Default.WifiTethering,
                title = "Direct P2P Link (LOHS SoftAP)",
                subtitle = if (lohsActive) "Active: ${lohsSsid ?: "SoftAP Gateway"}" else "Standby (Channel negotiated via BLE)",
                isActive = lohsActive,
                action = {
                    CupertinoButton(
                        onClick = onToggleHotspot,
                        style = if (lohsActive) CupertinoButtonStyle.DESTRUCTIVE else CupertinoButtonStyle.FILLED,
                        shape = ArchDropTheme.shapes.buttonSmall
                    ) {
                        Text(
                            text = if (lohsActive) "Stop" else "Start",
                            style = ArchDropTheme.typography.subheadline,
                            fontWeight = FontWeight.SemiBold
                        )
                    }
                }
            )

            Spacer(modifier = Modifier.height(10.dp))

            // Tier 2: BLE Proximity
            TopologyTierRow(
                icon = Icons.Default.Bluetooth,
                title = "BLE Proximity (Apple GATT 0x05)",
                subtitle = if (bleAdvertising) "Advertising (UUID a4c80001, port 53317)" else "Idle",
                isActive = bleAdvertising
            )

            Spacer(modifier = Modifier.height(10.dp))

            // Tier 3: DERP Mesh Relay
            TopologyTierRow(
                icon = if (derpConnected) Icons.Default.CloudDone else Icons.Default.CloudOff,
                title = "DERP Mesh Relay (HTTPS:443)",
                subtitle = if (derpConnected) "Connected (Loopback 127.0.0.1:53318, ${derpLatencyMs ?: 28} ms)" else "Disconnected",
                isActive = derpConnected
            )
        }
    }
}

@Composable
fun TopologyTierRow(
    icon: ImageVector,
    title: String,
    subtitle: String,
    isActive: Boolean,
    action: (@Composable () -> Unit)? = null
) {
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .clip(ArchDropTheme.shapes.card)
            .background(ArchDropTheme.colors.tertiarySystemGroupedBackground)
            .border(0.5.dp, ArchDropTheme.colors.hairlineBorder, ArchDropTheme.shapes.card)
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.weight(1f)
            ) {
                Box(
                    modifier = Modifier
                        .size(36.dp)
                        .clip(CircleShape)
                        .background(
                            if (isActive) ArchDropTheme.colors.primaryAccent.copy(alpha = 0.15f)
                            else ArchDropTheme.colors.systemGray.copy(alpha = 0.2f)
                        ),
                    contentAlignment = Alignment.Center
                ) {
                    Icon(
                        imageVector = icon,
                        contentDescription = null,
                        modifier = Modifier.size(18.dp),
                        tint = if (isActive) ArchDropTheme.colors.primaryAccent
                        else ArchDropTheme.colors.secondaryLabel
                    )
                }

                Spacer(modifier = Modifier.width(12.dp))

                Column {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Box(
                            modifier = Modifier
                                .size(8.dp)
                                .clip(CircleShape)
                                .background(
                                    if (isActive) ArchDropTheme.colors.systemGreen
                                    else ArchDropTheme.colors.systemGray
                                )
                        )
                        Spacer(modifier = Modifier.width(6.dp))
                        Text(
                            text = title,
                            style = ArchDropTheme.typography.body,
                            color = ArchDropTheme.colors.label,
                            fontWeight = FontWeight.SemiBold
                        )
                    }
                    Spacer(modifier = Modifier.height(2.dp))
                    Text(
                        text = subtitle,
                        style = ArchDropTheme.typography.caption1,
                        color = ArchDropTheme.colors.secondaryLabel,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis
                    )
                }
            }

            if (action != null) {
                Spacer(modifier = Modifier.width(8.dp))
                action()
            }
        }
    }
}

@Composable
fun TransferItemCard(transfer: TransferUiModel) {
    CupertinoCard(
        shape = ArchDropTheme.shapes.card
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(14.dp),
            verticalAlignment = Alignment.CenterVertically
        ) {
            Box(
                modifier = Modifier
                    .size(40.dp)
                    .clip(CircleShape)
                    .background(
                        if (transfer.isIncoming) ArchDropTheme.colors.primaryAccent.copy(alpha = 0.15f)
                        else ArchDropTheme.colors.systemGreen.copy(alpha = 0.15f)
                    ),
                contentAlignment = Alignment.Center
            ) {
                Icon(
                    imageVector = if (transfer.isIncoming) Icons.Default.ArrowDownward else Icons.Default.ArrowUpward,
                    contentDescription = null,
                    modifier = Modifier.size(20.dp),
                    tint = if (transfer.isIncoming) ArchDropTheme.colors.primaryAccent else ArchDropTheme.colors.systemGreen
                )
            }

            Spacer(modifier = Modifier.width(12.dp))

            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = transfer.fileName,
                    style = ArchDropTheme.typography.headline,
                    color = ArchDropTheme.colors.label,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis
                )
                Spacer(modifier = Modifier.height(2.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        text = transfer.fileSizeFormatted,
                        style = ArchDropTheme.typography.subheadline,
                        color = ArchDropTheme.colors.secondaryLabel
                    )
                    Text(
                        text = " • ${transfer.timestampFormatted}",
                        style = ArchDropTheme.typography.subheadline,
                        color = ArchDropTheme.colors.secondaryLabel
                    )
                }
            }

            if (transfer.isSuccess) {
                Spacer(modifier = Modifier.width(8.dp))
                Icon(
                    imageVector = Icons.Default.CheckCircle,
                    contentDescription = "Verified SHA-256 Checksum",
                    tint = ArchDropTheme.colors.systemGreen,
                    modifier = Modifier.size(20.dp)
                )
            }
        }
    }
}

@Composable
fun EmptyTransfersCard() {
    CupertinoCard(
        shape = ArchDropTheme.shapes.card
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(24.dp),
            horizontalAlignment = Alignment.CenterHorizontally
        ) {
            Icon(
                imageVector = Icons.Default.History,
                contentDescription = null,
                modifier = Modifier.size(44.dp),
                tint = ArchDropTheme.colors.systemGray.copy(alpha = 0.6f)
            )
            Spacer(modifier = Modifier.height(8.dp))
            Text(
                text = "No transfers yet",
                style = ArchDropTheme.typography.title3,
                color = ArchDropTheme.colors.label,
                fontWeight = FontWeight.SemiBold
            )
            Spacer(modifier = Modifier.height(4.dp))
            Text(
                text = "Scan a peer's QR code or start the service to send and receive files securely.",
                style = ArchDropTheme.typography.subheadline,
                color = ArchDropTheme.colors.secondaryLabel,
                textAlign = TextAlign.Center
            )
        }
    }
}

@Composable
fun ClipboardActionCard(
    isSending: Boolean,
    onSendClipboard: () -> Unit,
    modifier: Modifier = Modifier
) {
    CupertinoCard(
        shape = ArchDropTheme.shapes.cardHero,
        modifier = modifier
    ) {
        Column(modifier = Modifier.padding(18.dp)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically
            ) {
                Box(
                    modifier = Modifier
                        .size(40.dp)
                        .clip(CircleShape)
                        .background(ArchDropTheme.colors.primaryAccent.copy(alpha = 0.15f)),
                    contentAlignment = Alignment.Center
                ) {
                    Icon(
                        imageVector = Icons.Default.Share,
                        contentDescription = null,
                        tint = ArchDropTheme.colors.primaryAccent,
                        modifier = Modifier.size(20.dp)
                    )
                }
                Spacer(modifier = Modifier.width(12.dp))
                Column {
                    Text(
                        text = "Universal Clipboard",
                        style = ArchDropTheme.typography.headline,
                        color = ArchDropTheme.colors.label,
                        fontWeight = FontWeight.Bold
                    )
                    Text(
                        text = "Instant bidirectional copy & paste with desktop",
                        style = ArchDropTheme.typography.footnote,
                        color = ArchDropTheme.colors.secondaryLabel
                    )
                }
            }

            Spacer(modifier = Modifier.height(14.dp))

            CupertinoButton(
                onClick = onSendClipboard,
                enabled = !isSending,
                style = CupertinoButtonStyle.FILLED,
                shape = ArchDropTheme.shapes.buttonMedium,
                modifier = Modifier.fillMaxWidth()
            ) {
                if (isSending) {
                    CupertinoActivityIndicator(
                        modifier = Modifier.size(18.dp),
                        strokeWidth = 2.dp,
                        color = Color.White
                    )
                    Spacer(modifier = Modifier.width(8.dp))
                    Text(
                        text = "Sending...",
                        style = ArchDropTheme.typography.headline
                    )
                } else {
                    Icon(
                        imageVector = Icons.AutoMirrored.Filled.Send,
                        contentDescription = null,
                        modifier = Modifier.size(18.dp)
                    )
                    Spacer(modifier = Modifier.width(8.dp))
                    Text(
                        text = "Send Clipboard to Laptop",
                        style = ArchDropTheme.typography.headline,
                        fontWeight = FontWeight.SemiBold
                    )
                }
            }
        }
    }
}

@Composable
fun AudioQuickActionCard(
    isStreaming: Boolean,
    connectionState: AudioReceiverState,
    telemetry: JitterTelemetry,
    onToggleStream: () -> Unit,
    onOpenAudioScreen: () -> Unit,
    modifier: Modifier = Modifier
) {
    CupertinoCard(
        shape = ArchDropTheme.shapes.cardHero,
        modifier = modifier.clickable { onOpenAudioScreen() }
    ) {
        Column(modifier = Modifier.padding(18.dp)) {
            // Header Row: Icon, Title & Summary, Chevron button
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.weight(1f)
                ) {
                    Box(
                        modifier = Modifier
                            .size(40.dp)
                            .clip(CircleShape)
                            .background(
                                if (isStreaming) ArchDropTheme.colors.systemGreen.copy(alpha = 0.15f)
                                else ArchDropTheme.colors.primaryAccent.copy(alpha = 0.15f)
                            ),
                        contentAlignment = Alignment.Center
                    ) {
                        Icon(
                            imageVector = Icons.Default.Headphones,
                            contentDescription = null,
                            tint = if (isStreaming) ArchDropTheme.colors.systemGreen
                            else ArchDropTheme.colors.primaryAccent,
                            modifier = Modifier.size(20.dp)
                        )
                    }

                    Spacer(modifier = Modifier.width(12.dp))

                    Column {
                        Text(
                            text = "Wireless Audio Output",
                            style = ArchDropTheme.typography.headline,
                            color = ArchDropTheme.colors.label,
                            fontWeight = FontWeight.Bold
                        )
                        Spacer(modifier = Modifier.height(2.dp))
                        Text(
                            text = getAudioStatusSummary(connectionState, telemetry),
                            style = ArchDropTheme.typography.subheadline,
                            color = ArchDropTheme.colors.secondaryLabel,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis
                        )
                    }
                }

                Row(verticalAlignment = Alignment.CenterVertically) {
                    AudioConnectionBadge(connectionState = connectionState)
                    Spacer(modifier = Modifier.width(6.dp))
                    CupertinoIconButton(
                        onClick = onOpenAudioScreen,
                        modifier = Modifier.size(28.dp)
                    ) {
                        Icon(
                            imageVector = Icons.AutoMirrored.Filled.ArrowForward,
                            contentDescription = "Open Audio Screen",
                            modifier = Modifier.size(18.dp),
                            tint = ArchDropTheme.colors.secondaryLabel
                        )
                    }
                }
            }

            // Live Telemetry Bar (active during streaming)
            if (isStreaming && connectionState is AudioReceiverState.Streaming) {
                Spacer(modifier = Modifier.height(12.dp))
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    // Buffer depth pill
                    val bufferDepth = telemetry.bufferDepthMs.toInt()
                    val isOptimal = bufferDepth in 30..60
                    Box(
                        modifier = Modifier
                            .weight(1f)
                            .clip(ArchDropTheme.shapes.capsule)
                            .background(
                                if (isOptimal) ArchDropTheme.colors.systemGreen.copy(alpha = 0.15f)
                                else ArchDropTheme.colors.systemYellow.copy(alpha = 0.15f)
                            ),
                        contentAlignment = Alignment.Center
                    ) {
                        Text(
                            text = "${bufferDepth}ms buffer",
                            style = ArchDropTheme.typography.caption1,
                            fontWeight = FontWeight.SemiBold,
                            color = if (isOptimal) ArchDropTheme.colors.systemGreen else ArchDropTheme.colors.systemYellow,
                            textAlign = TextAlign.Center,
                            modifier = Modifier.padding(vertical = 6.dp, horizontal = 8.dp)
                        )
                    }

                    // Jitter pill
                    Box(
                        modifier = Modifier
                            .weight(1f)
                            .clip(ArchDropTheme.shapes.capsule)
                            .background(ArchDropTheme.colors.tertiarySystemGroupedBackground),
                        contentAlignment = Alignment.Center
                    ) {
                        Text(
                            text = String.format(Locale.US, "±%.1fms jitter", telemetry.jitterMs),
                            style = ArchDropTheme.typography.caption1,
                            fontWeight = FontWeight.Medium,
                            color = ArchDropTheme.colors.secondaryLabel,
                            textAlign = TextAlign.Center,
                            modifier = Modifier.padding(vertical = 6.dp, horizontal = 8.dp)
                        )
                    }

                    // Loss counter pill
                    Box(
                        modifier = Modifier
                            .weight(1f)
                            .clip(ArchDropTheme.shapes.capsule)
                            .background(ArchDropTheme.colors.tertiarySystemGroupedBackground),
                        contentAlignment = Alignment.Center
                    ) {
                        Text(
                            text = if (telemetry.packetsLost > 0) "${telemetry.packetsLost} drops" else "0 drops",
                            style = ArchDropTheme.typography.caption1,
                            fontWeight = FontWeight.Medium,
                            color = if (telemetry.packetsLost > 0) ArchDropTheme.colors.systemRed else ArchDropTheme.colors.secondaryLabel,
                            textAlign = TextAlign.Center,
                            modifier = Modifier.padding(vertical = 6.dp, horizontal = 8.dp)
                        )
                    }
                }
            }

            Spacer(modifier = Modifier.height(14.dp))

            // 1-Tap Toggle Button
            CupertinoButton(
                onClick = onToggleStream,
                style = if (isStreaming) CupertinoButtonStyle.DESTRUCTIVE else CupertinoButtonStyle.FILLED,
                shape = ArchDropTheme.shapes.buttonMedium,
                modifier = Modifier.fillMaxWidth()
            ) {
                if (connectionState is AudioReceiverState.Connecting) {
                    CupertinoActivityIndicator(
                        modifier = Modifier.size(18.dp),
                        strokeWidth = 2.dp,
                        color = Color.White
                    )
                    Spacer(modifier = Modifier.width(8.dp))
                    Text("Connecting...", style = ArchDropTheme.typography.headline)
                } else if (isStreaming) {
                    Icon(
                        imageVector = Icons.Default.Stop,
                        contentDescription = null,
                        modifier = Modifier.size(18.dp)
                    )
                    Spacer(modifier = Modifier.width(8.dp))
                    Text("Stop Listening", style = ArchDropTheme.typography.headline, fontWeight = FontWeight.SemiBold)
                } else {
                    Icon(
                        imageVector = Icons.Default.PlayArrow,
                        contentDescription = null,
                        modifier = Modifier.size(18.dp)
                    )
                    Spacer(modifier = Modifier.width(8.dp))
                    Text("Listen to Desktop Audio", style = ArchDropTheme.typography.headline, fontWeight = FontWeight.SemiBold)
                }
            }
        }
    }
}

fun getAudioStatusSummary(
    state: AudioReceiverState,
    telemetry: JitterTelemetry
): String {
    return when (state) {
        is AudioReceiverState.Streaming -> {
            val buf = telemetry.bufferDepthMs.toInt()
            val jit = String.format(Locale.US, "%.1f", telemetry.jitterMs)
            "Streaming · ${buf}ms buffer · ±${jit}ms jitter"
        }
        is AudioReceiverState.Connecting -> "Connecting to desktop PipeWire sink..."
        is AudioReceiverState.Paused -> "Audio focus paused (e.g. phone call)"
        is AudioReceiverState.Error -> state.message.take(35) + if (state.message.length > 35) "..." else ""
        is AudioReceiverState.Disconnected -> "Stream disconnected · Tap to reconnect"
        is AudioReceiverState.Idle -> "Low-latency wireless headphone receiver"
    }
}

@Composable
fun AudioConnectionBadge(connectionState: AudioReceiverState) {
    val (bgColor, textColor, text, icon) = when (connectionState) {
        is AudioReceiverState.Streaming -> Quadruple(
            ArchDropTheme.colors.systemGreen.copy(alpha = 0.15f),
            ArchDropTheme.colors.systemGreen,
            "Live",
            Icons.Default.CheckCircle
        )
        is AudioReceiverState.Connecting -> Quadruple(
            ArchDropTheme.colors.systemYellow.copy(alpha = 0.15f),
            ArchDropTheme.colors.systemYellow,
            "Connecting",
            Icons.Default.Info
        )
        is AudioReceiverState.Paused -> Quadruple(
            ArchDropTheme.colors.systemYellow.copy(alpha = 0.15f),
            ArchDropTheme.colors.systemYellow,
            "Paused",
            Icons.Default.Info
        )
        is AudioReceiverState.Error -> Quadruple(
            ArchDropTheme.colors.systemRed.copy(alpha = 0.15f),
            ArchDropTheme.colors.systemRed,
            "Error",
            Icons.Default.Error
        )
        is AudioReceiverState.Disconnected,
        is AudioReceiverState.Idle -> Quadruple(
            ArchDropTheme.colors.tertiarySystemGroupedBackground,
            ArchDropTheme.colors.secondaryLabel,
            "Standby",
            Icons.Default.Headphones
        )
    }

    Box(
        modifier = Modifier
            .clip(ArchDropTheme.shapes.capsule)
            .background(bgColor)
    ) {
        Row(
            modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp)
        ) {
            Icon(
                imageVector = icon,
                contentDescription = null,
                tint = textColor,
                modifier = Modifier.size(12.dp)
            )
            Text(
                text = text,
                style = ArchDropTheme.typography.caption2,
                fontWeight = FontWeight.SemiBold,
                color = textColor
            )
        }
    }
}

private data class Quadruple<A, B, C, D>(val first: A, val second: B, val third: C, val fourth: D)


