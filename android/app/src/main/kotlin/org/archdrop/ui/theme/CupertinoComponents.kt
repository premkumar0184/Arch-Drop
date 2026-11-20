package org.archdrop.ui.theme

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.animation.core.RepeatMode
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Error
import androidx.compose.material.icons.filled.Hub
import androidx.compose.material.icons.filled.Wifi
import androidx.compose.material.icons.filled.WifiOff
import androidx.compose.material.icons.filled.WifiTethering
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import org.archdrop.transfer.RouteState
import org.archdrop.transfer.RouteTelemetry
import org.archdrop.transfer.RouteTier
import org.archdrop.transfer.TransportRoute

// ============================================================================
// ARCH DROP APPLE HIG DESIGN SYSTEM - PURE CUPERTINO COMPONENT SUITE
// ============================================================================

// ----------------------------------------------------------------------------
// 1. CUPERTINO BOTTOM TAB BAR & TAB ITEM
// ----------------------------------------------------------------------------

/**
 * Authentic Apple iOS Tab Bar:
 * - 50dp height (fits standard iOS 49-50pt bar bounds)
 * - Translucent frosted glass scrim ([ArchDropTheme.colors.glassBarBackground])
 * - 0.5dp top hairline border
 * - Strict 44x44pt touch ergonomics
 */
@Composable
fun CupertinoTabBar(
    modifier: Modifier = Modifier,
    backgroundColor: Color = ArchDropTheme.colors.glassBarBackground,
    borderColor: Color = ArchDropTheme.colors.hairlineBorder,
    content: @Composable RowScope.() -> Unit
) {
    Box(
        modifier = modifier
            .fillMaxWidth()
            .background(backgroundColor)
            .cupertinoHairlineTop(borderColor)
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .height(50.dp)
                .padding(horizontal = 4.dp),
            horizontalArrangement = Arrangement.SpaceAround,
            verticalAlignment = Alignment.CenterVertically,
            content = content
        )
    }
}

/**
 * Authentic Apple Tab Bar Item:
 * - Direct vector icon (24dp) with 10sp caption text below
 * - Active tint: primaryAccent (systemBlue); Inactive tint: systemGray
 * - Zero Material pill or circular background
 * - Subtle tactile haptic on selection
 */
@Composable
fun RowScope.CupertinoTabItem(
    selected: Boolean,
    onClick: () -> Unit,
    icon: ImageVector,
    label: String,
    modifier: Modifier = Modifier,
    selectedColor: Color = ArchDropTheme.colors.primaryAccent,
    unselectedColor: Color = ArchDropTheme.colors.systemGray
) {
    val haptic = LocalHapticFeedback.current
    Column(
        modifier = modifier
            .weight(1f)
            .fillMaxHeight()
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null, // Authentic iOS instant touch without ripple
                role = Role.Tab
            ) {
                haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                onClick()
            }
            .cupertinoTouchTarget(minWidth = 44.dp, minHeight = 44.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center
    ) {
        Icon(
            imageVector = icon,
            contentDescription = label,
            tint = if (selected) selectedColor else unselectedColor,
            modifier = Modifier.size(24.dp)
        )
        Spacer(modifier = Modifier.height(2.dp))
        Text(
            text = label,
            style = ArchDropTheme.typography.caption1.copy(fontSize = 10.sp, lineHeight = 12.sp),
            color = if (selected) selectedColor else unselectedColor,
            fontWeight = if (selected) FontWeight.Medium else FontWeight.Normal,
            maxLines = 1
        )
    }
}

// ----------------------------------------------------------------------------
// 2. CUPERTINO SWITCH (TOGGLE)
// ----------------------------------------------------------------------------

/**
 * Authentic Apple iOS Toggle Switch:
 * - 51dp × 31dp capsule geometry
 * - 27dp circular white thumb with 2dp drop shadow
 * - Green track (#34C759 / #30D158) when active, tertiary grouped track when inactive
 * - Smooth 200ms thumb animation
 * - Tactile haptic feedback on state change
 */
