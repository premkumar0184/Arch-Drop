package org.archdrop.ui.theme

import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Shapes
import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.unit.dp

// ============================================================================
// ARCH DROP APPLE HIG DESIGN SYSTEM - SQUIRCLE SHAPE TOKENS
// ============================================================================

/**
 * Authentic Apple continuous squircle corner tokens for elevated cards,
 * modals, segmented controls, and pill elements.
 */
@Immutable
data class CupertinoShapes(
    val buttonSmall: Shape = RoundedCornerShape(8.dp),
    val buttonMedium: Shape = RoundedCornerShape(12.dp),
    val buttonLarge: Shape = RoundedCornerShape(14.dp),
    val card: Shape = RoundedCornerShape(16.dp),         // Standard Inset Grouped Card
    val cardHero: Shape = RoundedCornerShape(20.dp),     // Prominent Hero Card (AirDrop radar, AirPlay card)
    val sheet: Shape = RoundedCornerShape(topStart = 24.dp, topEnd = 24.dp), // Modal Bottom Sheet
    val dialog: Shape = RoundedCornerShape(20.dp),       // iOS System Alert / Dialog
    val segmentedTrack: Shape = RoundedCornerShape(10.dp), // Segmented Control background track
    val segmentedThumb: Shape = RoundedCornerShape(8.dp),  // Segmented Control active thumb
    val capsule: Shape = CircleShape,                    // Floating Pill / Glass Tag
    val circle: Shape = CircleShape                      // Avatar / Round Action
) {
    // Interface contract aliases per PROJECT.md line 104
    val cardSquircle: Shape get() = card
    val heroSquircle: Shape get() = cardHero
    val sheetSquircle: Shape get() = sheet
    val controlSquircle: Shape get() = segmentedTrack
}

val DefaultCupertinoShapes = CupertinoShapes()

/**
 * Bridges CupertinoShapes to Material 3 Shapes hierarchy.
 */
val ArchDropShapes = Shapes(
    extraSmall = RoundedCornerShape(6.dp),
    small = RoundedCornerShape(8.dp),
    medium = RoundedCornerShape(12.dp),
    large = RoundedCornerShape(16.dp),
    extraLarge = RoundedCornerShape(20.dp)
)
