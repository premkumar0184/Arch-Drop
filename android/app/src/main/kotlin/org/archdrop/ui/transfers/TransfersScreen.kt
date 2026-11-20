package org.archdrop.ui.transfers

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
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
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowDownward
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.Icon
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import org.archdrop.ui.theme.CupertinoAlertDialog
import org.archdrop.ui.theme.CupertinoCard
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
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import org.archdrop.db.ArchDropDatabase
import org.archdrop.ui.dashboard.DashboardViewModel
import org.archdrop.ui.dashboard.TransferUiModel
import org.archdrop.ui.theme.ArchDropTheme
import org.archdrop.ui.theme.CupertinoSegmentedControl
import org.archdrop.ui.theme.cupertinoHairline
import org.archdrop.ui.theme.cupertinoTouchTarget

@Composable
fun TransfersScreen(
    database: ArchDropDatabase,
    modifier: Modifier = Modifier
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snackbarHostState = remember { SnackbarHostState() }

    var searchQuery by remember { mutableStateOf("") }
    var selectedFilter by remember { mutableStateOf("All") }
    var selectedTransferForDetails by remember { mutableStateOf<TransferUiModel?>(null) }

    val transfersFlow = remember(database) {
        database.eventDao().observeTransferEvents(200).map { events ->
            events.map { DashboardViewModel.eventToTransferUiModel(it) }
        }
    }

    val transfers by transfersFlow.collectAsState(initial = emptyList())

    val filteredTransfers = remember(transfers, searchQuery, selectedFilter) {
        transfers.filter { item ->
            val matchesSearch = searchQuery.isEmpty() ||
                item.fileName.contains(searchQuery, ignoreCase = true) ||
                item.sha256.contains(searchQuery, ignoreCase = true)
            val matchesFilter = when (selectedFilter) {
                "Incoming" -> item.isIncoming
                "Outgoing" -> !item.isIncoming
                "Verified" -> item.isSuccess
                else -> true
            }
            matchesSearch && matchesFilter
        }
    }

    Box(
        modifier = modifier
            .fillMaxSize()
            .background(ArchDropTheme.colors.systemBackground)
    ) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(horizontal = 16.dp)
        ) {
            Spacer(modifier = Modifier.height(16.dp))

            // iOS Large Title Header
            Text(
                text = "Activity",
                style = ArchDropTheme.typography.largeTitle,
                color = ArchDropTheme.colors.label,
                fontWeight = FontWeight.Bold
            )

            Spacer(modifier = Modifier.height(12.dp))

            // Cupertino Search Bar
            CupertinoSearchBar(
                query = searchQuery,
                onQueryChange = { searchQuery = it },
                onClear = { searchQuery = "" },
                modifier = Modifier.fillMaxWidth()
            )

            Spacer(modifier = Modifier.height(12.dp))

            // Cupertino Segmented Control Filter Scope
            val filterOptions = listOf("All", "Incoming", "Outgoing", "Verified")
            val selectedIndex = filterOptions.indexOf(selectedFilter).coerceAtLeast(0)
            CupertinoSegmentedControl(
                items = filterOptions,
                selectedIndex = selectedIndex,
                onItemSelected = { selectedFilter = filterOptions[it] },
                modifier = Modifier.fillMaxWidth()
            )

            Spacer(modifier = Modifier.height(14.dp))

            if (filteredTransfers.isEmpty()) {
                Box(
                    modifier = Modifier
                        .fillMaxSize()
                        .padding(bottom = 60.dp),
                    contentAlignment = Alignment.Center
                ) {
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        Icon(
                            imageVector = Icons.Default.History,
                            contentDescription = null,
                            modifier = Modifier.size(56.dp),
                            tint = ArchDropTheme.colors.systemGray.copy(alpha = 0.5f)
                        )
                        Spacer(modifier = Modifier.height(12.dp))
                        Text(
                            text = if (searchQuery.isNotEmpty()) "No Matching Transfers" else "No Transfer History",
                            style = ArchDropTheme.typography.title3,
                            color = ArchDropTheme.colors.label,
                            fontWeight = FontWeight.SemiBold
                        )
                        Spacer(modifier = Modifier.height(4.dp))
                        Text(
                            text = if (searchQuery.isNotEmpty()) "Check file name or cryptographic hash" else "Direct and mesh transfers will appear here",
                            style = ArchDropTheme.typography.subheadline,
                            color = ArchDropTheme.colors.secondaryLabel
                        )
                    }
                }
            } else {
                LazyColumn(
                    modifier = Modifier.fillMaxSize(),
                    verticalArrangement = Arrangement.spacedBy(10.dp)
                ) {
                    items(filteredTransfers, key = { it.eventId }) { item ->
                        DetailedTransferCard(
                            transfer = item,
                            onClick = { selectedTransferForDetails = item }
                        )
                    }
                    item {
                        Spacer(modifier = Modifier.height(24.dp))
                    }
                }
            }
        }

        SnackbarHost(
            hostState = snackbarHostState,
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .padding(bottom = 16.dp)
        )

        // Details Modal with Cupertino Inset Styling
        if (selectedTransferForDetails != null) {
            val item = selectedTransferForDetails!!
            CupertinoAlertDialog(
                onDismissRequest = { selectedTransferForDetails = null },
                title = item.fileName,
                content = {
                    Column(horizontalAlignment = Alignment.Start) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(
                                text = if (item.isIncoming) "Incoming Transfer" else "Outgoing Transfer",
                                style = ArchDropTheme.typography.headline,
                                color = if (item.isIncoming) ArchDropTheme.colors.systemBlue else ArchDropTheme.colors.systemGreen
                            )
                            Spacer(modifier = Modifier.width(8.dp))
                            if (item.isSuccess) {
                                Text(
                                    text = "• Verified Checksum",
                                    style = ArchDropTheme.typography.caption1,
                                    color = ArchDropTheme.colors.systemGreen
                                )
                            }
                        }

                        Spacer(modifier = Modifier.height(12.dp))
                        Text(
                            text = "Size: ${item.fileSizeFormatted}",
                            style = ArchDropTheme.typography.body,
                            color = ArchDropTheme.colors.label
                        )
                        Spacer(modifier = Modifier.height(2.dp))
                        Text(
                            text = "Time: ${item.timestampFormatted}",
                            style = ArchDropTheme.typography.body,
                            color = ArchDropTheme.colors.secondaryLabel
                        )
                        Spacer(modifier = Modifier.height(14.dp))

                        Text(
                            text = "SHA-256 INTEGRITY DIGEST",
                            style = ArchDropTheme.typography.caption1,
                            fontWeight = FontWeight.Bold,
                            color = ArchDropTheme.colors.secondaryLabel
                        )
                        Spacer(modifier = Modifier.height(6.dp))
                        Box(
                            modifier = Modifier
                                .fillMaxWidth()
                                .clip(ArchDropTheme.shapes.card)
                                .background(ArchDropTheme.colors.tertiarySystemGroupedBackground)
                                .cupertinoHairline(ArchDropTheme.shapes.card)
                                .clickable {
                                    val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                                    val clip = ClipData.newPlainText("SHA-256", item.sha256)
                                    clipboard?.setPrimaryClip(clip)
                                    scope.launch {
                                        snackbarHostState.showSnackbar("SHA-256 digest copied")
                                    }
                                }
                        ) {
                            Row(
                                modifier = Modifier.padding(12.dp),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.CenterVertically
                            ) {
                                Text(
                                    text = item.sha256,
                                    fontFamily = FontFamily.Monospace,
                                    fontSize = 11.sp,
                                    color = ArchDropTheme.colors.label,
                                    modifier = Modifier.weight(1f)
                                )
                                Spacer(modifier = Modifier.width(8.dp))
                                Icon(
                                    imageVector = Icons.Default.ContentCopy,
                                    contentDescription = "Copy",
                                    tint = ArchDropTheme.colors.systemBlue,
                                    modifier = Modifier.size(18.dp)
                                )
                            }
                        }
                    }
                },
                confirmTitle = "Done",
                confirmColor = ArchDropTheme.colors.systemBlue,
                onConfirm = { selectedTransferForDetails = null },
                cancelTitle = null,
                onCancel = null
            )
        }
    }
}

