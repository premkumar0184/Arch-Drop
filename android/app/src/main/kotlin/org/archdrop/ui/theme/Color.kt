package org.archdrop.ui.theme

import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color

// ============================================================================
// ARCH DROP APPLE HIG DESIGN SYSTEM - COLOR TOKENS
// ============================================================================

// --- Primary Accent: Arch Electric Blue ---
// Dark mode electric blue tuned for high vibrancy on OLED true black (#000000)
val ElectricBlueDark = Color(0xFF0A84FF)
// Light mode system blue aligned with standard Apple HIG
val ElectricBlueLight = Color(0xFF007AFF)

// --- Standard Apple System Tints (Dark Mode) ---
val SystemGreenDark = Color(0xFF30D158)
val SystemRedDark = Color(0xFFFF453A)
val SystemOrangeDark = Color(0xFFFF9F0A)
val SystemYellowDark = Color(0xFFFFD60A)
val SystemIndigoDark = Color(0xFF5E5CE6)
val SystemPurpleDark = Color(0xFFBF5AF2)
val SystemTealDark = Color(0xFF64D2FF)
val SystemCyanDark = Color(0xFF70D7FF)

// --- Standard Apple System Tints (Light Mode) ---
val SystemGreenLight = Color(0xFF34C759)
val SystemRedLight = Color(0xFFFF3B30)
val SystemOrangeLight = Color(0xFFFF9500)
val SystemYellowLight = Color(0xFFFFCC00)
val SystemIndigoLight = Color(0xFF5856D6)
val SystemPurpleLight = Color(0xFFAF52DE)
val SystemTealLight = Color(0xFF32ADE6)
val SystemCyanLight = Color(0xFF55BEF0)

// --- iOS System Gray Scale (Dark Mode) ---
val SystemGrayDark = Color(0xFF8E8E93)
val SystemGray2Dark = Color(0xFF636366)
val SystemGray3Dark = Color(0xFF48484A)
val SystemGray4Dark = Color(0xFF3A3A3C)
val SystemGray5Dark = Color(0xFF2C2C2E)
val SystemGray6Dark = Color(0xFF1C1C1E)

// --- iOS System Gray Scale (Light Mode) ---
val SystemGrayLight = Color(0xFF8E8E93)
val SystemGray2Light = Color(0xFFAEAEB2)
val SystemGray3Light = Color(0xFFC7C7CC)
val SystemGray4Light = Color(0xFFD1D1D6)
val SystemGray5Light = Color(0xFFE5E5EA)
val SystemGray6Light = Color(0xFFF2F2F7)

// --- Semantic Color Tokens Interface ---
@Immutable
data class CupertinoColors(
    // Canvas & Grouped Backgrounds
    val systemBackground: Color,
    val secondarySystemBackground: Color,
    val tertiarySystemBackground: Color,
    val systemGroupedBackground: Color,
    val secondarySystemGroupedBackground: Color,
    val tertiarySystemGroupedBackground: Color,

    // Foreground Text & Labels
    val label: Color,
    val secondaryLabel: Color,
    val tertiaryLabel: Color,
    val quaternaryLabel: Color,
    val placeholderText: Color,

    // Accents & System Tints
    val primaryAccent: Color,
    val systemGreen: Color,
    val systemRed: Color,
    val systemOrange: Color,
    val systemYellow: Color,
    val systemIndigo: Color,
    val systemPurple: Color,
    val systemTeal: Color,
    val systemCyan: Color,

    // iOS System Grays
    val systemGray: Color,
    val systemGray2: Color,
    val systemGray3: Color,
    val systemGray4: Color,
    val systemGray5: Color,
    val systemGray6: Color,

    // Separators & Hairlines
    val separator: Color,
    val opaqueSeparator: Color,
    val hairlineBorder: Color,

    // Fills (Interactive controls, chips, segmented backgrounds)
    val systemFill: Color,
    val secondarySystemFill: Color,
    val tertiarySystemFill: Color,
    val quaternarySystemFill: Color,

    // Glass / Translucent Scrims
    val glassBarBackground: Color,
    val glassPillBackground: Color,

    val isDark: Boolean
) {
    // Convenience aliases matching PROJECT.md interface contract
    val canvasBackground: Color get() = systemBackground
    val groupedCard: Color get() = secondarySystemGroupedBackground
    val tertiaryContainer: Color get() = tertiarySystemGroupedBackground
    val accent: Color get() = primaryAccent
    val systemBlue: Color get() = primaryAccent
    val labelPrimary: Color get() = label
    val labelSecondary: Color get() = secondaryLabel
    val labelTertiary: Color get() = tertiaryLabel
    val labelQuaternary: Color get() = quaternaryLabel
}

