package org.archdrop.ui.pairing

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import androidx.compose.foundation.background
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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material3.Icon
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import org.archdrop.ui.theme.CupertinoAlertDialog
import org.archdrop.ui.theme.CupertinoButton
import org.archdrop.ui.theme.CupertinoButtonStyle
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
import org.archdrop.ui.theme.CupertinoSegmentedControl
import org.archdrop.ui.theme.cupertinoHairline
import org.archdrop.ui.theme.cupertinoHairlineBottom
import org.archdrop.ui.theme.cupertinoTouchTarget
import java.io.File
import java.security.MessageDigest
import java.util.UUID

@Composable
fun PairingScreen(
    database: ArchDropDatabase,
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

    Box(
        modifier = modifier
            .fillMaxSize()
            .background(ArchDropTheme.colors.systemBackground)
    ) {
        Column(modifier = Modifier.fillMaxSize()) {
            // Apple HIG Segmented Control Bar Header
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .background(ArchDropTheme.colors.glassBarBackground)
                    .cupertinoHairlineBottom(ArchDropTheme.colors.hairlineBorder)
                    .padding(horizontal = 20.dp, vertical = 10.dp)
            ) {
                CupertinoSegmentedControl(
                    items = listOf("My QR Code", "Scan Camera"),
                    selectedIndex = selectedTabIndex,
                    onItemSelected = { selectedTabIndex = it }
                )
            }

            if (selectedTabIndex == 0) {
                // My QR Tab
                Column(
                    modifier = Modifier
                        .fillMaxSize()
                        .verticalScroll(rememberScrollState())
                        .padding(horizontal = 24.dp, vertical = 16.dp),
                    horizontalAlignment = Alignment.CenterHorizontally
                ) {
                    Text(
                        text = "Pair with Arch Linux or Companion",
                        style = ArchDropTheme.typography.title2,
                        color = ArchDropTheme.colors.label,
                        fontWeight = FontWeight.Bold,
                        textAlign = TextAlign.Center
                    )
                    Spacer(modifier = Modifier.height(6.dp))
                    Text(
                        text = "Scan this offline QR code with 'archdrop pair' or camera to establish mutual trust.",
                        style = ArchDropTheme.typography.subheadline,
                        color = ArchDropTheme.colors.secondaryLabel,
                        textAlign = TextAlign.Center
                    )

                    Spacer(modifier = Modifier.height(24.dp))

                    // Apple continuous squircle card containing high-contrast QR
                    Box(
                        modifier = Modifier
                            .clip(ArchDropTheme.shapes.cardHero)
                            .background(ArchDropTheme.colors.secondarySystemGroupedBackground)
                            .cupertinoHairline(ArchDropTheme.shapes.cardHero)
                            .padding(20.dp),
                        contentAlignment = Alignment.Center
                    ) {
                        QrCodeView(content = myPairingUri, size = 260.dp)
                    }

                    Spacer(modifier = Modifier.height(20.dp))

                    Text(
                        text = deviceAlias,
                        style = ArchDropTheme.typography.title3,
                        color = ArchDropTheme.colors.label,
                        fontWeight = FontWeight.Bold
                    )

                    Spacer(modifier = Modifier.height(8.dp))

                    if (localMeshKey.isNotEmpty()) {
                        Text(
                            text = formatHexBlocks(localMeshKey),
                            style = ArchDropTheme.typography.monospaceHex,
                            color = ArchDropTheme.colors.systemBlue,
                            textAlign = TextAlign.Center
                        )
                    }

                    Spacer(modifier = Modifier.height(24.dp))

                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.Center
                    ) {
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
                            modifier = Modifier.cupertinoTouchTarget()
                        ) {
                            Icon(
                                imageVector = Icons.Default.ContentCopy,
                                contentDescription = null,
                                tint = ArchDropTheme.colors.systemBlue,
                                modifier = Modifier.size(18.dp)
                            )
                            Spacer(modifier = Modifier.width(8.dp))
                            Text(
                                text = "Copy Pairing URI",
                                style = ArchDropTheme.typography.headline,
                                color = ArchDropTheme.colors.label
                            )
                        }
                    }
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

        // Authentic Apple HIG Confirmation Dialog
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
                            color = ArchDropTheme.colors.secondaryLabel
                        )
                        Spacer(modifier = Modifier.height(10.dp))
                        Text(
                            text = "Alias: ${payload.alias}",
                            style = ArchDropTheme.typography.headline,
                            color = ArchDropTheme.colors.label,
                            fontWeight = FontWeight.SemiBold
                        )
                        Spacer(modifier = Modifier.height(4.dp))
                        Text(
                            text = "Port: ${payload.port}",
                            style = ArchDropTheme.typography.body,
                            color = ArchDropTheme.colors.secondaryLabel
                        )
                        Spacer(modifier = Modifier.height(8.dp))
                        Text(
                            text = "Ed25519 Mesh Public Key:",
                            style = ArchDropTheme.typography.footnote,
                            color = ArchDropTheme.colors.secondaryLabel,
                            fontWeight = FontWeight.Bold
                        )
                        Spacer(modifier = Modifier.height(2.dp))
                        Text(
                            text = formatHexBlocks(payload.meshKey),
                            style = ArchDropTheme.typography.monospaceHex,
                            color = ArchDropTheme.colors.systemBlue
                        )
                        if (payload.fingerprint.isNotEmpty()) {
                            Spacer(modifier = Modifier.height(8.dp))
                            Text(
                                text = "mTLS Certificate Fingerprint:",
                                style = ArchDropTheme.typography.footnote,
                                color = ArchDropTheme.colors.secondaryLabel,
                                fontWeight = FontWeight.Bold
                            )
                            Spacer(modifier = Modifier.height(2.dp))
                            Text(
                                text = payload.fingerprint.take(24) + "...",
                                style = ArchDropTheme.typography.monospaceHex,
                                color = ArchDropTheme.colors.systemGreen
                            )
                        }
                    }
                },
                confirmTitle = "Trust & Pair",
                onConfirm = {
                    showConfirmDialog = false
                    scope.launch {
                        withContext(Dispatchers.IO) {
                            val now = System.currentTimeMillis()
                            // 1. Save peer to Room DB
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

                            // 2. Commit peer.paired event into causal ledger
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

fun formatHexBlocks(hex: String): String {
    return hex.chunked(8).chunked(4).joinToString("\n") { row ->
        row.joinToString(" ")
    }
}
