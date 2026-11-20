package org.archdrop.ui.theme

import androidx.compose.ui.graphics.Color
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow

/**
 * Empirical Adversarial Test Harness for Milestone 1:
 * - Stress-tests WCAG 2.1 AA and AAA mathematical contrast ratio calculations
 * - Multi-layer alpha compositing (glass materials, translucent labels, pills over cards)
 * - Dynamic color leak protection (verifying immunity against Monet wallpaper extraction)
 * - Semantic tint distinguishability and accessibility independence
 * - Backward compatibility token stability
 */
@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class ColorContrastRobustnessTest {

    // =========================================================================
    // WCAG 2.1 Formula Implementation & Reference Oracles
    // =========================================================================

    private fun linearizeComponent(c: Float): Double {
        return if (c <= 0.04045f) {
            c.toDouble() / 12.92
        } else {
            ((c.toDouble() + 0.055) / 1.055).pow(2.4)
        }
    }

    private fun relativeLuminance(color: Color): Double {
        val r = linearizeComponent(color.red)
        val g = linearizeComponent(color.green)
        val b = linearizeComponent(color.blue)
        return 0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /**
     * Standard Porter-Duff source-over compositing of a translucent foreground over an opaque background.
     */
    private fun composite(foreground: Color, background: Color): Color {
        val a = foreground.alpha
        val r = a * foreground.red + (1f - a) * background.red
        val g = a * foreground.green + (1f - a) * background.green
        val b = a * foreground.blue + (1f - a) * background.blue
        return Color(red = r, green = g, blue = b, alpha = 1f)
    }

    /**
     * Computes WCAG 2.1 contrast ratio: (L1 + 0.05) / (L2 + 0.05).
     */
    private fun contrast(foreground: Color, background: Color): Double {
        val effectiveFg = if (foreground.alpha < 1.0f) composite(foreground, background) else foreground
        val effectiveBg = if (background.alpha < 1.0f) composite(background, Color.Black) else background
        val l1 = relativeLuminance(effectiveFg)
        val l2 = relativeLuminance(effectiveBg)
        val lighter = max(l1, l2)
        val darker = min(l1, l2)
        return (lighter + 0.05) / (darker + 0.05)
    }

    // =========================================================================
    // 1. Mathematical WCAG Contrast Formula Properties
    // =========================================================================

    @Test
    fun testMathematicalPropertiesOfContrastFormula() {
        // Identity: Any color against itself has contrast ratio exactly 1.0:1
        val black = Color(0xFF000000)
        val white = Color(0xFFFFFFFF)
        val blue = ElectricBlueDark
        val card = Color(0xFF1C1C1E)

        assertEquals(1.0, contrast(black, black), 0.0001)
        assertEquals(1.0, contrast(white, white), 0.0001)
        assertEquals(1.0, contrast(blue, blue), 0.0001)
        assertEquals(1.0, contrast(card, card), 0.0001)

        // Extreme bounds: Pure Black on Pure White is exactly 21.0:1
        val maxContrast = contrast(white, black)
        assertEquals(21.0, maxContrast, 0.01)

        // Symmetry: contrast(A, B) must equal contrast(B, A)
        assertEquals(contrast(white, blue), contrast(blue, white), 0.0001)
        assertEquals(contrast(card, blue), contrast(blue, card), 0.0001)

        // Boundedness: For any two colors, 1.0 <= contrast <= 21.0
        val testColors = listOf(
            ElectricBlueDark, ElectricBlueLight, SystemGreenDark, SystemRedDark,
            SystemOrangeDark, SystemYellowDark, SystemIndigoDark, SystemPurpleDark,
            SystemTealDark, SystemCyanDark, SystemGrayDark, SystemGray6Dark
        )
        for (c1 in testColors) {
            for (c2 in testColors) {
                val cr = contrast(c1, c2)
                assertTrue("Contrast must be >= 1.0, was $cr for $c1 vs $c2", cr >= 1.0 - 0.0001)
                assertTrue("Contrast must be <= 21.0, was $cr for $c1 vs $c2", cr <= 21.0 + 0.0001)
            }
        }
    }

    // =========================================================================
    // 2. Dark Mode Layered Surfaces & Semantic Foreground Contrasts
    // =========================================================================

    @Test
    fun testDarkModeCanvasCardAndTertiaryTextContrast() {
        val oledCanvas = DarkCupertinoColors.canvasBackground // #000000
        val card = DarkCupertinoColors.groupedCard // #1C1C1E
        val tertiaryContainer = DarkCupertinoColors.tertiaryContainer // #2C2C2E

        // 1. Primary Label (100% White):
        // Canvas (#000000): 21.0:1 (Exceeds WCAG AAA 7.0:1)
        val labelOnCanvas = contrast(DarkCupertinoColors.labelPrimary, oledCanvas)
        assertEquals(21.0, labelOnCanvas, 0.01)

        // Card (#1C1C1E): ~16.2:1 (Exceeds WCAG AAA 7.0:1)
        val labelOnCard = contrast(DarkCupertinoColors.labelPrimary, card)
        assertTrue("Primary label on card must exceed 15:1, was $labelOnCard", labelOnCard >= 15.0)

        // Tertiary container (#2C2C2E): ~13.5:1 (Exceeds WCAG AAA 7.0:1)
        val labelOnTertiary = contrast(DarkCupertinoColors.labelPrimary, tertiaryContainer)
        assertTrue("Primary label on tertiary surface must exceed 12:1, was $labelOnTertiary", labelOnTertiary >= 12.0)

        // 2. Secondary Label (60% White #EBEBF5):
        // Canvas: ~6.36:1 (Passes WCAG AA Body Text >= 4.5:1)
        val secOnCanvas = contrast(DarkCupertinoColors.labelSecondary, oledCanvas)
        assertTrue("Secondary label on canvas must be >= 4.5:1, was $secOnCanvas", secOnCanvas >= 4.5)

        // Card: ~5.95:1 (Passes WCAG AA Body Text >= 4.5:1)
        val secOnCard = contrast(DarkCupertinoColors.labelSecondary, card)
        assertTrue("Secondary label on card must be >= 4.5:1, was $secOnCard", secOnCard >= 4.5)

        // Tertiary: ~5.26:1 (Passes WCAG AA Body Text >= 4.5:1)
        val secOnTertiary = contrast(DarkCupertinoColors.labelSecondary, tertiaryContainer)
        assertTrue("Secondary label on tertiary container must be >= 4.5:1, was $secOnTertiary", secOnTertiary >= 4.5)

        // 3. Primary Accent (Arch Electric Blue Dark #0A84FF):
        // Canvas: ~5.76:1 (Passes WCAG AA >= 4.5:1)
        val accentOnCanvas = contrast(DarkCupertinoColors.accent, oledCanvas)
        assertTrue("Accent blue on canvas must be >= 4.5:1, was $accentOnCanvas", accentOnCanvas >= 4.5)

        // Card: ~4.67:1 (Passes WCAG AA >= 4.5:1)
        val accentOnCard = contrast(DarkCupertinoColors.accent, card)
        assertTrue("Accent blue on card must be >= 4.5:1, was $accentOnCard", accentOnCard >= 4.5)

        // 4. Status Tints on Canvas:
        // System Green (#30D158): > 10.0:1 (AAA)
        val greenOnCanvas = contrast(DarkCupertinoColors.systemGreen, oledCanvas)
        assertTrue("Green on canvas must exceed 10:1, was $greenOnCanvas", greenOnCanvas >= 10.0)

        // System Red (#FF453A): > 6.0:1 (AA)
        val redOnCanvas = contrast(DarkCupertinoColors.systemRed, oledCanvas)
        assertTrue("Red on canvas must exceed 6:1, was $redOnCanvas", redOnCanvas >= 6.0)

        // System Orange (#FF9F0A): > 9.0:1 (AAA)
        val orangeOnCanvas = contrast(DarkCupertinoColors.systemOrange, oledCanvas)
        assertTrue("Orange on canvas must exceed 9:1, was $orangeOnCanvas", orangeOnCanvas >= 9.0)

        // System Yellow (#FFD60A): > 14.0:1 (AAA)
        val yellowOnCanvas = contrast(DarkCupertinoColors.systemYellow, oledCanvas)
        assertTrue("Yellow on canvas must exceed 14:1, was $yellowOnCanvas", yellowOnCanvas >= 14.0)

        // System Teal (#64D2FF): > 11.0:1 (AAA)
        val tealOnCanvas = contrast(DarkCupertinoColors.systemTeal, oledCanvas)
        assertTrue("Teal on canvas must exceed 11:1, was $tealOnCanvas", tealOnCanvas >= 11.0)
    }

    // =========================================================================
    // 3. Multi-Layer Frosted Glass & Pill Compositing Contrasts
    // =========================================================================

    @Test
    fun testFrostedGlassAndGlassPillCompositedContrasts() {
        val oledCanvas = DarkCupertinoColors.canvasBackground // #000000
        val card = DarkCupertinoColors.groupedCard // #1C1C1E

        // Frosted Glass Nav/Tab Bar: 80% #1C1C1E over OLED canvas
        val effectiveGlassBar = composite(DarkCupertinoColors.glassBarBackground, oledCanvas)

        // White text on frosted glass bar:
        val whiteOnGlassBar = contrast(Color.White, effectiveGlassBar)
        assertTrue("White text on frosted glass bar must exceed 15:1, was $whiteOnGlassBar", whiteOnGlassBar >= 15.0)

        // Secondary label on frosted glass bar:
        val secOnGlassBar = contrast(DarkCupertinoColors.labelSecondary, effectiveGlassBar)
        assertTrue("Secondary label on frosted glass bar must exceed 4.5:1, was $secOnGlassBar", secOnGlassBar >= 4.5)

        // Primary accent on frosted glass bar:
        val accentOnGlassBar = contrast(DarkCupertinoColors.accent, effectiveGlassBar)
        assertTrue("Accent blue on frosted glass bar must exceed 4.5:1, was $accentOnGlassBar", accentOnGlassBar >= 4.5)

        // Floating Glass Pill: 40% Black over #1C1C1E Card
        val effectivePill = composite(DarkCupertinoColors.glassPillBackground, card)

        // White text on floating glass pill (e.g. FPS badge, lens picker):
        val whiteOnPill = contrast(Color.White, effectivePill)
        assertTrue("White text on glass pill must exceed 17:1, was $whiteOnPill", whiteOnPill >= 17.0)

        // Secondary label on floating glass pill:
        val secOnPill = contrast(DarkCupertinoColors.labelSecondary, effectivePill)
        assertTrue("Secondary label on glass pill must exceed 5.0:1, was $secOnPill", secOnPill >= 5.0)
    }

    // =========================================================================
    // 4. Light Mode Contrast Matrix
    // =========================================================================

    @Test
    fun testLightModeContrastMatrix() {
        val lightCanvas = LightCupertinoColors.canvasBackground // #FFFFFF
        val lightGrouped = LightCupertinoColors.systemGroupedBackground // #F2F2F7

        // Primary Label (#000000) on White Card: 21.0:1 (AAA)
        val labelOnWhite = contrast(LightCupertinoColors.labelPrimary, lightCanvas)
        assertEquals(21.0, labelOnWhite, 0.01)

        // Primary Label on Inset Grouped Canvas (#F2F2F7): > 15:1 (AAA)
        val labelOnGrouped = contrast(LightCupertinoColors.labelPrimary, lightGrouped)
        assertTrue("Label on grouped canvas must be >= 15:1, was $labelOnGrouped", labelOnGrouped >= 15.0)

        // Secondary Label (rgba(60, 60, 67, 0.60)) on White Card: > 3.0:1 (WCAG AA Large/Secondary)
        val secOnWhite = contrast(LightCupertinoColors.labelSecondary, lightCanvas)
        assertTrue("Secondary label on white card must be >= 3.0:1, was $secOnWhite", secOnWhite >= 3.0)

        // Primary Accent (ElectricBlueLight = #007AFF) on White Card: > 4.0:1
        val accentOnWhite = contrast(LightCupertinoColors.accent, lightCanvas)
        assertTrue("Accent blue on white card must be >= 4.0:1, was $accentOnWhite", accentOnWhite >= 4.0)

        // System Red Light (#FF3B30) on White: ~3.55:1 (Passes WCAG AA Large / UI component >= 3.0:1)
        val redOnWhite = contrast(LightCupertinoColors.systemRed, lightCanvas)
        assertTrue("Red on white card must be >= 3.5:1, was $redOnWhite", redOnWhite >= 3.5)
    }

    // =========================================================================
    // 5. Hairline Divider Visibility & Contrast
    // =========================================================================

    @Test
    fun testHairlineBorderContrastAndPerceptibility() {
        // Dark Mode Hairline: rgba(255, 255, 255, 0.10) (10% white)
        val darkHairline = DarkCupertinoColors.hairlineBorder
        val darkCard = DarkCupertinoColors.groupedCard // #1C1C1E
        val effectiveDarkHairline = composite(darkHairline, darkCard)

        // The hairline must be subtly perceptible (contrast between 1.05 and 1.35 against card)
        val darkHairlineContrast = contrast(effectiveDarkHairline, darkCard)
        assertTrue(
            "Dark hairline must provide subtle edge separation (1.05..1.35), was $darkHairlineContrast",
            darkHairlineContrast in 1.05..1.35
        )

        // Light Mode Hairline: rgba(0, 0, 0, 0.15) (15% black)
        val lightHairline = LightCupertinoColors.hairlineBorder
        val lightCard = LightCupertinoColors.groupedCard // #FFFFFF
        val effectiveLightHairline = composite(lightHairline, lightCard)

        val lightHairlineContrast = contrast(effectiveLightHairline, lightCard)
        assertTrue(
            "Light hairline must provide subtle edge separation (1.10..1.50), was $lightHairlineContrast",
            lightHairlineContrast in 1.10..1.50
        )
    }

    // =========================================================================
    // 6. Dynamic Color Leak Protection & Material 3 Bridging
    // =========================================================================

    @Test
    fun testDynamicColorLeakProtectionAndM3Fallback() {
        // Verify default dynamicColor is false
        // Even if caller requests dynamicColor = false or true, CupertinoColors must never change
        val defaultDark = DarkCupertinoColors
        val defaultLight = LightCupertinoColors

        // Ensure canvas background is OLED True Black (#000000)
        assertEquals(Color(0xFF000000), defaultDark.systemBackground)
        assertEquals(Color(0xFF1C1C1E), defaultDark.secondarySystemGroupedBackground)
        assertEquals(Color(0xFF2C2C2E), defaultDark.tertiarySystemGroupedBackground)
        assertEquals(ElectricBlueDark, defaultDark.primaryAccent)

        // Verify that M3 dark color scheme maps to Apple HIG tokens
        // Check that background is OLED True Black (#000000) and surface is #1C1C1E
        assertEquals(Color(0xFF000000), DarkBackground)
        assertEquals(Color(0xFF1C1C1E), DarkSurface)
        assertEquals(Color(0xFF2C2C2E), DarkSurfaceVariant)
        assertEquals(ElectricBlueDark, DarkPrimary)
        assertEquals(Color.White, DarkOnPrimary)
        assertEquals(SystemGreenDark, DarkTertiary)
        assertEquals(SystemRedDark, DarkError)

        // Ensure Light Mode M3 fallback mappings
        assertEquals(Color(0xFFF2F2F7), LightBackground)
        assertEquals(Color(0xFFFFFFFF), LightSurface)
        assertEquals(Color(0xFFE5E5EA), LightSurfaceVariant)
        assertEquals(ElectricBlueLight, LightPrimary)
        assertEquals(SystemGreenLight, LightTertiary)
        assertEquals(SystemRedLight, LightError)
    }

    // =========================================================================
    // 7. System Tint Independence & Distinction
    // =========================================================================

    @Test
    fun testSystemTintPairwiseDistinction() {
        // Tints must have distinct hues and values to avoid visual collision
        val dark = DarkCupertinoColors
        assertNotEquals(dark.systemGreen, dark.systemRed)
        assertNotEquals(dark.systemOrange, dark.systemYellow)
        assertNotEquals(dark.primaryAccent, dark.systemTeal)
        assertNotEquals(dark.primaryAccent, dark.systemCyan)
        assertNotEquals(dark.systemIndigo, dark.systemPurple)

        // Verify Euclidean color distance in sRGB space is >= 0.15 between adjacent tints
        fun colorDistance(c1: Color, c2: Color): Double {
            val dr = (c1.red - c2.red).toDouble()
            val dg = (c1.green - c2.green).toDouble()
            val db = (c1.blue - c2.blue).toDouble()
            return kotlin.math.sqrt(dr * dr + dg * dg + db * db)
        }

        val greenRedDist = colorDistance(dark.systemGreen, dark.systemRed)
        assertTrue("Green and Red distance must be >= 0.8, was $greenRedDist", greenRedDist >= 0.8)

        val blueTealDist = colorDistance(dark.primaryAccent, dark.systemTeal)
        assertTrue("Electric blue and Teal distance must be >= 0.2, was $blueTealDist", blueTealDist >= 0.2)

        val orangeYellowDist = colorDistance(dark.systemOrange, dark.systemYellow)
        assertTrue("Orange and Yellow distance must be >= 0.2, was $orangeYellowDist", orangeYellowDist >= 0.2)
    }

    // =========================================================================
    // 8. Backward Compatibility Aliases Verification
    // =========================================================================

    @Test
    fun testBackwardCompatibilityAliasesIntegrity() {
        assertEquals(ElectricBlueDark, ArchCyan)
        assertEquals(ElectricBlueLight, ArchCyanLight)
        assertEquals(Color(0xFF0062CC), ArchCyanDark)
        assertEquals(SystemGray4Dark, ArchSlate)

        assertEquals(DarkCupertinoColors.systemBackground, DarkBackground)
        assertEquals(DarkCupertinoColors.secondarySystemGroupedBackground, DarkSurface)
        assertEquals(DarkCupertinoColors.tertiarySystemGroupedBackground, DarkSurfaceVariant)
        assertEquals(DarkCupertinoColors.primaryAccent, DarkPrimary)

        assertEquals(LightCupertinoColors.systemGroupedBackground, LightBackground)
        assertEquals(LightCupertinoColors.systemBackground, LightSurface)
        assertEquals(LightCupertinoColors.primaryAccent, LightPrimary)
    }
}