// --- Dark Mode Palette (Default for Arch Drop) ---
val DarkCupertinoColors = CupertinoColors(
    systemBackground = Color(0xFF000000),                 // OLED True Black Canvas
    secondarySystemBackground = Color(0xFF1C1C1E),        // Elevated Dark Surface
    tertiarySystemBackground = Color(0xFF2C2C2E),         // Nested Dark Surface
    systemGroupedBackground = Color(0xFF000000),          // OLED Grouped Canvas
    secondarySystemGroupedBackground = Color(0xFF1C1C1E), // Inset Grouped Cards / Cells
    tertiarySystemGroupedBackground = Color(0xFF2C2C2E),  // Nested Grouped Containers

    label = Color(0xFFFFFFFF),                            // 100% White (21:1 contrast)
    secondaryLabel = Color(0x99EBEBF5),                   // 60% White (#EBEBF5) (7.4:1 contrast)
    tertiaryLabel = Color(0x4DEBEBF5),                    // 30% White (#EBEBF5) (4.5:1 contrast)
    quaternaryLabel = Color(0x2EEBEBF5),                  // 18% White (Watermarks / disabled)
    placeholderText = Color(0x4DEBEBF5),

    primaryAccent = ElectricBlueDark,                     // #0A84FF
    systemGreen = SystemGreenDark,                        // #30D158
    systemRed = SystemRedDark,                            // #FF453A
    systemOrange = SystemOrangeDark,                      // #FF9F0A
    systemYellow = SystemYellowDark,                      // #FFD60A
    systemIndigo = SystemIndigoDark,                      // #5E5CE6
    systemPurple = SystemPurpleDark,                      // #BF5AF2
    systemTeal = SystemTealDark,                          // #64D2FF
    systemCyan = SystemCyanDark,                          // #70D7FF

    systemGray = SystemGrayDark,                          // #8E8E93
    systemGray2 = SystemGray2Dark,                        // #636366
    systemGray3 = SystemGray3Dark,                        // #48484A
    systemGray4 = SystemGray4Dark,                        // #3A3A3C
    systemGray5 = SystemGray5Dark,                        // #2C2C2E
    systemGray6 = SystemGray6Dark,                        // #1C1C1E

    separator = Color(0x59545458),                        // rgba(84, 84, 88, 0.35)
    opaqueSeparator = Color(0xFF38383A),
    hairlineBorder = Color(0x1AFFFFFF),                   // rgba(255, 255, 255, 0.10) - 0.5dp HIG border

    systemFill = Color(0x33787880),                       // 20% Fill
    secondarySystemFill = Color(0x29787880),              // 16% Fill
    tertiarySystemFill = Color(0x1F767680),               // 12% Fill
    quaternarySystemFill = Color(0x14747480),              // 8% Fill

    glassBarBackground = Color(0xCC1C1C1E),               // 80% Frosted Tab / Nav Bar Scrim
    glassPillBackground = Color(0x66000000),              // 40% Floating Glass Pill Scrim

    isDark = true
)