/**
 * Authentic Apple Cupertino Search Field with subtle translucency, hairline border,
 * and 44pt minimum touch ergonomics.
 */
@Composable
fun CupertinoSearchBar(
    query: String,
    onQueryChange: (String) -> Unit,
    onClear: () -> Unit,
    modifier: Modifier = Modifier
) {
    Box(
        modifier = modifier
            .fillMaxWidth()
            .cupertinoTouchTarget(minHeight = 44.dp)
            .clip(ArchDropTheme.shapes.segmentedTrack)
            .background(ArchDropTheme.colors.secondarySystemGroupedBackground)
            .cupertinoHairline(ArchDropTheme.shapes.segmentedTrack)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        contentAlignment = Alignment.CenterStart
    ) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically
        ) {
            Icon(
                imageVector = Icons.Default.Search,
                contentDescription = "Search",
                tint = ArchDropTheme.colors.secondaryLabel,
                modifier = Modifier.size(18.dp)
            )
            Spacer(modifier = Modifier.width(8.dp))
            Box(
                modifier = Modifier.weight(1f),
                contentAlignment = Alignment.CenterStart
            ) {
                if (query.isEmpty()) {
                    Text(
                        text = "Search transfers or hashes...",
                        style = ArchDropTheme.typography.body,
                        color = ArchDropTheme.colors.placeholderText
                    )
                }
                BasicTextField(
                    value = query,
                    onValueChange = onQueryChange,
                    singleLine = true,
                    textStyle = ArchDropTheme.typography.body.copy(color = ArchDropTheme.colors.label),
                    cursorBrush = SolidColor(ArchDropTheme.colors.systemBlue),
                    modifier = Modifier.fillMaxWidth()
                )
            }
            if (query.isNotEmpty()) {
                Box(
                    modifier = Modifier
                        .size(24.dp)
                        .clip(CircleShape)
                        .background(ArchDropTheme.colors.systemGray.copy(alpha = 0.4f))
                        .clickable(onClick = onClear),
                    contentAlignment = Alignment.Center
                ) {
                    Icon(
                        imageVector = Icons.Default.Clear,
                        contentDescription = "Clear",
                        tint = ArchDropTheme.colors.label,
                        modifier = Modifier.size(14.dp)
                    )
                }
            }
        }
    }
}