@Composable
fun CupertinoSwitch(
    checked: Boolean,
    onCheckedChange: ((Boolean) -> Unit)?,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    activeColor: Color = ArchDropTheme.colors.systemGreen,
    inactiveColor: Color = ArchDropTheme.colors.tertiarySystemGroupedBackground,
    thumbColor: Color = Color.White
) {
    val haptic = LocalHapticFeedback.current
    val thumbOffset by animateDpAsState(
        targetValue = if (checked) 22.dp else 2.dp,
        animationSpec = tween(durationMillis = 200),
        label = "CupertinoSwitchThumbOffset"
    )

    Box(
        modifier = modifier
            .size(width = 51.dp, height = 31.dp)
            .clip(CircleShape)
            .background(if (checked) activeColor else inactiveColor)
            .border(
                width = 0.5.dp,
                color = if (checked) Color.Transparent else ArchDropTheme.colors.hairlineBorder,
                shape = CircleShape
            )
            .clickable(
                enabled = enabled && onCheckedChange != null,
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
                role = Role.Switch
            ) {
                haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                onCheckedChange?.invoke(!checked)
            },
        contentAlignment = Alignment.CenterStart
    ) {
        Box(
            modifier = Modifier
                .padding(start = thumbOffset)
                .size(27.dp)
                .shadow(elevation = 2.dp, shape = CircleShape)
                .clip(CircleShape)
                .background(thumbColor)
        )
    }
}

// ----------------------------------------------------------------------------
// 3. CUPERTINO ALERT DIALOG
// ----------------------------------------------------------------------------

/**
 * Authentic Apple iOS Alert Dialog:
 * - 270dp fixed width with 14dp squircle corners
 * - Centered headline title (17sp Semibold)
 * - Centered footnote message (13sp Regular, secondaryLabel)
 * - 0.5dp hairline dividers between content and buttons, and between split buttons
 * - 44dp height touch target button rows
 */
@Composable
fun CupertinoAlertDialog(
    onDismissRequest: () -> Unit,
    title: String,
    message: String? = null,
    modifier: Modifier = Modifier,
    content: (@Composable () -> Unit)? = null,
    confirmTitle: String = "OK",
    confirmColor: Color = ArchDropTheme.colors.systemBlue,
    isDestructive: Boolean = false,
    onConfirm: () -> Unit = onDismissRequest,
    cancelTitle: String? = "Cancel",
    onCancel: (() -> Unit)? = onDismissRequest
) {
    Dialog(
        onDismissRequest = onDismissRequest,
        properties = DialogProperties(usePlatformDefaultWidth = false)
    ) {
        Box(
            modifier = Modifier
                .fillMaxSize()
                .background(Color.Black.copy(alpha = 0.5f))
                .clickable(
                    interactionSource = remember { MutableInteractionSource() },
                    indication = null,
                    onClick = onDismissRequest
                ),
            contentAlignment = Alignment.Center
        ) {
            Box(
                modifier = modifier
                    .width(270.dp)
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                        onClick = {} // Intercept click inside dialog
                    )
                    .clip(RoundedCornerShape(14.dp))
                    .background(ArchDropTheme.colors.secondarySystemGroupedBackground)
                    .border(0.5.dp, ArchDropTheme.colors.hairlineBorder, RoundedCornerShape(14.dp))
            ) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Column(
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 20.dp),
                        horizontalAlignment = Alignment.CenterHorizontally
                    ) {
                        Text(
                            text = title,
                            style = ArchDropTheme.typography.headline,
                            color = ArchDropTheme.colors.label,
                            fontWeight = FontWeight.SemiBold,
                            textAlign = TextAlign.Center
                        )
                        if (message != null) {
                            Spacer(modifier = Modifier.height(4.dp))
                            Text(
                                text = message,
                                style = ArchDropTheme.typography.footnote,
                                color = ArchDropTheme.colors.secondaryLabel,
                                textAlign = TextAlign.Center
                            )
                        }
                        if (content != null) {
                            Spacer(modifier = Modifier.height(12.dp))
                            content()
                        }
                    }

                    // 0.5dp hairline divider
                    Box(
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(0.5.dp)
                            .background(ArchDropTheme.colors.separator)
                    )

                    // Action Buttons (1 or 2 buttons)
                    if (cancelTitle != null && onCancel != null) {
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .height(44.dp)
                        ) {
                            // Cancel Button
                            Box(
                                modifier = Modifier
                                    .weight(1f)
                                    .fillMaxHeight()
                                    .clickable(role = Role.Button) { onCancel() },
                                contentAlignment = Alignment.Center
                            ) {
                                Text(
                                    text = cancelTitle,
                                    style = ArchDropTheme.typography.body,
                                    color = ArchDropTheme.colors.systemBlue,
                                    fontWeight = FontWeight.Normal
                                )
                            }

                            // Vertical 0.5dp divider
                            Box(
                                modifier = Modifier
                                    .width(0.5.dp)
                                    .fillMaxHeight()
                                    .background(ArchDropTheme.colors.separator)
                            )

                            // Confirm Button
                            Box(
                                modifier = Modifier
                                    .weight(1f)
                                    .fillMaxHeight()
                                    .clickable(role = Role.Button) { onConfirm() },
                                contentAlignment = Alignment.Center
                            ) {
                                Text(
                                    text = confirmTitle,
                                    style = ArchDropTheme.typography.headline,
                                    color = if (isDestructive) ArchDropTheme.colors.systemRed else confirmColor,
                                    fontWeight = FontWeight.SemiBold
                                )
                            }
                        }
                    } else {
                        // Single Confirm Button
                        Box(
                            modifier = Modifier
                                .fillMaxWidth()
                                .height(44.dp)
                                .clickable(role = Role.Button) { onConfirm() },
                            contentAlignment = Alignment.Center
                        ) {
                            Text(
                                text = confirmTitle,
                                style = ArchDropTheme.typography.headline,
                                color = if (isDestructive) ArchDropTheme.colors.systemRed else confirmColor,
                                fontWeight = FontWeight.SemiBold
                            )
                        }
                    }
                }
            }
        }
    }
}

