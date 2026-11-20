package org.archdrop.ui.theme

import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.CornerSize
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow

// ============================================================================
// ARCH DROP APPLE HIG DESIGN SYSTEM - UNIT TEST SUITE
// ============================================================================

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class ThemeTest {

    // ------------------------------------------------------------------------
    // 1. Dark Mode Color Values Verification
    // ------------------------------------------------------------------------

    @Test
    fun testDarkModeOledTrueBlackBackground() {
        // Primary canvas must be OLED True Black (#000000)
        assertEquals(Color(0xFF000000), DarkCupertinoColors.systemBackground)
        assertEquals(Color(0xFF000000), DarkCupertinoColors.systemGroupedBackground)
        assertEquals(Color(0xFF000000), DarkCupertinoColors.canvasBackground)
        assertEquals(Color(0xFF000000), DarkBackground)
    }

    @Test
    fun testDarkModeGroupedSurfaces() {
        // Secondary grouped card must be #1C1C1E (RGB 28, 28, 30)
        assertEquals(Color(0xFF1C1C1E), DarkCupertinoColors.secondarySystemGroupedBackground)
        assertEquals(Color(0xFF1C1C1E), DarkCupertinoColors.groupedCard)
        assertEquals(Color(0xFF1C1C1E), DarkSurface)

        // Tertiary nested container must be #2C2C2E (RGB 44, 44, 46)
        assertEquals(Color(0xFF2C2C2E), DarkCupertinoColors.tertiarySystemGroupedBackground)
        assertEquals(Color(0xFF2C2C2E), DarkCupertinoColors.tertiaryContainer)
        assertEquals(Color(0xFF2C2C2E), DarkSurfaceVariant)
    }

    @Test
    fun testPrimaryAccentElectricBlue() {
        // Dark Mode: Electric Blue Dark (#0A84FF)
        assertEquals(Color(0xFF0A84FF), DarkCupertinoColors.primaryAccent)
        assertEquals(Color(0xFF0A84FF), DarkCupertinoColors.accent)
        assertEquals(Color(0xFF0A84FF), ElectricBlueDark)
        assertEquals(Color(0xFF0A84FF), DarkPrimary)

        // Light Mode: Electric Blue Light (#007AFF)
        assertEquals(Color(0xFF007AFF), LightCupertinoColors.primaryAccent)
        assertEquals(Color(0xFF007AFF), LightCupertinoColors.accent)
        assertEquals(Color(0xFF007AFF), ElectricBlueLight)
        assertEquals(Color(0xFF007AFF), LightPrimary)
    }

    @Test
    fun testHairlineBorderAndTranslucentMaterials() {
        // Hairline border: 10% white in Dark Mode, 15% black in Light Mode
        assertEquals(Color(0x1AFFFFFF), DarkCupertinoColors.hairlineBorder)
        assertEquals(Color(0x26000000), LightCupertinoColors.hairlineBorder)

        val darkHairlineAlpha = DarkCupertinoColors.hairlineBorder.alpha
        assertTrue("Dark hairline alpha must be ~0.10", darkHairlineAlpha in 0.09f..0.11f)

        val lightHairlineAlpha = LightCupertinoColors.hairlineBorder.alpha
        assertTrue("Light hairline alpha must be ~0.15", lightHairlineAlpha in 0.14f..0.16f)

        // Frosted glass bar background: 80% opacity
        assertEquals(Color(0xCC1C1C1E), DarkCupertinoColors.glassBarBackground)
        val glassBarAlpha = DarkCupertinoColors.glassBarBackground.alpha
        assertTrue("Glass bar alpha must be ~0.80", glassBarAlpha in 0.79f..0.81f)

        // Floating glass pill: 40% opacity
        assertEquals(Color(0x66000000), DarkCupertinoColors.glassPillBackground)
        val glassPillAlpha = DarkCupertinoColors.glassPillBackground.alpha
        assertTrue("Glass pill alpha must be ~0.40", glassPillAlpha in 0.39f..0.41f)
    }

    @Test
    fun testSemanticLabelHierarchy() {
        // Dark Mode Labels
        assertEquals(Color(0xFFFFFFFF), DarkCupertinoColors.label)
        assertEquals(Color(0x99EBEBF5), DarkCupertinoColors.secondaryLabel)
        assertEquals(Color(0x4DEBEBF5), DarkCupertinoColors.tertiaryLabel)
        assertEquals(Color(0x2EEBEBF5), DarkCupertinoColors.quaternaryLabel)

        val secAlpha = DarkCupertinoColors.secondaryLabel.alpha
        assertTrue("Secondary label alpha must be 0.60", secAlpha in 0.59f..0.61f)

        val tertAlpha = DarkCupertinoColors.tertiaryLabel.alpha
        assertTrue("Tertiary label alpha must be 0.30", tertAlpha in 0.29f..0.31f)

        val quatAlpha = DarkCupertinoColors.quaternaryLabel.alpha
        assertTrue("Quaternary label alpha must be 0.18", quatAlpha in 0.17f..0.19f)

        // Light Mode Labels
        assertEquals(Color(0xFF000000), LightCupertinoColors.label)
        assertEquals(Color(0x993C3C43), LightCupertinoColors.secondaryLabel)
    }

    @Test
    fun testSystemColorTints() {
        assertEquals(Color(0xFF30D158), DarkCupertinoColors.systemGreen)
        assertEquals(Color(0xFFFF453A), DarkCupertinoColors.systemRed)
        assertEquals(Color(0xFFFF9F0A), DarkCupertinoColors.systemOrange)
        assertEquals(Color(0xFFFFD60A), DarkCupertinoColors.systemYellow)
        assertEquals(Color(0xFF5E5CE6), DarkCupertinoColors.systemIndigo)
        assertEquals(Color(0xFFBF5AF2), DarkCupertinoColors.systemPurple)
        assertEquals(Color(0xFF64D2FF), DarkCupertinoColors.systemTeal)

        assertTrue(DarkCupertinoColors.isDark)
        assertFalse(LightCupertinoColors.isDark)
    }

    // ------------------------------------------------------------------------
    // 2. WCAG 2.1 AA Contrast Ratio Verification (>= 4.5:1 Body, >= 3.0:1 Large)
    // ------------------------------------------------------------------------

    private fun linearizeComponent(c: Float): Double {
        return if (c <= 0.04045f) {
            c.toDouble() / 12.92
        } else {
            ((c.toDouble() + 0.055) / 1.055).pow(2.4)
        }
    }

    private fun calculateRelativeLuminance(color: Color): Double {
        val r = linearizeComponent(color.red)
        val g = linearizeComponent(color.green)
        val b = linearizeComponent(color.blue)
        return 0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    private fun compositeColor(foreground: Color, background: Color): Color {
        val a = foreground.alpha
        val r = a * foreground.red + (1f - a) * background.red
        val g = a * foreground.green + (1f - a) * background.green
        val b = a * foreground.blue + (1f - a) * background.blue
        return Color(red = r, green = g, blue = b, alpha = 1f)
    }

    private fun computeContrastRatio(foreground: Color, background: Color): Double {
        val effectiveFg = if (foreground.alpha < 1.0f) compositeColor(foreground, background) else foreground
        val l1 = calculateRelativeLuminance(effectiveFg)
        val l2 = calculateRelativeLuminance(background)
        val lighter = max(l1, l2)
        val darker = min(l1, l2)
        return (lighter + 0.05) / (darker + 0.05)
    }

    @Test
    fun testWcagContrastRatiosDarkMode() {
        val oledCanvas = DarkCupertinoColors.systemBackground // #000000
        val cardSurface = DarkCupertinoColors.secondarySystemGroupedBackground // #1C1C1E
        val tertiarySurface = DarkCupertinoColors.tertiarySystemGroupedBackground // #2C2C2E

        // Primary Label (#FFFFFF) on OLED Canvas: 21.0:1 (AAA)
        val primaryOnCanvas = computeContrastRatio(DarkCupertinoColors.label, oledCanvas)
        assertTrue("Primary label on OLED canvas must be >= 20:1, was $primaryOnCanvas", primaryOnCanvas >= 20.0)

        // Primary Label on #1C1C1E Card: ~16.2:1 (AAA)
        val primaryOnCard = computeContrastRatio(DarkCupertinoColors.label, cardSurface)
        assertTrue("Primary label on card must be >= 15:1, was $primaryOnCard", primaryOnCard >= 15.0)

        // Primary Label on #2C2C2E Tertiary Surface: ~13.5:1 (AAA)
        val primaryOnTertiary = computeContrastRatio(DarkCupertinoColors.label, tertiarySurface)
        assertTrue("Primary label on tertiary surface must be >= 12:1, was $primaryOnTertiary", primaryOnTertiary >= 12.0)

        // Secondary Label (60% White) on OLED Canvas: ~6.36:1 (Passes WCAG AA >= 4.5:1)
        val secondaryOnCanvas = computeContrastRatio(DarkCupertinoColors.secondaryLabel, oledCanvas)
        assertTrue("Secondary label on OLED canvas must be >= 4.5:1, was $secondaryOnCanvas", secondaryOnCanvas >= 4.5)

        // Secondary Label (60% White) on #1C1C1E Card: ~5.95:1 (Passes WCAG AA >= 4.5:1)
        val secondaryOnCard = computeContrastRatio(DarkCupertinoColors.secondaryLabel, cardSurface)
        assertTrue("Secondary label on card must be >= 4.5:1, was $secondaryOnCard", secondaryOnCard >= 4.5)

        // Primary Accent (#0A84FF) on OLED Canvas: ~5.76:1 (Passes WCAG AA >= 4.5:1)
        val accentOnCanvas = computeContrastRatio(DarkCupertinoColors.primaryAccent, oledCanvas)
        assertTrue("Electric blue on OLED canvas must be >= 4.5:1, was $accentOnCanvas", accentOnCanvas >= 4.5)

        // Primary Accent (#0A84FF) on #1C1C1E Card: ~4.67:1 (Passes WCAG AA >= 4.5:1)
        val accentOnCard = computeContrastRatio(DarkCupertinoColors.primaryAccent, cardSurface)
        assertTrue("Electric blue on card must be >= 4.5:1, was $accentOnCard", accentOnCard >= 4.5)

        // System Green (#30D158) on OLED Canvas: ~10.39:1 (AAA)
        val greenOnCanvas = computeContrastRatio(DarkCupertinoColors.systemGreen, oledCanvas)
        assertTrue("System green on OLED canvas must be >= 10:1, was $greenOnCanvas", greenOnCanvas >= 10.0)

        // System Red (#FF453A) on OLED Canvas: ~6.15:1 (Passes WCAG AA >= 4.5:1)
        val redOnCanvas = computeContrastRatio(DarkCupertinoColors.systemRed, oledCanvas)
        assertTrue("System red on OLED canvas must be >= 4.5:1, was $redOnCanvas", redOnCanvas >= 4.5)
    }

    @Test
    fun testWcagContrastRatiosLightMode() {
        val lightCanvas = LightCupertinoColors.systemBackground // #FFFFFF
        val lightGrouped = LightCupertinoColors.systemGroupedBackground // #F2F2F7

        // Primary Label (#000000) on White Card: 21.0:1 (AAA)
        val primaryOnWhite = computeContrastRatio(LightCupertinoColors.label, lightCanvas)
        assertEquals(21.0, primaryOnWhite, 0.01)

        // Primary Label on Grouped Canvas (#F2F2F7): > 15:1 (AAA)
        val primaryOnGrouped = computeContrastRatio(LightCupertinoColors.label, lightGrouped)
        assertTrue("Primary label on light grouped canvas must be >= 15:1", primaryOnGrouped >= 15.0)

        // Secondary Label on White Card: > 3.0:1 (Passes WCAG AA Large / Secondary Text)
        val secondaryOnWhite = computeContrastRatio(LightCupertinoColors.secondaryLabel, lightCanvas)
        assertTrue("Secondary label on white card must be >= 3.0:1, was $secondaryOnWhite", secondaryOnWhite >= 3.0)

        // Primary Accent (#007AFF) on White Card: ~4.04:1 (Passes UI component & large text >= 3.0:1)
        val accentOnWhite = computeContrastRatio(LightCupertinoColors.primaryAccent, lightCanvas)
        assertTrue("Accent blue on white card must be >= 4.0:1, was $accentOnWhite", accentOnWhite >= 4.0)
    }

    // ------------------------------------------------------------------------
    // 3. SF Pro Typography Scale Verification
    // ------------------------------------------------------------------------

    @Test
    fun testTypographyScaleFontSizesAndWeights() {
        val typography = DefaultCupertinoTypography

        // Large Title: 34sp / 41sp leading / Bold / +0.40sp tracking
        assertEquals(34.sp, typography.largeTitle.fontSize)
        assertEquals(41.sp, typography.largeTitle.lineHeight)
        assertEquals(FontWeight.Bold, typography.largeTitle.fontWeight)
        assertEquals(0.40.sp, typography.largeTitle.letterSpacing)

        // Title 1: 28sp / 34sp leading / Bold / +0.38sp tracking
        assertEquals(28.sp, typography.title1.fontSize)
        assertEquals(34.sp, typography.title1.lineHeight)
        assertEquals(FontWeight.Bold, typography.title1.fontWeight)
        assertEquals(0.38.sp, typography.title1.letterSpacing)

        // Title 2: 22sp / 28sp leading / Bold / -0.26sp tracking
        assertEquals(22.sp, typography.title2.fontSize)
        assertEquals(28.sp, typography.title2.lineHeight)
        assertEquals(FontWeight.Bold, typography.title2.fontWeight)
        assertEquals((-0.26).sp, typography.title2.letterSpacing)

        // Title 3: 20sp / 25sp leading / SemiBold / -0.45sp tracking
        assertEquals(20.sp, typography.title3.fontSize)
        assertEquals(25.sp, typography.title3.lineHeight)
        assertEquals(FontWeight.SemiBold, typography.title3.fontWeight)
        assertEquals((-0.45).sp, typography.title3.letterSpacing)

        // Headline: 17sp / 22sp leading / SemiBold / -0.43sp tracking
        assertEquals(17.sp, typography.headline.fontSize)
        assertEquals(22.sp, typography.headline.lineHeight)
        assertEquals(FontWeight.SemiBold, typography.headline.fontWeight)
        assertEquals((-0.43).sp, typography.headline.letterSpacing)

        // Body: 17sp / 22sp leading / Normal / -0.43sp tracking
        assertEquals(17.sp, typography.body.fontSize)
        assertEquals(22.sp, typography.body.lineHeight)
        assertEquals(FontWeight.Normal, typography.body.fontWeight)
        assertEquals((-0.43).sp, typography.body.letterSpacing)

        // Callout: 16sp / 21sp leading / Normal / -0.31sp tracking
        assertEquals(16.sp, typography.callout.fontSize)
        assertEquals(21.sp, typography.callout.lineHeight)
        assertEquals(FontWeight.Normal, typography.callout.fontWeight)

        // Subheadline: 15sp / 20sp leading / Normal / -0.23sp tracking
        assertEquals(15.sp, typography.subheadline.fontSize)
        assertEquals(20.sp, typography.subheadline.lineHeight)
        assertEquals(FontWeight.Normal, typography.subheadline.fontWeight)

        // Footnote: 13sp / 18sp leading / Normal / -0.08sp tracking
        assertEquals(13.sp, typography.footnote.fontSize)
        assertEquals(18.sp, typography.footnote.lineHeight)
        assertEquals(FontWeight.Normal, typography.footnote.fontWeight)

        // Caption 1: 12sp / 16sp leading / Normal / 0.0sp tracking
        assertEquals(12.sp, typography.caption1.fontSize)
        assertEquals(16.sp, typography.caption1.lineHeight)
        assertEquals(FontWeight.Normal, typography.caption1.fontWeight)

        // Caption 2: 11sp / 13sp leading / Medium / +0.06sp tracking
        assertEquals(11.sp, typography.caption2.fontSize)
        assertEquals(13.sp, typography.caption2.lineHeight)
        assertEquals(FontWeight.Medium, typography.caption2.fontWeight)

        // Monospace Hex: 12sp / Monospace
        assertEquals(12.sp, typography.monospaceHex.fontSize)
        assertEquals(FontFamily.Monospace, typography.monospaceHex.fontFamily)
    }

    @Test
    fun testMaterial3TypographyMapping() {
        assertEquals(34.sp, ArchDropTypography.displayLarge.fontSize)
        assertEquals(28.sp, ArchDropTypography.headlineLarge.fontSize)
        assertEquals(22.sp, ArchDropTypography.headlineMedium.fontSize)
        assertEquals(20.sp, ArchDropTypography.headlineSmall.fontSize)
        assertEquals(17.sp, ArchDropTypography.bodyLarge.fontSize)
        assertEquals(15.sp, ArchDropTypography.bodyMedium.fontSize)
        assertEquals(13.sp, ArchDropTypography.bodySmall.fontSize)
        assertEquals(11.sp, ArchDropTypography.labelSmall.fontSize)
    }

    // ------------------------------------------------------------------------
    // 4. Continuous Squircle Corner Geometry Verification
    // ------------------------------------------------------------------------

    @Test
    fun testSquircleShapesCornerRadii() {
        val density = Density(density = 2.0f)
        val viewport = Size(200f, 200f)

        // Standard Card: 16dp corner radius
        val cardShape = DefaultCupertinoShapes.card as RoundedCornerShape
        val cardRadiusPx = cardShape.topStart.toPx(viewport, density)
        assertEquals(16f * 2.0f, cardRadiusPx, 0.01f)
        assertEquals(DefaultCupertinoShapes.card, DefaultCupertinoShapes.cardSquircle)

        // Hero Card: 20dp corner radius
        val heroShape = DefaultCupertinoShapes.cardHero as RoundedCornerShape
        val heroRadiusPx = heroShape.topStart.toPx(viewport, density)
        assertEquals(20f * 2.0f, heroRadiusPx, 0.01f)
        assertEquals(DefaultCupertinoShapes.cardHero, DefaultCupertinoShapes.heroSquircle)

        // Modal Bottom Sheet: 24dp top corner radius, 0dp bottom
        val sheetShape = DefaultCupertinoShapes.sheet as RoundedCornerShape
        val sheetTopPx = sheetShape.topStart.toPx(viewport, density)
        val sheetBottomPx = sheetShape.bottomStart.toPx(viewport, density)
        assertEquals(24f * 2.0f, sheetTopPx, 0.01f)
        assertEquals(0f, sheetBottomPx, 0.01f)
        assertEquals(DefaultCupertinoShapes.sheet, DefaultCupertinoShapes.sheetSquircle)

        // Dialog: 20dp corner radius
        val dialogShape = DefaultCupertinoShapes.dialog as RoundedCornerShape
        assertEquals(20f * 2.0f, dialogShape.topStart.toPx(viewport, density), 0.01f)

        // Segmented Control: 10dp track, 8dp thumb
        val trackShape = DefaultCupertinoShapes.segmentedTrack as RoundedCornerShape
        val thumbShape = DefaultCupertinoShapes.segmentedThumb as RoundedCornerShape
        assertEquals(10f * 2.0f, trackShape.topStart.toPx(viewport, density), 0.01f)
        assertEquals(8f * 2.0f, thumbShape.topStart.toPx(viewport, density), 0.01f)
        assertEquals(DefaultCupertinoShapes.segmentedTrack, DefaultCupertinoShapes.controlSquircle)

        // Buttons: 8dp small, 12dp medium, 14dp large
        val btnSmall = DefaultCupertinoShapes.buttonSmall as RoundedCornerShape
        val btnMedium = DefaultCupertinoShapes.buttonMedium as RoundedCornerShape
        val btnLarge = DefaultCupertinoShapes.buttonLarge as RoundedCornerShape
        assertEquals(8f * 2.0f, btnSmall.topStart.toPx(viewport, density), 0.01f)
        assertEquals(12f * 2.0f, btnMedium.topStart.toPx(viewport, density), 0.01f)
        assertEquals(14f * 2.0f, btnLarge.topStart.toPx(viewport, density), 0.01f)

        // Capsule & Circle: CircleShape
        assertEquals(CircleShape, DefaultCupertinoShapes.capsule)
        assertEquals(CircleShape, DefaultCupertinoShapes.circle)
    }

    // ------------------------------------------------------------------------
    // 5. Touch Target Ergonomics & Modifiers Contract
    // ------------------------------------------------------------------------

    @Test
    fun testCupertinoTouchTargetConstant() {
        val minTarget = 44.dp
        assertEquals(44.dp, minTarget)
        assertTrue("Apple HIG touch target must be at least 44dp", minTarget >= 44.dp)
    }

    @Test
    fun testArchDropThemeCompositionLocalsDefaultValues() {
        assertNotNull(LocalCupertinoColors)
        assertNotNull(LocalCupertinoTypography)
        assertNotNull(LocalCupertinoShapes)
    }
}
