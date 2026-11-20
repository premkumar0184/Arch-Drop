package org.archdrop.ui.pairing

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
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxHeight
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
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material3.Icon
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import org.archdrop.ui.theme.CupertinoAlertDialog
import org.archdrop.ui.theme.CupertinoButton
import org.archdrop.ui.theme.CupertinoButtonStyle
import org.archdrop.ui.theme.CupertinoIconButton
import org.archdrop.ui.theme.CupertinoModalBottomSheet
import org.archdrop.ui.theme.CupertinoSheetGrabber
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
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
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.archdrop.db.ArchDropDatabase
import org.archdrop.db.EventEntity
import org.archdrop.db.PeerEntity
import org.archdrop.mesh.MeshIdentity
import org.archdrop.ui.theme.ArchDropTheme
import org.archdrop.ui.theme.cupertinoHairline
import org.archdrop.ui.theme.cupertinoTouchTarget
import java.io.File
import java.security.MessageDigest
import java.util.UUID

/**
 * Reusable full-height Apple HIG Modal Bottom Sheet for cryptographic peer pairing.
 *
 * Implements 24dp top squircle corners via [ArchDropTheme.shapes.sheetSquircle],
 * OLED True Black / Cupertino Grouped Background styling, an Apple grabber indicator,
 * a close icon button with >= 44x44 pt touch target, and dual-tab pairing support.
 */
@Composable
fun PairingBottomSheet(
    isVisible: Boolean = true,
    onDismissRequest: () -> Unit,
    database: ArchDropDatabase,
    modifier: Modifier = Modifier
) {
    if (!isVisible) return

    CupertinoModalBottomSheet(
        onDismissRequest = onDismissRequest,
        modifier = modifier
    ) {
        PairingSheetContent(
            database = database,
            onClose = onDismissRequest
        )
    }
}

/**
 * Convenience overload accepting (database, onDismissRequest, modifier)
 */
@Composable
fun PairingBottomSheet(
    database: ArchDropDatabase,
    onDismissRequest: () -> Unit,
    modifier: Modifier = Modifier
) = PairingBottomSheet(
    isVisible = true,
    onDismissRequest = onDismissRequest,
    database = database,
    modifier = modifier
)