// ----------------------------------------------------------------------------
// 4. CUPERTINO BUTTON
// ----------------------------------------------------------------------------

enum class CupertinoButtonStyle {
    FILLED,      // systemBlue filled with white text
    TONAL,       // secondarySystemGroupedBackground with hairline border
    PLAIN,       // transparent background with systemBlue text
    DESTRUCTIVE  // systemRed filled or systemRed text
}

/**
 * Authentic Apple iOS Button:
 * - Continuous squircle geometry (8dp–14dp)
 * - Strict minimum 44pt touch target
 * - Smooth opacity reduction feedback on press (0.7f) without Material ripple
 */
@Composable
fun CupertinoButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    style: CupertinoButtonStyle = CupertinoButtonStyle.FILLED,
    shape: Shape = ArchDropTheme.shapes.buttonLarge,
    enabled: Boolean = true,
    content: @Composable RowScope.() -> Unit
) {
    val haptic = LocalHapticFeedback.current
    var isPressed by remember { mutableStateOf(false) }

    val backgroundColor = when (style) {
        CupertinoButtonStyle.FILLED -> ArchDropTheme.colors.systemBlue
        CupertinoButtonStyle.TONAL -> ArchDropTheme.colors.secondarySystemGroupedBackground
        CupertinoButtonStyle.PLAIN -> Color.Transparent
        CupertinoButtonStyle.DESTRUCTIVE -> ArchDropTheme.colors.systemRed
    }

    val contentColor = when (style) {
        CupertinoButtonStyle.FILLED -> Color.White
        CupertinoButtonStyle.TONAL -> ArchDropTheme.colors.label
        CupertinoButtonStyle.PLAIN -> ArchDropTheme.colors.systemBlue
        CupertinoButtonStyle.DESTRUCTIVE -> Color.White
    }

    val currentAlpha = if (!enabled) 0.4f else if (isPressed) 0.7f else 1.0f

    Box(
        modifier = modifier
            .defaultMinSize(minHeight = 44.dp)
            .clip(shape)
            .background(backgroundColor.copy(alpha = if (style == CupertinoButtonStyle.PLAIN) 0f else currentAlpha))
            .then(
                if (style == CupertinoButtonStyle.TONAL) Modifier.cupertinoHairline(shape)
                else Modifier
            )
            .pointerInput(enabled) {
                if (enabled) {
                    detectTapGestures(
                        onPress = {
                            isPressed = true
                            tryAwaitRelease()
                            isPressed = false
                        },
                        onTap = {
                            haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                            onClick()
                        }
                    )
                }
            }
            .padding(horizontal = 16.dp, vertical = 10.dp),
        contentAlignment = Alignment.Center
    ) {
        Row(
            horizontalArrangement = Arrangement.Center,
            verticalAlignment = Alignment.CenterVertically,
            content = content
        )
    }
}

// ----------------------------------------------------------------------------
// 5. CUPERTINO INSET GROUPED CARD & SECTIONS
// ----------------------------------------------------------------------------

/**
 * Authentic Apple Inset Grouped Card Container:
 * - 16dp continuous squircle corners ([ArchDropTheme.shapes.card])
 * - Secondary grouped background (#1C1C1E dark / #FFFFFF light)
 * - 0.5dp hairline border
 */
@Composable
fun CupertinoCard(
    modifier: Modifier = Modifier,
    shape: Shape = ArchDropTheme.shapes.card,
    backgroundColor: Color = ArchDropTheme.colors.secondarySystemGroupedBackground,
    borderColor: Color = ArchDropTheme.colors.hairlineBorder,
    borderWidth: Dp = 0.5.dp,
    content: @Composable ColumnScope.() -> Unit
) {
    Column(
        modifier = modifier
            .fillMaxWidth()
            .clip(shape)
            .background(backgroundColor)
            .border(borderWidth, borderColor, shape),
        content = content
    )
}