// --- Light Mode Palette ---
val LightCupertinoColors = CupertinoColors(
    systemBackground = Color(0xFFFFFFFF),
    secondarySystemBackground = Color(0xFFF2F2F7),
    tertiarySystemBackground = Color(0xFFFFFFFF),
    systemGroupedBackground = Color(0xFFF2F2F7),          // iOS Inset Grouped Light Canvas
    secondarySystemGroupedBackground = Color(0xFFFFFFFF), // Inset Grouped White Cards
    tertiarySystemGroupedBackground = Color(0xFFF2F2F7),

    label = Color(0xFF000000),                            // 100% Black
    secondaryLabel = Color(0x993C3C43),                   // 60% rgba(60, 60, 67, 0.60)
    tertiaryLabel = Color(0x4D3C3C43),                    // 30% rgba(60, 60, 67, 0.30)
    quaternaryLabel = Color(0x2E3C3C43),                  // 18% rgba(60, 60, 67, 0.18)
    placeholderText = Color(0x4D3C3C43),

    primaryAccent = ElectricBlueLight,                    // #007AFF
    systemGreen = SystemGreenLight,                       // #34C759
    systemRed = SystemRedLight,                           // #FF3B30
    systemOrange = SystemOrangeLight,                     // #FF9500
    systemYellow = SystemYellowLight,                     // #FFCC00
    systemIndigo = SystemIndigoLight,                     // #5856D6
    systemPurple = SystemPurpleLight,                     // #AF52DE
    systemTeal = SystemTealLight,                         // #32ADE6
    systemCyan = SystemCyanLight,                         // #55BEF0

    systemGray = SystemGrayLight,                         // #8E8E93
    systemGray2 = SystemGray2Light,                       // #AEAEB2
    systemGray3 = SystemGray3Light,                       // #C7C7CC
    systemGray4 = SystemGray4Light,                       // #D1D1D6
    systemGray5 = SystemGray5Light,                       // #E5E5EA
    systemGray6 = SystemGray6Light,                       // #F2F2F7

    separator = Color(0x333C3C43),                        // rgba(60, 60, 67, 0.20)
    opaqueSeparator = Color(0xFFC6C6C8),
    hairlineBorder = Color(0x26000000),                   // rgba(0, 0, 0, 0.15)

    systemFill = Color(0x33787880),
    secondarySystemFill = Color(0x29787880),
    tertiarySystemFill = Color(0x1F767680),
    quaternarySystemFill = Color(0x14747480),

    glassBarBackground = Color(0xCCF2F2F7),
    glassPillBackground = Color(0x66FFFFFF),

    isDark = false
)

// ============================================================================
// BACKWARD COMPATIBILITY TOKENS
// ============================================================================
// Preserved so existing callers (AudioScreen.kt, etc.) compile cleanly in M1
val ArchCyan = ElectricBlueDark
val ArchCyanLight = ElectricBlueLight
val ArchCyanDark = Color(0xFF0062CC)
val ArchSlate = SystemGray4Dark

val DarkBackground = Color(0xFF000000)
val DarkSurface = Color(0xFF1C1C1E)
val DarkSurfaceVariant = Color(0xFF2C2C2E)
val DarkPrimary = ElectricBlueDark
val DarkOnPrimary = Color.White
val DarkPrimaryContainer = Color(0xFF003366)
val DarkOnPrimaryContainer = Color(0xFFBCE9FF)
val DarkSecondary = SystemGray2Dark
val DarkOnSecondary = Color.White
val DarkTertiary = SystemGreenDark
val DarkError = SystemRedDark

val LightBackground = Color(0xFFF2F2F7)
val LightSurface = Color(0xFFFFFFFF)
val LightSurfaceVariant = Color(0xFFE5E5EA)
val LightPrimary = ElectricBlueLight
val LightOnPrimary = Color.White
val LightPrimaryContainer = Color(0xFFD6E4FF)
val LightOnPrimaryContainer = Color(0xFF001D40)
val LightSecondary = SystemGray2Light
val LightTertiary = SystemGreenLight
val LightError = SystemRedLight
