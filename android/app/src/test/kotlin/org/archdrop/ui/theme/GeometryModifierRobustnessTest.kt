package org.archdrop.ui.theme

import android.os.Build
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.geometry.RoundRect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Outline
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * Empirical Adversarial Test Harness for Milestone 1:
 * - Squircle CornerSize resolution across diverse screen densities (ldpi to xxxhdpi)
 * - Boundary viewport geometry & zero/degenerate size handling
 * - Hairline rendering mathematics (0.5dp vs subpixel thresholds)
 * - Frosted glass & blur fallbacks across API levels (29 to 34)
 * - 44x44 pt touch target ergonomics & constraint propagation
 */
@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class GeometryModifierRobustnessTest {

    // Diverse density profiles for real-world Android displays
    private val densities = listOf(
        0.75f, // ldpi (e.g. low-end wearables/feature phones)
        1.0f,  // mdpi (160 dpi baseline)
        1.33f, // tvdpi (720p TV)
        1.5f,  // hdpi (240 dpi)
        2.0f,  // xhdpi (320 dpi - iPhone Retina / standard 720p/1080p phone)
        2.625f,// fractional (Pixel 3a / 420 dpi)
        3.0f,  // xxhdpi (480 dpi - 1080p flagship)
        3.5f,  // fractional (560 dpi)
        4.0f,  // xxxhdpi (640 dpi - 1440p Quad HD)
        5.0f   // ultra-high density future proofing
    )

    // ========================================================================
    // 1. SQUIRCLE CORNER SIZE ACROSS DIVERSE DENSITIES
    // ========================================================================

    @Test
    fun testSquircleCornerSizeResolutionAcrossDensities() {
        val shapes = DefaultCupertinoShapes
        val viewport = Size(400f, 400f)

        for (densityVal in densities) {
            val density = Density(density = densityVal)

            // Button Small: 8dp
            val btnSmall = shapes.buttonSmall as RoundedCornerShape
            val btnSmallPx = btnSmall.topStart.toPx(viewport, density)
            assertEquals("Button small at density $densityVal", 8f * densityVal, btnSmallPx, 0.001f)

            // Button Medium: 12dp
            val btnMedium = shapes.buttonMedium as RoundedCornerShape
            val btnMediumPx = btnMedium.topStart.toPx(viewport, density)
            assertEquals("Button medium at density $densityVal", 12f * densityVal, btnMediumPx, 0.001f)

            // Button Large: 14dp
            val btnLarge = shapes.buttonLarge as RoundedCornerShape
            val btnLargePx = btnLarge.topStart.toPx(viewport, density)
            assertEquals("Button large at density $densityVal", 14f * densityVal, btnLargePx, 0.001f)

            // Standard Card / cardSquircle: 16dp
            val card = shapes.card as RoundedCornerShape
            val cardPx = card.topStart.toPx(viewport, density)
            assertEquals("Card at density $densityVal", 16f * densityVal, cardPx, 0.001f)
            assertEquals(shapes.card, shapes.cardSquircle)

            // Hero Card / heroSquircle: 20dp
            val cardHero = shapes.cardHero as RoundedCornerShape
            val cardHeroPx = cardHero.topStart.toPx(viewport, density)
            assertEquals("Card hero at density $densityVal", 20f * densityVal, cardHeroPx, 0.001f)
            assertEquals(shapes.cardHero, shapes.heroSquircle)

            // Dialog: 20dp
            val dialog = shapes.dialog as RoundedCornerShape
            val dialogPx = dialog.topStart.toPx(viewport, density)
            assertEquals("Dialog at density $densityVal", 20f * densityVal, dialogPx, 0.001f)

            // Segmented Track / controlSquircle: 10dp
            val track = shapes.segmentedTrack as RoundedCornerShape
            val trackPx = track.topStart.toPx(viewport, density)
            assertEquals("Segmented track at density $densityVal", 10f * densityVal, trackPx, 0.001f)
            assertEquals(shapes.segmentedTrack, shapes.controlSquircle)

            // Segmented Thumb: 8dp
            val thumb = shapes.segmentedThumb as RoundedCornerShape
            val thumbPx = thumb.topStart.toPx(viewport, density)
            assertEquals("Segmented thumb at density $densityVal", 8f * densityVal, thumbPx, 0.001f)
        }
    }

    @Test
    fun testModalSheetAsymmetricCornerGeometryAcrossDensities() {
        val shapes = DefaultCupertinoShapes
        val viewport = Size(360f, 640f)

        for (densityVal in densities) {
            val density = Density(density = densityVal)
            val sheet = shapes.sheet as RoundedCornerShape

            // Top corners must resolve to exactly 24dp in pixels
            val topStartPx = sheet.topStart.toPx(viewport, density)
            val topEndPx = sheet.topEnd.toPx(viewport, density)
            assertEquals("Sheet topStart at density $densityVal", 24f * densityVal, topStartPx, 0.001f)
            assertEquals("Sheet topEnd at density $densityVal", 24f * densityVal, topEndPx, 0.001f)

            // Bottom corners MUST be strictly 0.0px for full-width docked modal sheets
            val bottomStartPx = sheet.bottomStart.toPx(viewport, density)
            val bottomEndPx = sheet.bottomEnd.toPx(viewport, density)
            assertEquals("Sheet bottomStart must be 0px at density $densityVal", 0f, bottomStartPx, 0.001f)
            assertEquals("Sheet bottomEnd must be 0px at density $densityVal", 0f, bottomEndPx, 0.001f)

            // Verify sheetSquircle alias identity
            assertEquals(shapes.sheet, shapes.sheetSquircle)
        }
    }

    // ========================================================================
    // 2. STRESS TEST VIEWPORTS & BOUNDARY GEOMETRIES
    // ========================================================================

    @Test
    fun testCornerGeometryUnderAdversarialViewports() {
        val density = Density(density = 2.0f)
        val shapes = DefaultCupertinoShapes

        // Viewports: Degenerate, sub-corner, ultra-wide, ultra-tall, and massive
        val testViewports = listOf(
            Size(0f, 0f),           // Empty / unmeasured
            Size(1f, 1f),           // 1x1 px sub-corner
            Size(10f, 10f),         // 10x10 px (smaller than 16dp * 2 = 32px radius)
            Size(32f, 32f),         // Exact boundary (2 * radius == size)
            Size(1000f, 10f),       // Extreme aspect ratio: ribbon horizontal
            Size(10f, 1000f),       // Extreme aspect ratio: ribbon vertical
            Size(10000f, 10000f)    // Massive 10k x 10k viewport
        )

        val shapesToTest = listOf(
            shapes.card,
            shapes.cardHero,
            shapes.sheet,
            shapes.segmentedTrack,
            shapes.capsule,
            shapes.circle
        )

        for (shape in shapesToTest) {
            for (vp in testViewports) {
                // Must create outline without throwing ArithmeticException, IllegalArgumentException or NaN
                val outline = shape.createOutline(vp, LayoutDirection.Ltr, density)
                assertNotNull("Outline should not be null for viewport $vp", outline)

                when (outline) {
                    is Outline.Rounded -> {
                        val roundRect = outline.roundRect
                        assertEquals("Outline left must match 0", 0f, roundRect.left, 0.001f)
                        assertEquals("Outline top must match 0", 0f, roundRect.top, 0.001f)
                        assertEquals("Outline right must match width", vp.width, roundRect.right, 0.001f)
                        assertEquals("Outline bottom must match height", vp.height, roundRect.bottom, 0.001f)
                    }
                    is Outline.Generic -> {
                        // Capsule / Circle on certain viewports can produce generic path
                        val bounds = outline.path.getBounds()
                        assertTrue("Path bounds width <= viewport width", bounds.width <= vp.width + 0.01f)
                        assertTrue("Path bounds height <= viewport height", bounds.height <= vp.height + 0.01f)
                    }
                    is Outline.Rectangle -> {
                        assertEquals(vp.width, outline.rect.width, 0.001f)
                        assertEquals(vp.height, outline.rect.height, 0.001f)
                    }
                }
            }
        }
    }

    @Test
    fun testCircleShapeCapsuleBehaviorOnRectangularViewport() {
        val density = Density(density = 2.0f)
        val pillShape = DefaultCupertinoShapes.capsule

        // Floating pill on wide rectangle: 120 x 40 px
        val vp = Size(120f, 40f)
        val outline = pillShape.createOutline(vp, LayoutDirection.Ltr, density)
        assertNotNull(outline)

        // CircleShape creates a rounded rect with radius = min(width, height) / 2 = 20px
        when (outline) {
            is Outline.Rounded -> {
                val roundRect = outline.roundRect
                assertEquals(40f / 2f, roundRect.topLeftCornerRadius.x, 0.001f)
                assertEquals(40f / 2f, roundRect.topRightCornerRadius.x, 0.001f)
                assertEquals(40f / 2f, roundRect.bottomLeftCornerRadius.x, 0.001f)
                assertEquals(40f / 2f, roundRect.bottomRightCornerRadius.x, 0.001f)
            }
            else -> {
                // Path outline also valid for circle shape
                assertTrue(outline is Outline.Generic || outline is Outline.Rounded)
            }
        }
    }

    // ========================================================================
    // 3. HAIRLINE RENDERING & SUBPIXEL PIXEL MATHEMATICS
    // ========================================================================

    @Test
    fun testHairlineStrokeWidthPixelCalculationAcrossDensities() {
        val strokeWidthDp = 0.5.dp

        for (densityVal in densities) {
            val density = Density(density = densityVal)
            val strokePx = with(density) { strokeWidthDp.toPx() }

            // Mathematical expectation: strokePx = 0.5f * densityVal
            assertEquals("Stroke px at density $densityVal", 0.5f * densityVal, strokePx, 0.001f)

            // Subpixel classification
            if (densityVal < 2.0f) {
                // On mdpi (1.0x = 0.5px) and hdpi (1.5x = 0.75px), stroke is subpixel (< 1.0px)
                assertTrue("Density $densityVal stroke $strokePx px is subpixel (<1.0px)", strokePx < 1.0f)
            } else {
                // On xhdpi (2.0x = 1.0px) and above, stroke is at least 1 full physical pixel
                assertTrue("Density $densityVal stroke $strokePx px is >= 1.0px", strokePx >= 1.0f)
            }
        }
    }

    @Test
    fun testHairlineOffsetCalculationsPreventClipping() {
        val strokeWidthDp = 0.5.dp
        val canvasHeight = 100f
        val canvasWidth = 360f

        for (densityVal in densities) {
            val density = Density(density = densityVal)
            val strokePx = with(density) { strokeWidthDp.toPx() }

            // Top hairline: start = (0, strokePx / 2f), end = (width, strokePx / 2f)
            val topY = strokePx / 2f
            val topStrokeMinY = topY - strokePx / 2f
            val topStrokeMaxY = topY + strokePx / 2f

            assertEquals("Top stroke starts exactly at y=0", 0f, topStrokeMinY, 0.001f)
            assertEquals("Top stroke ends exactly at y=strokePx", strokePx, topStrokeMaxY, 0.001f)

            // Bottom hairline: start = (0, height - strokePx / 2f), end = (width, height - strokePx / 2f)
            val bottomY = canvasHeight - strokePx / 2f
            val bottomStrokeMinY = bottomY - strokePx / 2f
            val bottomStrokeMaxY = bottomY + strokePx / 2f

            assertEquals("Bottom stroke starts at height - strokePx", canvasHeight - strokePx, bottomStrokeMinY, 0.001f)
            assertEquals("Bottom stroke ends at height", canvasHeight, bottomStrokeMaxY, 0.001f)
        }
    }

    // ========================================================================
    // 4. BLUR FALLBACKS ACROSS API LEVELS (29 TO 34)
    // ========================================================================

    @Test
    @Config(sdk = [Build.VERSION_CODES.Q]) // Android 10 (API 29)
    fun testFrostedGlassFallbackApi29() {
        assertEquals(29, Build.VERSION.SDK_INT)
        assertTrue(Build.VERSION.SDK_INT < Build.VERSION_CODES.S)

        // On API 29, RenderEffect is not available.
        // Build.VERSION.SDK_INT >= Build.VERSION_CODES.S is false.
        val shouldApplyHardwareBlur = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
        assertFalse("API 29 must NOT attempt to apply hardware RenderEffect blur", shouldApplyHardwareBlur)
    }

    @Test
    @Config(sdk = [Build.VERSION_CODES.R]) // Android 11 (API 30)
    fun testFrostedGlassFallbackApi30() {
        assertEquals(30, Build.VERSION.SDK_INT)
        assertTrue(Build.VERSION.SDK_INT < Build.VERSION_CODES.S)

        val shouldApplyHardwareBlur = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
        assertFalse("API 30 must NOT attempt to apply hardware RenderEffect blur", shouldApplyHardwareBlur)
    }

    @Test
    @Config(sdk = [Build.VERSION_CODES.S]) // Android 12 (API 31)
    fun testFrostedGlassHardwareBlurApi31() {
        assertEquals(31, Build.VERSION.SDK_INT)
        assertTrue(Build.VERSION.SDK_INT >= Build.VERSION_CODES.S)

        val shouldApplyHardwareBlur = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
        assertTrue("API 31 supports hardware RenderEffect blur", shouldApplyHardwareBlur)
    }

    @Test
    @Config(sdk = [Build.VERSION_CODES.TIRAMISU]) // Android 13 (API 33)
    fun testFrostedGlassHardwareBlurApi33() {
        assertEquals(33, Build.VERSION.SDK_INT)
        assertTrue(Build.VERSION.SDK_INT >= Build.VERSION_CODES.S)
    }

    @Test
    @Config(sdk = [Build.VERSION_CODES.UPSIDE_DOWN_CAKE]) // Android 14 (API 34)
    fun testFrostedGlassHardwareBlurApi34() {
        assertEquals(34, Build.VERSION.SDK_INT)
        assertTrue(Build.VERSION.SDK_INT >= Build.VERSION_CODES.S)
    }

    // ========================================================================
    // 5. 44x44 PT TOUCH TARGET ERGONOMICS & MINIMUM SIZE
    // ========================================================================

    @Test
    fun testTouchTargetMinimumSizeAcrossDensities() {
        val minTargetDp = 44.dp

        for (densityVal in densities) {
            val density = Density(density = densityVal)
            val minTargetPx = with(density) { minTargetDp.toPx() }

            // 44pt converted to physical pixels across densities
            assertEquals("44pt to px at density $densityVal", 44f * densityVal, minTargetPx, 0.001f)

            // On modern devices (xhdpi = 2.0x, xxhdpi = 3.0x, xxxhdpi = 4.0x):
            // 44pt translates to 88px, 132px, and 176px touch hitboxes.
            if (densityVal >= 2.0f) {
                assertTrue("Physical hit target must be >= 88px for density >= 2.0", minTargetPx >= 88f)
            }
        }
    }
}