// ----------------------------------------------------------------------------
// 6. CUPERTINO TEXT FIELD
// ----------------------------------------------------------------------------

/**
 * Authentic Apple iOS Text Input:
 * - 10dp squircle container
 * - 44pt touch target ergonomics
 * - Subtle background track with hairline border
 * - systemBlue cursor
 */
@Composable
fun CupertinoTextField(
    value: String,
    onValueChange: (String) -> Unit,
    modifier: Modifier = Modifier,
    placeholder: String = "",
    singleLine: Boolean = true,
    enabled: Boolean = true
) {
    Box(
        modifier = modifier
            .fillMaxWidth()
            .defaultMinSize(minHeight = 44.dp)
            .clip(ArchDropTheme.shapes.buttonMedium)
            .background(ArchDropTheme.colors.tertiarySystemGroupedBackground)
            .border(0.5.dp, ArchDropTheme.colors.hairlineBorder, ArchDropTheme.shapes.buttonMedium)
            .padding(horizontal = 14.dp, vertical = 10.dp),
        contentAlignment = Alignment.CenterStart
    ) {
        if (value.isEmpty()) {
            Text(
                text = placeholder,
                style = ArchDropTheme.typography.body,
                color = ArchDropTheme.colors.placeholderText
            )
        }
        BasicTextField(
            value = value,
            onValueChange = onValueChange,
            singleLine = singleLine,
            enabled = enabled,
            textStyle = ArchDropTheme.typography.body.copy(color = ArchDropTheme.colors.label),
            cursorBrush = SolidColor(ArchDropTheme.colors.systemBlue),
            modifier = Modifier.fillMaxWidth()
        )
    }
}

// ----------------------------------------------------------------------------
// 7. CUPERTINO ICON BUTTON
// ----------------------------------------------------------------------------

/**
 * Authentic Apple iOS Icon Button:
 * - 44×44pt touch target compliance
 * - Instant touch feedback with 0.6f pressed opacity (no Material ripple)
 * - Tactile haptic feedback on selection
 */
@Composable
fun CupertinoIconButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    backgroundColor: Color = Color.Transparent,
    shape: Shape = CircleShape,
    content: @Composable () -> Unit
) {
    val haptic = LocalHapticFeedback.current
    var isPressed by remember { mutableStateOf(false) }
    val currentAlpha = if (!enabled) 0.4f else if (isPressed) 0.6f else 1.0f

    Box(
        modifier = modifier
            .defaultMinSize(minWidth = 44.dp, minHeight = 44.dp)
            .clip(shape)
            .background(backgroundColor.copy(alpha = currentAlpha))
            .pointerInput(enabled) {
                if (enabled) {
                    detectTapGestures(
                        onPress = {
                            isPressed = true
                            tryAwaitRelease()
                            isPressed = false
                        },
                        onTap = {
                            haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                            onClick()
                        }
                    )
                }
            },
        contentAlignment = Alignment.Center
    ) {
        content()
    }
}

// ----------------------------------------------------------------------------
// 8. CUPERTINO SLIDER
// ----------------------------------------------------------------------------

/**
 * Authentic Apple iOS Continuous Track Slider:
 * - 4dp continuous track with rounded capsule ends
 * - 28dp circular white thumb with 2dp drop shadow
 * - Full 44pt touch ergonomics with smooth drag and tap gestures
 * - Dynamic color transitions with accessibility range semantics
 */