@Composable
fun DetailedTransferCard(
    transfer: TransferUiModel,
    onClick: () -> Unit
) {
    CupertinoCard(
        shape = ArchDropTheme.shapes.card,
        backgroundColor = ArchDropTheme.colors.secondarySystemGroupedBackground,
        modifier = Modifier
            .fillMaxWidth()
            .clickable { onClick() }
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(14.dp),
            verticalAlignment = Alignment.CenterVertically
        ) {
            Box(
                modifier = Modifier
                    .size(42.dp)
                    .clip(CircleShape)
                    .background(
                        if (transfer.isIncoming) ArchDropTheme.colors.systemBlue.copy(alpha = 0.15f)
                        else ArchDropTheme.colors.systemGreen.copy(alpha = 0.15f)
                    ),
                contentAlignment = Alignment.Center
            ) {
                Icon(
                    imageVector = if (transfer.isIncoming) Icons.Default.ArrowDownward else Icons.Default.ArrowUpward,
                    contentDescription = null,
                    modifier = Modifier.size(20.dp),
                    tint = if (transfer.isIncoming) ArchDropTheme.colors.systemBlue else ArchDropTheme.colors.systemGreen
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
                Spacer(modifier = Modifier.height(2.dp))
                Text(
                    text = "SHA: ${transfer.sha256.take(16)}...",
                    style = ArchDropTheme.typography.caption1,
                    fontFamily = FontFamily.Monospace,
                    fontSize = 11.sp,
                    color = ArchDropTheme.colors.systemBlue
                )
            }

            if (transfer.isSuccess) {
                Spacer(modifier = Modifier.width(8.dp))
                Icon(
                    imageVector = Icons.Default.CheckCircle,
                    contentDescription = "Verified SHA-256",
                    tint = ArchDropTheme.colors.systemGreen,
                    modifier = Modifier.size(20.dp)
                )
            }
        }
    }
}

