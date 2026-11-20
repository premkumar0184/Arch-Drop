package org.archdrop.ui.pairing

import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.qrcode.QRCodeWriter
import com.google.zxing.qrcode.decoder.ErrorCorrectionLevel
import org.archdrop.ui.theme.ArchDropTheme
import org.archdrop.ui.theme.cupertinoHairline

/**
 * High-contrast Apple HIG QR code card designed for instant optical camera decodability.
 * Enclosed in an authentic Apple continuous squircle card with hairline borders.
 */
@Composable
fun QrCodeView(
    content: String,
    modifier: Modifier = Modifier,
    size: Dp = 260.dp,
    foregroundColor: Int = android.graphics.Color.BLACK,
    backgroundColor: Int = android.graphics.Color.WHITE
) {
    val bitmap = remember(content, foregroundColor, backgroundColor) {
        generateQrBitmap(content, 512, foregroundColor, backgroundColor)
    }

    Box(
        modifier = modifier
            .size(size)
            .clip(ArchDropTheme.shapes.cardSquircle)
            .background(Color.White)
            .cupertinoHairline(shape = ArchDropTheme.shapes.cardSquircle)
            .padding(16.dp),
        contentAlignment = Alignment.Center
    ) {
        if (bitmap != null) {
            Image(
                bitmap = bitmap.asImageBitmap(),
                contentDescription = "Cryptographic Pairing QR Code",
                modifier = Modifier.size(size - 32.dp)
            )
        }
    }
}

fun generateQrBitmap(
    content: String,
    dimension: Int,
    foregroundColor: Int,
    backgroundColor: Int
): Bitmap? {
    if (content.isEmpty()) return null
    return try {
        val hints = mapOf(
            EncodeHintType.ERROR_CORRECTION to ErrorCorrectionLevel.M,
            EncodeHintType.MARGIN to 1
        )
        val bitMatrix = QRCodeWriter().encode(content, BarcodeFormat.QR_CODE, dimension, dimension, hints)
        val width = bitMatrix.width
        val height = bitMatrix.height
        val pixels = IntArray(width * height)
        for (y in 0 until height) {
            val offset = y * width
            for (x in 0 until width) {
                pixels[offset + x] = if (bitMatrix.get(x, y)) foregroundColor else backgroundColor
            }
        }
        Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888).apply {
            setPixels(pixels, 0, width, 0, 0, width, height)
        }
    } catch (_: Exception) {
        null
    }
}