@Composable
fun CupertinoSlider(
    value: Float,
    onValueChange: (Float) -> Unit,
    modifier: Modifier = Modifier,
    valueRange: ClosedFloatingPointRange<Float> = 0f..1f,
    enabled: Boolean = true,
    activeTrackColor: Color = ArchDropTheme.colors.primaryAccent,
    inactiveTrackColor: Color = ArchDropTheme.colors.tertiarySystemGroupedBackground,
    thumbColor: Color = Color.White
) {
    val haptic = LocalHapticFeedback.current
    val rangeSpan = (valueRange.endInclusive - valueRange.start).coerceAtLeast(0.0001f)
    val normalizedValue = ((value - valueRange.start) / rangeSpan).coerceIn(0f, 1f)

    BoxWithConstraints(
        modifier = modifier
            .fillMaxWidth()
            .defaultMinSize(minHeight = 44.dp)
            .semantics(mergeDescendants = true) {
                progressBarRangeInfo = ProgressBarRangeInfo(
                    current = value,
                    range = valueRange
                )
            },
        contentAlignment = Alignment.CenterStart
    ) {
        val totalWidthPx = constraints.maxWidth.toFloat()
        val density = LocalDensity.current
        val thumbRadiusPx = with(density) { 14.dp.toPx() }
        val usableTrackWidthPx = (totalWidthPx - 2 * thumbRadiusPx).coerceAtLeast(1f)

        fun updateFromX(x: Float) {
            val clampedX = (x - thumbRadiusPx).coerceIn(0f, usableTrackWidthPx)
            val fraction = clampedX / usableTrackWidthPx
            val newValue = valueRange.start + fraction * rangeSpan
            onValueChange(newValue)
        }

        val thumbOffsetDp = with(density) {
            (thumbRadiusPx + normalizedValue * usableTrackWidthPx - thumbRadiusPx).toDp()
        }

        Box(
            modifier = Modifier
                .fillMaxWidth()
                .height(44.dp)
                .pointerInput(enabled, totalWidthPx) {
                    if (enabled) {
                        detectTapGestures { offset ->
                            haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                            updateFromX(offset.x)
                        }
                    }
                }
                .pointerInput(enabled, totalWidthPx) {
                    if (enabled) {
                        detectDragGestures(
                            onDrag = { change, _ ->
                                change.consume()
                                updateFromX(change.position.x)
                            }
                        )
                    }
                },
            contentAlignment = Alignment.CenterStart
        ) {
            // Inactive Track Background (4dp capsule)
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .height(4.dp)
                    .clip(CircleShape)
                    .background(inactiveTrackColor)
            )

            // Active Track (4dp capsule)
            Box(
                modifier = Modifier
                    .fillMaxWidth(fraction = normalizedValue.coerceIn(0.01f, 1f))
                    .height(4.dp)
                    .clip(CircleShape)
                    .background(if (enabled) activeTrackColor else activeTrackColor.copy(alpha = 0.5f))
            )

            // Thumb (28dp circular white disk with 2dp shadow)
            Box(
                modifier = Modifier
                    .padding(start = thumbOffsetDp)
                    .size(28.dp)
                    .shadow(elevation = 2.dp, shape = CircleShape)
                    .clip(CircleShape)
                    .background(if (enabled) thumbColor else thumbColor.copy(alpha = 0.6f))
            )
        }
    }
}

// ----------------------------------------------------------------------------
// 9. CUPERTINO ACTIVITY INDICATOR (SPINNER)
// ----------------------------------------------------------------------------

/**
 * Authentic Apple iOS Spinner Activity Indicator:
 * - Smooth rotating 280-degree circular arc with rounded caps
 * - Robolectric safe (disables continuous animation in unit test environment)
 */
@Composable
fun CupertinoActivityIndicator(
    modifier: Modifier = Modifier,
    color: Color = ArchDropTheme.colors.primaryAccent,
    strokeWidth: Dp = 2.dp,
    size: Dp = 20.dp
) {
    val isRobolectric = remember {
        android.os.Build.FINGERPRINT == "robolectric" || android.os.Build.HARDWARE == "robolectric"
    }
    val rotation = if (isRobolectric) {
        0f
    } else {
        val infiniteTransition = rememberInfiniteTransition(label = "CupertinoActivityIndicatorTransition")
        val angle by infiniteTransition.animateFloat(
            initialValue = 0f,
            targetValue = 360f,
            animationSpec = infiniteRepeatable(
                animation = tween(durationMillis = 1000, easing = LinearEasing)
            ),
            label = "CupertinoActivityIndicatorRotation"
        )
        angle
    }

    Canvas(
        modifier = modifier
            .size(size)
            .graphicsLayer { rotationZ = rotation }
    ) {
        val stroke = Stroke(
            width = strokeWidth.toPx(),
            cap = StrokeCap.Round
        )
        drawArc(
            color = color,
            startAngle = 0f,
            sweepAngle = 280f,
            useCenter = false,
            style = stroke
        )
    }
}

// ----------------------------------------------------------------------------
// 10. CUPERTINO DIVIDER / HAIRLINE SEPARATOR
// ----------------------------------------------------------------------------

/**
 * Authentic Apple iOS 0.5dp Separator / Divider:
 * - Subtle hairline divider conforming to iOS Inset Grouped lists
 */
@Composable
fun CupertinoDivider(
    modifier: Modifier = Modifier,
    color: Color = ArchDropTheme.colors.separator,
    thickness: Dp = 0.5.dp
) {
    Box(
        modifier = modifier
            .fillMaxWidth()
            .height(thickness)
            .background(color)
    )
}

// ----------------------------------------------------------------------------
// 11. CUPERTINO SCAFFOLD
// ----------------------------------------------------------------------------