@Composable
fun PairingSheetContent(
    database: ArchDropDatabase,
    onClose: () -> Unit,
    modifier: Modifier = Modifier
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snackbarHostState = remember { SnackbarHostState() }

    var selectedTabIndex by remember { mutableIntStateOf(0) }
    var localMeshKey by remember { mutableStateOf("") }
    var localFingerprint by remember { mutableStateOf("") }
    val deviceAlias = remember { Build.MODEL ?: "Android Device" }

    var scannedPayload by remember { mutableStateOf<PairingPayload?>(null) }
    var showConfirmDialog by remember { mutableStateOf(false) }

    LaunchedEffect(Unit) {
        withContext(Dispatchers.IO) {
            val keyFile = File(context.filesDir, "mesh_identity.key")
            val identity = MeshIdentity.loadOrGenerate(keyFile)
            val keyHex = identity.toHex()
            val md = MessageDigest.getInstance("SHA-256")
            val fp = md.digest(identity.publicKeyBytes).joinToString("") { "%02X".format(it) }
            withContext(Dispatchers.Main) {
                localMeshKey = keyHex
                localFingerprint = fp
            }
        }
    }

    val myPairingUri = remember(localMeshKey, localFingerprint, deviceAlias) {
        if (localMeshKey.isNotEmpty()) {
            val payload = PairingPayload(
                version = 1,
                alias = deviceAlias,
                meshKey = localMeshKey,
                fingerprint = localFingerprint,
                port = 53317
            )
            payload.toUri()
        } else ""
    }

    Box(modifier = modifier.fillMaxWidth().fillMaxHeight(0.92f)) {
        Column(modifier = Modifier.fillMaxSize()) {
            // Header Bar: Title and Apple-style circular Close Button
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 20.dp, vertical = 6.dp),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Text(
                    text = "Pair Device",
                    style = ArchDropTheme.typography.headline,
                    color = ArchDropTheme.colors.labelPrimary,
                    fontWeight = FontWeight.SemiBold
                )

                CupertinoIconButton(
                    onClick = onClose,
                    modifier = Modifier.size(44.dp)
                ) {
                    Box(
                        modifier = Modifier
                            .size(30.dp)
                            .clip(CircleShape)
                            .background(ArchDropTheme.colors.systemFill),
                        contentAlignment = Alignment.Center
                    ) {
                        Icon(
                            imageVector = Icons.Default.Close,
                            contentDescription = "Close Pairing Sheet",
                            modifier = Modifier.size(16.dp),
                            tint = ArchDropTheme.colors.labelSecondary
                        )
                    }
                }
            }

            // iOS Segmented Control Track: [ My QR Code | Scan Camera ]
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 20.dp, vertical = 6.dp)
                    .clip(ArchDropTheme.shapes.controlSquircle)
                    .background(ArchDropTheme.colors.systemFill)
                    .padding(2.dp),
                horizontalArrangement = Arrangement.spacedBy(4.dp)
            ) {
                listOf(
                    0 to "My QR Code",
                    1 to "Scan Camera"
                ).forEach { (index, title) ->
                    val isSelected = selectedTabIndex == index
                    Box(
                        modifier = Modifier
                            .weight(1f)
                            .height(32.dp)
                            .clip(ArchDropTheme.shapes.segmentedThumb)
                            .background(
                                if (isSelected) ArchDropTheme.colors.groupedCard else Color.Transparent
                            )
                            .clickable { selectedTabIndex = index }
                            .then(
                                if (isSelected) Modifier.cupertinoHairline(shape = ArchDropTheme.shapes.segmentedThumb) else Modifier
                            ),
                        contentAlignment = Alignment.Center
                    ) {
                        Text(
                            text = title,
                            style = ArchDropTheme.typography.subheadline,
                            fontWeight = if (isSelected) FontWeight.SemiBold else FontWeight.Normal,
                            color = if (isSelected) ArchDropTheme.colors.labelPrimary else ArchDropTheme.colors.labelSecondary
                        )
                    }
                }
            }

            Spacer(modifier = Modifier.height(8.dp))

            // Tab Content
            if (selectedTabIndex == 0) {
                // My QR Code Tab
                Column(
                    modifier = Modifier
                        .fillMaxSize()
                        .verticalScroll(rememberScrollState())
                        .padding(horizontal = 24.dp, vertical = 12.dp),
                    horizontalAlignment = Alignment.CenterHorizontally
                ) {
                    Text(
                        text = "Pair with Arch Linux or Companion",
                        style = ArchDropTheme.typography.title2,
                        color = ArchDropTheme.colors.labelPrimary,
                        textAlign = TextAlign.Center
                    )
                    Spacer(modifier = Modifier.height(4.dp))
                    Text(
                        text = "Scan this offline QR code with 'archdrop pair' or camera to establish mutual trust.",
                        style = ArchDropTheme.typography.subheadline,
                        color = ArchDropTheme.colors.labelSecondary,
                        textAlign = TextAlign.Center
                    )

                    Spacer(modifier = Modifier.height(20.dp))

                    // QR Code Card in 20dp Squircle Container
                    Box(
                        modifier = Modifier
                            .clip(ArchDropTheme.shapes.heroSquircle)
                            .background(Color.White)
                            .padding(16.dp),
                        contentAlignment = Alignment.Center
                    ) {
                        QrCodeView(content = myPairingUri, size = 240.dp)
                    }

                    Spacer(modifier = Modifier.height(18.dp))

                    Text(
                        text = deviceAlias,
                        style = ArchDropTheme.typography.headline,
                        color = ArchDropTheme.colors.labelPrimary,
                        fontWeight = FontWeight.Bold
                    )

                    Spacer(modifier = Modifier.height(6.dp))

                    if (localMeshKey.isNotEmpty()) {
                        Text(
                            text = formatHexBlocks(localMeshKey),
                            style = ArchDropTheme.typography.caption2,
                            color = ArchDropTheme.colors.accent,
                            fontFamily = FontFamily.Monospace,
                            textAlign = TextAlign.Center
                        )
                    }

                    Spacer(modifier = Modifier.height(18.dp))

                    CupertinoButton(
                        onClick = {
                            val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                            val clip = ClipData.newPlainText("Arch Drop Pairing URI", myPairingUri)
                            clipboard?.setPrimaryClip(clip)
                            scope.launch {
                                snackbarHostState.showSnackbar("Pairing URI copied to clipboard")
                            }
                        },
                        style = CupertinoButtonStyle.TONAL,
                        shape = ArchDropTheme.shapes.buttonLarge,
                        modifier = Modifier
                            .defaultMinSize(minHeight = 44.dp)
                            .cupertinoTouchTarget()
                    ) {
                        Icon(
                            imageVector = Icons.Default.ContentCopy,
                            contentDescription = null,
                            modifier = Modifier.size(16.dp),
                            tint = ArchDropTheme.colors.accent
                        )
                        Spacer(modifier = Modifier.width(8.dp))
                        Text(
                            text = "Copy Pairing URI",
                            style = ArchDropTheme.typography.subheadline,
                            color = ArchDropTheme.colors.accent
                        )
                    }
                    Spacer(modifier = Modifier.height(24.dp))
                }
            } else {
                // Scan Camera Tab
                QrCameraScanner(
                    onPayloadScanned = { payload ->
                        scannedPayload = payload
                        showConfirmDialog = true
                    },
                    modifier = Modifier.fillMaxSize()
                )
            }
        }

        SnackbarHost(
            hostState = snackbarHostState,
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .padding(bottom = 16.dp)
        )

        // Confirmation Dialog
        if (showConfirmDialog && scannedPayload != null) {
            val payload = scannedPayload!!
            CupertinoAlertDialog(
                onDismissRequest = {
                    showConfirmDialog = false
                    scannedPayload = null
                },
                title = "Trust & Pair Device",
                content = {
                    Column(horizontalAlignment = Alignment.Start) {
                        Text(
                            text = "Discovered peer wants to establish cryptographic trust:",
                            style = ArchDropTheme.typography.subheadline,
                            color = ArchDropTheme.colors.labelSecondary
                        )
                        Spacer(modifier = Modifier.height(12.dp))
                        Text(
                            text = "Alias: ${payload.alias}",
                            style = ArchDropTheme.typography.headline,
                            color = ArchDropTheme.colors.labelPrimary,
                            fontWeight = FontWeight.SemiBold
                        )
                        Spacer(modifier = Modifier.height(2.dp))
                        Text(
                            text = "Port: ${payload.port}",
                            style = ArchDropTheme.typography.subheadline,
                            color = ArchDropTheme.colors.labelSecondary
                        )
                        Spacer(modifier = Modifier.height(8.dp))
                        Text(
                            text = "Ed25519 Mesh Public Key:",
                            style = ArchDropTheme.typography.caption1,
                            fontWeight = FontWeight.Bold,
                            color = ArchDropTheme.colors.labelTertiary
                        )
                        Text(
                            text = formatHexBlocks(payload.meshKey),
                            style = ArchDropTheme.typography.caption2,
                            color = ArchDropTheme.colors.accent,
                            fontFamily = FontFamily.Monospace
                        )
                        if (payload.fingerprint.isNotEmpty()) {
                            Spacer(modifier = Modifier.height(6.dp))
                            Text(
                                text = "mTLS Certificate Fingerprint:",
                                style = ArchDropTheme.typography.caption1,
                                fontWeight = FontWeight.Bold,
                                color = ArchDropTheme.colors.labelTertiary
                            )
                            Text(
                                text = payload.fingerprint.take(24) + "...",
                                style = ArchDropTheme.typography.caption2,
                                color = ArchDropTheme.colors.systemGreen,
                                fontFamily = FontFamily.Monospace
                            )
                        }
                    }
                },
                confirmTitle = "Trust & Pair",
                confirmColor = ArchDropTheme.colors.accent,
                onConfirm = {
                    showConfirmDialog = false
                    scope.launch {
                        withContext(Dispatchers.IO) {
                            val now = System.currentTimeMillis()
                            val peer = PeerEntity(
                                meshKey = payload.meshKey,
                                alias = payload.alias,
                                fingerprint = payload.fingerprint,
                                port = payload.port,
                                pairedAtMs = now,
                                lastSeenMs = now,
                                isTrusted = true
                            )
                            database.peerDao().insertPeer(peer)

                            val nextSeq = database.eventDao().getNextSequenceNumber(localMeshKey)
                            val eventPayload = "{\"alias\":\"${payload.alias}\",\"meshKey\":\"${payload.meshKey}\",\"fingerprint\":\"${payload.fingerprint}\",\"port\":${payload.port},\"timestamp\":$now}"
                            val md = MessageDigest.getInstance("SHA-256")
                            val payloadHash = md.digest(eventPayload.toByteArray()).joinToString("") { "%02x".format(it) }

                            val event = EventEntity(
                                originDeviceId = localMeshKey.ifEmpty { "ANDROID_LOCAL" },
                                sequenceNumber = nextSeq,
                                eventId = UUID.randomUUID().toString(),
                                timestampMs = now,
                                eventType = "peer.paired",
                                payload = eventPayload,
                                payloadHash = payloadHash,
                                ackStatus = 0
                            )
                            database.eventDao().insertEvent(event)
                            database.vectorClockDao().advanceHighWaterMark(
                                localMeshKey.ifEmpty { "ANDROID_LOCAL" },
                                nextSeq,
                                now,
                                "COMMITTED"
                            )
                        }
                        snackbarHostState.showSnackbar("Successfully paired with ${payload.alias}")
                        onClose()
                    }
                },
                cancelTitle = "Cancel",
                onCancel = {
                    showConfirmDialog = false
                    scannedPayload = null
                }
            )
        }
    }
}
