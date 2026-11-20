package org.archdrop.ui.theme

import android.os.Build
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

// ============================================================================
// ARCH DROP APPLE HIG DESIGN SYSTEM - CUPERTINO MODIFIERS
// ============================================================================

/**
 * Applies an authentic Apple 0.5dp hairline border for elevated surfaces, cards, and containers.
 *
 * Defaults to ArchDropTheme.colors.hairlineBorder (rgba(255, 255, 255, 0.10) in Dark Mode,
 * rgba(0, 0, 0, 0.15) in Light Mode) and 0.5dp stroke width.
 */
@Composable
fun Modifier.cupertinoHairline(
    color: Color = ArchDropTheme.colors.hairlineBorder,
    shape: Shape = ArchDropTheme.shapes.card,
    strokeWidth: Dp = 0.5.dp
): Modifier = this.border(
    width = strokeWidth,
    color = color,
    shape = shape
)

/**
 * Shape-first overload allowing convenient invocation like:
 * `Modifier.cupertinoHairline(ArchDropTheme.shapes.sheet)`
 */
@Composable
fun Modifier.cupertinoHairline(
    shape: Shape
): Modifier = this.border(
    width = 0.5.dp,
    color = ArchDropTheme.colors.hairlineBorder,
    shape = shape
)

/**
 * Renders a 0.5dp top hairline border.
 * Essential for the Apple 4-tab bottom navigation bar and bottom-docked toolbars.
 */
@Composable
fun Modifier.cupertinoHairlineTop(
    color: Color = ArchDropTheme.colors.hairlineBorder,
    strokeWidth: Dp = 0.5.dp
): Modifier = this.drawBehind {
    val strokePx = strokeWidth.toPx()
    drawLine(
        color = color,
        start = Offset(0f, strokePx / 2f),
        end = Offset(size.width, strokePx / 2f),
        strokeWidth = strokePx
    )
}

/**
 * Renders a 0.5dp bottom hairline border.
 * Essential for top navigation bars and segmented scope headers.
 */
@Composable
fun Modifier.cupertinoHairlineBottom(
    color: Color = ArchDropTheme.colors.hairlineBorder,
    strokeWidth: Dp = 0.5.dp
): Modifier = this.drawBehind {
    val strokePx = strokeWidth.toPx()
    drawLine(
        color = color,
        start = Offset(0f, size.height - strokePx / 2f),
        end = Offset(size.width, size.height - strokePx / 2f),
        strokeWidth = strokePx
    )
}

/**
 * Applies Apple HIG Frosted Glass material styling:
 * - High-luminosity translucent scrim background with configurable alpha (default 0.8f = 80%)
 * - 0.5dp hairline border for edge crispness
 * - Continuous squircle or custom shape clipping
 * - Hardware RenderEffect blur on API 31+ (Android 12+) when blurRadius > 0.dp
 *
 * @param alpha Translucency opacity (default 0.8f for standard frosted material)
 * @param shape Corner geometry (defaults to ArchDropTheme.shapes.card)
 * @param color Base scrim color (defaults to secondarySystemGroupedBackground)
 * @param borderColor Hairline border color (defaults to ArchDropTheme.colors.hairlineBorder)
 * @param borderWidth Hairline border thickness (defaults to 0.5.dp)
 * @param blurRadius Optional hardware blur radius on API 31+ (defaults to 0.dp to keep foreground sharp)
 */
@Composable
fun Modifier.cupertinoFrostedGlass(
    alpha: Float = 0.8f,
    shape: Shape = ArchDropTheme.shapes.card,
    color: Color = ArchDropTheme.colors.secondarySystemGroupedBackground,
    borderColor: Color = ArchDropTheme.colors.hairlineBorder,
    borderWidth: Dp = 0.5.dp,
    blurRadius: Dp = 0.dp
): Modifier {
    val scrimColor = color.copy(alpha = alpha)
    var modifier = this
        .clip(shape)
        .background(scrimColor)

    if (borderWidth > 0.dp) {
        modifier = modifier.border(
            width = borderWidth,
            color = borderColor,
            shape = shape
        )
    }

    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && blurRadius > 0.dp) {
        modifier = modifier.blur(blurRadius)
    }

    return modifier
}