/**
 * Authentic Apple iOS Screen Scaffold:
 * - Canvas background color ([ArchDropTheme.colors.canvasBackground])
 * - Top navigation bar slot
 * - Bottom tab bar slot (50dp height)
 * - Passes [PaddingValues] to content for safe inset scrolling
 */
@Composable
fun CupertinoScaffold(
    modifier: Modifier = Modifier,
    containerColor: Color = ArchDropTheme.colors.canvasBackground,
    topBar: (@Composable () -> Unit)? = null,
    bottomBar: (@Composable () -> Unit)? = null,
    content: @Composable (PaddingValues) -> Unit
) {
    val statusBarHeight = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
    val navBarHeight = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()

    Box(
        modifier = modifier
            .fillMaxSize()
            .background(containerColor)
    ) {
        val bottomBarHeight = if (bottomBar != null) 50.dp else 0.dp
        val topBarHeight = if (topBar != null) 44.dp else 0.dp
        val totalTopPadding = statusBarHeight + topBarHeight
        val totalBottomPadding = navBarHeight + bottomBarHeight
        val paddingValues = PaddingValues(top = totalTopPadding, bottom = totalBottomPadding)

        content(paddingValues)

        if (topBar != null) {
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(top = statusBarHeight)
                    .align(Alignment.TopCenter)
            ) {
                topBar()
            }
        }

        if (bottomBar != null) {
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .align(Alignment.BottomCenter)
            ) {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(ArchDropTheme.colors.glassBarBackground)
                ) {
                    bottomBar()
                    Spacer(modifier = Modifier.height(navBarHeight))
                }
            }
        }
    }
}

// ----------------------------------------------------------------------------
// 12. CUPERTINO SHEET GRABBER & MODAL BOTTOM SHEET
// ----------------------------------------------------------------------------

/**
 * Authentic Apple grabber indicator pill (36dp x 5dp capsule).
 */
@Composable
fun CupertinoSheetGrabber(modifier: Modifier = Modifier) {
    Box(
        modifier = modifier
            .fillMaxWidth()
            .padding(top = 8.dp, bottom = 4.dp),
        contentAlignment = Alignment.Center
    ) {
        Box(
            modifier = Modifier
                .width(36.dp)
                .height(5.dp)
                .clip(ArchDropTheme.shapes.capsule)
                .background(ArchDropTheme.colors.systemGray3)
        )
    }
}

/**
 * Authentic Apple iOS Modal Bottom Sheet:
 * - 24dp top squircle corners ([ArchDropTheme.shapes.sheetSquircle])
 * - Secondary grouped background styling
 * - Apple grabber indicator at top
 * - Dismiss on tap outside or back press
 */
@Composable
fun CupertinoModalBottomSheet(
    onDismissRequest: () -> Unit,
    modifier: Modifier = Modifier,
    dragHandle: (@Composable () -> Unit)? = { CupertinoSheetGrabber() },
    content: @Composable ColumnScope.() -> Unit
) {
    Dialog(
        onDismissRequest = onDismissRequest,
        properties = DialogProperties(
            usePlatformDefaultWidth = false,
            dismissOnBackPress = true,
            dismissOnClickOutside = true
        )
    ) {
        Box(
            modifier = Modifier
                .fillMaxSize()
                .background(Color.Black.copy(alpha = 0.5f))
                .clickable(
                    interactionSource = remember { MutableInteractionSource() },
                    indication = null,
                    onClick = onDismissRequest
                ),
            contentAlignment = Alignment.BottomCenter
        ) {
            Column(
                modifier = modifier
                    .fillMaxWidth()
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                        onClick = {} // Intercept touch inside sheet content
                    )
                    .clip(ArchDropTheme.shapes.sheetSquircle)
                    .background(ArchDropTheme.colors.secondarySystemGroupedBackground)
                    .cupertinoHairline(ArchDropTheme.shapes.sheetSquircle),
                content = {
                    if (dragHandle != null) {
                        dragHandle()
                    }
                    content()
                }
            )
        }
    }
}

// ============================================================================
// 14. CUPERTINO CONNECTION BADGE (APPLE HIG STATUS PILL)
// ============================================================================

/**
 * Pure visual configuration for CupertinoConnectionBadge.
 */
data class BadgeVisualConfig(
    val label: String,
    val tintColor: Color,
    val icon: ImageVector,
    val contentDescription: String,
    val isPulsing: Boolean = false,
    val showIndicatorDot: Boolean = true,
    val labelColor: Color = Color.Unspecified
)

object CupertinoConnectionBadgeDefaults {
    /**
     * Resolves the high-contrast label color strictly adhering to WCAG 2.1 AAA standards:
     * - Dark Mode: Color.White (> 12.8:1 contrast across all states)
     * - Light Mode: colors.label (#000000, > 18.4:1 contrast across all states)
     */
    fun resolveLabelColor(colors: CupertinoColors): Color =
        if (colors.isDark) Color.White else colors.label

