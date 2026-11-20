package org.archdrop.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color

// ============================================================================
// ARCH DROP APPLE HIG DESIGN SYSTEM - THEME PROVIDER & COMPOSITION LOCALS
// ============================================================================

val LocalCupertinoColors = staticCompositionLocalOf { DarkCupertinoColors }
val LocalCupertinoTypography = staticCompositionLocalOf { DefaultCupertinoTypography }
val LocalCupertinoShapes = staticCompositionLocalOf { DefaultCupertinoShapes }

object ArchDropTheme {
    val colors: CupertinoColors
        @Composable
        @ReadOnlyComposable
        get() = LocalCupertinoColors.current

    val typography: CupertinoTypography
        @Composable
        @ReadOnlyComposable
        get() = LocalCupertinoTypography.current

    val shapes: CupertinoShapes
        @Composable
        @ReadOnlyComposable
        get() = LocalCupertinoShapes.current
}

// Map Cupertino tokens into Material 3 Dark Color Scheme
private val M3DarkColorScheme = darkColorScheme(
    primary = ElectricBlueDark,
    onPrimary = Color.White,
    primaryContainer = Color(0xFF003366),
    onPrimaryContainer = Color(0xFFBCE9FF),
    secondary = SystemGray2Dark,
    onSecondary = Color.White,
    secondaryContainer = SystemGray5Dark,
    onSecondaryContainer = Color.White,
    tertiary = SystemGreenDark,
    onTertiary = Color.Black,
    tertiaryContainer = Color(0xFF0E3D1D),
    onTertiaryContainer = Color(0xFFA6F5BD),
    background = Color(0xFF000000),             // OLED True Black Canvas
    onBackground = Color.White,
    surface = Color(0xFF1C1C1E),                // Secondary Grouped Card Surface (#1C1C1E)
    onSurface = Color.White,
    surfaceVariant = Color(0xFF2C2C2E),         // Tertiary Grouped Container (#2C2C2E)
    onSurfaceVariant = Color(0x99EBEBF5),       // Secondary Label (60% White)
    outline = Color(0x59545458),                // Separator
    outlineVariant = Color(0x1AFFFFFF),         // Hairline Border
    error = SystemRedDark,
    onError = Color.White,
    errorContainer = Color(0xFF4D1412),
    onErrorContainer = Color(0xFFFFD4D2)
)

// Map Cupertino tokens into Material 3 Light Color Scheme
private val M3LightColorScheme = lightColorScheme(
    primary = ElectricBlueLight,
    onPrimary = Color.White,
    primaryContainer = Color(0xFFD6E4FF),
    onPrimaryContainer = Color(0xFF001D40),
    secondary = SystemGray2Light,
    onSecondary = Color.Black,
    secondaryContainer = SystemGray5Light,
    onSecondaryContainer = Color.Black,
    tertiary = SystemGreenLight,
    onTertiary = Color.White,
    background = Color(0xFFF2F2F7),             // iOS Grouped Background
    onBackground = Color.Black,
    surface = Color(0xFFFFFFFF),                // Inset Grouped Card Surface
    onSurface = Color.Black,
    surfaceVariant = Color(0xFFE5E5EA),
    onSurfaceVariant = Color(0x993C3C43),
    outline = Color(0x333C3C43),
    outlineVariant = Color(0x26000000),
    error = SystemRedLight,
    onError = Color.White
)

@Composable
fun ArchDropTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    // Strictly defaulted to false so Android 12+ wallpaper tinting never overrides Apple HIG tokens
    dynamicColor: Boolean = false,
    content: @Composable () -> Unit
) {
    val cupertinoColors = if (darkTheme) DarkCupertinoColors else LightCupertinoColors
    val m3ColorScheme = if (darkTheme) M3DarkColorScheme else M3LightColorScheme

    CompositionLocalProvider(
        LocalCupertinoColors provides cupertinoColors,
        LocalCupertinoTypography provides DefaultCupertinoTypography,
        LocalCupertinoShapes provides DefaultCupertinoShapes
    ) {
        MaterialTheme(
            colorScheme = m3ColorScheme,
            typography = ArchDropTypography,
            shapes = ArchDropShapes,
            content = content
        )
    }
}