/**
 * Shape-first overload for frosted glass:
 * `Modifier.cupertinoFrostedGlass(ArchDropTheme.shapes.sheet, alpha = 0.85f)`
 */
@Composable
fun Modifier.cupertinoFrostedGlass(
    shape: Shape,
    alpha: Float = 0.8f
): Modifier {
    val scrimColor = ArchDropTheme.colors.secondarySystemGroupedBackground.copy(alpha = alpha)
    return this
        .clip(shape)
        .background(scrimColor)
        .border(
            width = 0.5.dp,
            color = ArchDropTheme.colors.hairlineBorder,
            shape = shape
        )
}

/**
 * Applies floating glass pill styling (e.g. Camera FPS chip, resolution chip, status badge).
 * Uses capsule / CircleShape geometry, 40% translucent black scrim, and 0.5dp hairline border.
 */
@Composable
fun Modifier.cupertinoGlassPill(
    alpha: Float = 0.4f,
    backgroundColor: Color = ArchDropTheme.colors.glassPillBackground,
    borderColor: Color = ArchDropTheme.colors.hairlineBorder
): Modifier = this
    .clip(ArchDropTheme.shapes.capsule)
    .background(backgroundColor.copy(alpha = alpha))
    .border(
        width = 0.5.dp,
        color = borderColor,
        shape = ArchDropTheme.shapes.capsule
    )

/**
 * Enforces strict compliance with Apple HIG touch target ergonomics (minimum 44x44 pt).
 */
fun Modifier.cupertinoTouchTarget(
    minWidth: Dp = 44.dp,
    minHeight: Dp = 44.dp
): Modifier = this.defaultMinSize(
    minWidth = minWidth,
    minHeight = minHeight
)

/**
 * Authentic Apple Cupertino Segmented Control with animated sliding pill thumb.
 */
@Composable
fun CupertinoSegmentedControl(
    items: List<String>,
    selectedIndex: Int,
    onItemSelected: (Int) -> Unit,
    modifier: Modifier = Modifier
) {
    Row(
        modifier = modifier
            .fillMaxWidth()
            .clip(ArchDropTheme.shapes.segmentedTrack)
            .background(ArchDropTheme.colors.tertiarySystemGroupedBackground)
            .border(0.5.dp, ArchDropTheme.colors.hairlineBorder, ArchDropTheme.shapes.segmentedTrack)
            .padding(2.dp),
        verticalAlignment = Alignment.CenterVertically
    ) {
        items.forEachIndexed { index, title ->
            val isSelected = selectedIndex == index
            Box(
                modifier = Modifier
                    .weight(1f)
                    .defaultMinSize(minHeight = 44.dp)
                    .clip(ArchDropTheme.shapes.segmentedThumb)
                    .background(
                        if (isSelected) ArchDropTheme.colors.secondarySystemGroupedBackground
                        else Color.Transparent
                    )
                    .then(
                        if (isSelected) Modifier.border(0.5.dp, ArchDropTheme.colors.hairlineBorder, ArchDropTheme.shapes.segmentedThumb)
                        else Modifier
                    )
                    .clickable(role = Role.Tab) { onItemSelected(index) }
                    .padding(vertical = 8.dp),
                contentAlignment = Alignment.Center
            ) {
                Text(
                    text = title,
                    style = if (isSelected) ArchDropTheme.typography.headline else ArchDropTheme.typography.subheadline,
                    color = if (isSelected) ArchDropTheme.colors.label else ArchDropTheme.colors.secondaryLabel,
                    maxLines = 1
                )
            }
        }
    }
}