    /**
     * Resolves the visual configuration deterministically based on RouteTier and RouteState.
     */
    fun resolveVisualConfig(
        tier: RouteTier?,
        state: RouteState,
        colors: CupertinoColors,
        customLabel: String? = null
    ): BadgeVisualConfig {
        val labelColor = resolveLabelColor(colors)
        if (customLabel != null) {
            return BadgeVisualConfig(
                label = customLabel,
                tintColor = colors.primaryAccent,
                icon = Icons.Default.Wifi,
                contentDescription = "Connection Status: $customLabel",
                isPulsing = false,
                labelColor = labelColor
            )
        }

        return when (state) {
            RouteState.IDLE -> {
                val label = tier?.let { "${it.displayName} (Standby)" } ?: "Idle"
                BadgeVisualConfig(
                    label = label,
                    tintColor = colors.systemGray,
                    icon = Icons.Default.WifiOff,
                    contentDescription = "Connection Status: $label",
                    isPulsing = false,
                    labelColor = labelColor
                )
            }
            RouteState.PROBING -> {
                val label = tier?.let { "${it.displayName} (Probing)" } ?: "Connecting..."
                BadgeVisualConfig(
                    label = label,
                    tintColor = colors.systemYellow,
                    icon = tier?.let { getTierIcon(it) } ?: Icons.Default.Wifi,
                    contentDescription = "Connection Status: $label",
                    isPulsing = true,
                    labelColor = labelColor
                )
            }
            RouteState.ACTIVE -> {
                val activeTier = tier ?: RouteTier.DIRECT_LAN
                val tint = when (activeTier) {
                    RouteTier.DIRECT_LAN -> colors.systemGreen
                    RouteTier.PEER_TO_PEER -> colors.primaryAccent
                    RouteTier.CLOUD_RELAY -> colors.systemPurple
                }
                BadgeVisualConfig(
                    label = activeTier.displayName,
                    tintColor = tint,
                    icon = getTierIcon(activeTier),
                    contentDescription = "Connection Status: ${activeTier.displayName}, Connected",
                    isPulsing = false,
                    labelColor = labelColor
                )
            }
            RouteState.DEGRADED -> {
                val label = tier?.let { "${it.displayName} (Degraded)" } ?: "Degraded"
                BadgeVisualConfig(
                    label = label,
                    tintColor = colors.systemOrange,
                    icon = tier?.let { getTierIcon(it) } ?: Icons.Default.Wifi,
                    contentDescription = "Connection Status: $label",
                    isPulsing = false,
                    labelColor = labelColor
                )
            }
            RouteState.FAILED -> {
                BadgeVisualConfig(
                    label = "Disconnected",
                    tintColor = colors.systemRed,
                    icon = Icons.Default.Error,
                    contentDescription = "Connection Status: Disconnected",
                    isPulsing = false,
                    labelColor = labelColor
                )
            }
        }
    }

    fun getTierIcon(tier: RouteTier): ImageVector = when (tier) {
        RouteTier.DIRECT_LAN -> Icons.Default.Wifi
        RouteTier.PEER_TO_PEER -> Icons.Default.WifiTethering
        RouteTier.CLOUD_RELAY -> Icons.Default.Hub
    }

    val RouteTier.displayName: String
        get() = when (this) {
            RouteTier.DIRECT_LAN -> "Direct LAN"
            RouteTier.PEER_TO_PEER -> "Peer-to-Peer"
            RouteTier.CLOUD_RELAY -> "Cloud Relay"
        }
}

/**
 * Authentic Apple HIG Capsule Connection Badge:
 * - 26dp height capsule geometry (CircleShape)
 * - 0.5dp hairline border tinted to match active tier
 * - Translucent tinted glass fill (12% opacity in Dark Mode, 10% in Light Mode)
 * - Status indicator dot + vector icon + SF Pro caption2 typography
 * - WCAG 2.1 AA / AAA compliant contrast ratio (>= 4.5:1, up to 15:1 against card surface)
 * - Pulsing alpha breathing animation during RouteState.PROBING (Robolectric safe)
 * - Min 44x44 pt touch target when clickable (Apple HIG touch target standard)
 */
