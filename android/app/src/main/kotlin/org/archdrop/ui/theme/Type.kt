package org.archdrop.ui.theme

import androidx.compose.material3.Typography
import androidx.compose.runtime.Immutable
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

// ============================================================================
// ARCH DROP APPLE HIG DESIGN SYSTEM - SF PRO TYPOGRAPHY SCALE
// ============================================================================

/**
 * Authentic Apple SF Pro Dynamic Type scale for Jetpack Compose.
 * Calibrated against Apple Human Interface Guidelines for iOS:
 * - Exact point sizes (sp)
 * - Calibrated line heights (leading)
 * - Optical letter spacing (tracking in sp derived from Apple 1/1000 em tables)
 */
@Immutable
data class CupertinoTypography(
    val largeTitle: TextStyle,
    val title1: TextStyle,
    val title2: TextStyle,
    val title3: TextStyle,
    val headline: TextStyle,
    val body: TextStyle,
    val callout: TextStyle,
    val subheadline: TextStyle,
    val footnote: TextStyle,
    val caption1: TextStyle,
    val caption2: TextStyle,
    val monospaceHex: TextStyle
)

val DefaultCupertinoTypography = CupertinoTypography(
    largeTitle = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Bold,
        fontSize = 34.sp,
        lineHeight = 41.sp,
        letterSpacing = 0.40.sp // +12 / 1000 em
    ),
    title1 = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Bold,
        fontSize = 28.sp,
        lineHeight = 34.sp,
        letterSpacing = 0.38.sp // +14 / 1000 em
    ),
    title2 = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Bold,
        fontSize = 22.sp,
        lineHeight = 28.sp,
        letterSpacing = (-0.26).sp // -12 / 1000 em
    ),
    title3 = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.SemiBold,
        fontSize = 20.sp,
        lineHeight = 25.sp,
        letterSpacing = (-0.45).sp // -23 / 1000 em
    ),
    headline = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.SemiBold,
        fontSize = 17.sp,
        lineHeight = 22.sp,
        letterSpacing = (-0.43).sp // -26 / 1000 em
    ),
    body = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Normal,
        fontSize = 17.sp,
        lineHeight = 22.sp,
        letterSpacing = (-0.43).sp // -26 / 1000 em
    ),
    callout = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Normal,
        fontSize = 16.sp,
        lineHeight = 21.sp,
        letterSpacing = (-0.31).sp // -20 / 1000 em
    ),
    subheadline = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Normal,
        fontSize = 15.sp,
        lineHeight = 20.sp,
        letterSpacing = (-0.23).sp // -16 / 1000 em
    ),
    footnote = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Normal,
        fontSize = 13.sp,
        lineHeight = 18.sp,
        letterSpacing = (-0.08).sp // -6 / 1000 em
    ),
    caption1 = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Normal,
        fontSize = 12.sp,
        lineHeight = 16.sp,
        letterSpacing = 0.0.sp
    ),
    caption2 = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Medium,
        fontSize = 11.sp,
        lineHeight = 13.sp,
        letterSpacing = 0.06.sp // +6 / 1000 em
    ),
    monospaceHex = TextStyle(
        fontFamily = FontFamily.Monospace,
        fontWeight = FontWeight.Normal,
        fontSize = 12.sp,
        lineHeight = 16.sp,
        letterSpacing = 0.5.sp
    )
)

/**
 * Bridges CupertinoTypography to Material 3 Typography so stock Compose M3
 * components (Text, Button, Card) seamlessly inherit authentic SF Pro metrics.
 */
val ArchDropTypography = Typography(
    displayLarge = DefaultCupertinoTypography.largeTitle,
    displayMedium = DefaultCupertinoTypography.largeTitle,
    displaySmall = DefaultCupertinoTypography.title1,
    headlineLarge = DefaultCupertinoTypography.title1,
    headlineMedium = DefaultCupertinoTypography.title2,
    headlineSmall = DefaultCupertinoTypography.title3,
    titleLarge = DefaultCupertinoTypography.title3,
    titleMedium = DefaultCupertinoTypography.headline,
    titleSmall = DefaultCupertinoTypography.callout,
    bodyLarge = DefaultCupertinoTypography.body,
    bodyMedium = DefaultCupertinoTypography.subheadline,
    bodySmall = DefaultCupertinoTypography.footnote,
    labelLarge = DefaultCupertinoTypography.headline,
    labelMedium = DefaultCupertinoTypography.caption1,
    labelSmall = DefaultCupertinoTypography.caption2
)