@Composable
fun CupertinoConnectionBadge(
    tier: RouteTier?,
    state: RouteState = RouteState.IDLE,
    modifier: Modifier = Modifier,
    customLabel: String? = null,
    onClick: (() -> Unit)? = null,
    showDot: Boolean = true
) {
    val colors = ArchDropTheme.colors
    val haptic = LocalHapticFeedback.current
    val config = remember(tier, state, colors, customLabel) {
        CupertinoConnectionBadgeDefaults.resolveVisualConfig(tier, state, colors, customLabel)
    }

    val isRobolectric = remember {
        android.os.Build.FINGERPRINT == "robolectric" || android.os.Build.HARDWARE == "robolectric"
    }

    val pulseAlpha = if (config.isPulsing && !isRobolectric) {
        val infiniteTransition = rememberInfiniteTransition(label = "CupertinoBadgePulse")
        val alpha by infiniteTransition.animateFloat(
            initialValue = 0.40f,
            targetValue = 1.00f,
            animationSpec = infiniteRepeatable(
                animation = tween(durationMillis = 800, easing = LinearEasing),
                repeatMode = RepeatMode.Reverse
            ),
            label = "CupertinoBadgeAlphaPulse"
        )
        alpha
    } else {
        1.0f
    }

    val backgroundAlpha = if (colors.isDark) 0.12f else 0.10f
    val borderAlpha = if (colors.isDark) 0.28f else 0.22f
    val backgroundColor = config.tintColor.copy(alpha = backgroundAlpha * pulseAlpha)
    val borderColor = config.tintColor.copy(alpha = borderAlpha * pulseAlpha)

    // High-contrast label color strictly adhering to WCAG 2.1 AAA standards across all states:
    // In Dark Mode: Color.White guarantees > 12.8:1 to 14.9:1 contrast against dark cards and OLED black.
    // In Light Mode: colors.label (#000000) guarantees > 18.4:1 to 20.1:1 contrast against white/grouped cards.
    // Active route tier and state remain distinct and vibrant via the 6dp indicator dot,
    // 13dp vector icon tint, 0.5dp hairline border tint, and translucent background wash.
    val labelColor = if (colors.isDark) {
        Color.White
    } else {
        colors.label
    }

    val isClickable = onClick != null

    Box(
        modifier = modifier
            .then(
                if (isClickable) {
                    Modifier
                        .defaultMinSize(minWidth = 44.dp, minHeight = 44.dp)
                        .clickable(
                            interactionSource = remember { MutableInteractionSource() },
                            indication = null,
                            role = Role.Button
                        ) {
                            haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                            onClick()
                        }
                } else {
                    Modifier
                }
            ),
        contentAlignment = Alignment.Center
    ) {
        Row(
            modifier = Modifier
                .defaultMinSize(minHeight = 26.dp)
                .clip(ArchDropTheme.shapes.capsule)
                .background(backgroundColor)
                .border(width = 0.5.dp, color = borderColor, shape = ArchDropTheme.shapes.capsule)
                .semantics(mergeDescendants = true) {
                    contentDescription = config.contentDescription
                }
                .padding(horizontal = 10.dp, vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.Center
        ) {
            if (showDot && config.showIndicatorDot) {
                Box(
                    modifier = Modifier
                        .size(6.dp)
                        .graphicsLayer { alpha = pulseAlpha }
                        .clip(CircleShape)
                        .background(config.tintColor)
                )
                Spacer(modifier = Modifier.width(6.dp))
            }

            Icon(
                imageVector = config.icon,
                contentDescription = null,
                tint = config.tintColor.copy(alpha = pulseAlpha),
                modifier = Modifier.size(13.dp)
            )

            Spacer(modifier = Modifier.width(5.dp))

            Text(
                text = config.label,
                style = ArchDropTheme.typography.caption2,
                color = labelColor,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1
            )
        }
    }
}

/**
 * Convenience overload accepting [TransportRoute] directly.
 */
@Composable
fun CupertinoConnectionBadge(
    route: TransportRoute?,
    routeState: RouteState = RouteState.IDLE,
    modifier: Modifier = Modifier,
    customLabel: String? = null,
    onClick: (() -> Unit)? = null,
    showDot: Boolean = true
) = CupertinoConnectionBadge(
    tier = route?.tier,
    state = routeState,
    modifier = modifier,
    customLabel = customLabel,
    onClick = onClick,
    showDot = showDot
)

/**
 * Convenience overload accepting [RouteTelemetry] directly.
 */
@Composable
fun CupertinoConnectionBadge(
    telemetry: RouteTelemetry?,
    modifier: Modifier = Modifier,
    customLabel: String? = null,
    onClick: (() -> Unit)? = null,
    showDot: Boolean = true
) = CupertinoConnectionBadge(
    tier = telemetry?.route?.tier,
    state = telemetry?.state ?: RouteState.IDLE,
    modifier = modifier,
    customLabel = customLabel,
    onClick = onClick,
    showDot = showDot
)



