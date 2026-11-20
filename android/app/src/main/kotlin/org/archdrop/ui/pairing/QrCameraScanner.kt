package org.archdrop.ui.pairing

import android.Manifest
import android.content.pm.PackageManager
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.Camera
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.FlashOff
import androidx.compose.material.icons.filled.FlashOn
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import org.archdrop.ui.theme.ArchDropTheme
import org.archdrop.ui.theme.CupertinoButton
import org.archdrop.ui.theme.CupertinoButtonStyle
import org.archdrop.ui.theme.CupertinoCard
import org.archdrop.ui.theme.CupertinoIconButton
import org.archdrop.ui.theme.CupertinoTextField
import org.archdrop.ui.theme.cupertinoHairline
import org.archdrop.ui.theme.cupertinoTouchTarget
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.MultiFormatReader
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import org.json.JSONObject
import java.net.URLEncoder
import java.nio.ByteBuffer
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

data class PairingPayload(
    val version: Int = 1,
    val alias: String,
    val meshKey: String,
    val fingerprint: String,
    val port: Int = 53317
) {
    fun toUri(): String {
        val encodedAlias = URLEncoder.encode(alias, "UTF-8")
        return "archdrop://pair?v=$version&alias=$encodedAlias&meshKey=$meshKey&fp=$fingerprint&port=$port"
    }

    companion object {
        fun parse(raw: String): PairingPayload? {
            val text = raw.trim()
            if (text.startsWith("archdrop://pair")) {
                return try {
                    val uri = Uri.parse(text)
                    val v = uri.getQueryParameter("v")?.toIntOrNull() ?: 1
                    val alias = uri.getQueryParameter("alias") ?: "Arch Drop Peer"
                    val meshKey = uri.getQueryParameter("meshKey") ?: return null
                    val fp = uri.getQueryParameter("fp") ?: ""
                    val port = uri.getQueryParameter("port")?.toIntOrNull() ?: 53317
                    if (meshKey.length != 64) return null
                    PairingPayload(v, alias, meshKey.uppercase(), fp.uppercase(), port)
                } catch (_: Exception) {
                    null
                }
            }
            if (text.startsWith("{") && text.endsWith("}")) {
                return try {
                    val json = JSONObject(text)
                    val v = json.optInt("version", 1)
                    val alias = json.optString("alias", "Arch Drop Peer")
                    val meshKey = json.optString("meshKey", "")
                    val fp = json.optString("fingerprint", "")
                    val port = json.optInt("port", 53317)
                    if (meshKey.length != 64) return null
                    PairingPayload(v, alias, meshKey.uppercase(), fp.uppercase(), port)
                } catch (_: Exception) {
                    null
                }
            }
            return null
        }
    }
}

class QrCodeImageAnalyzer(
    private val onQrCodeScanned: (String, QrCodeImageAnalyzer) -> Unit
) : ImageAnalysis.Analyzer {

    private val isScanning = AtomicBoolean(true)
    private val reader = MultiFormatReader().apply {
        setHints(mapOf(
            DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE),
            DecodeHintType.TRY_HARDER to true
        ))
    }

    override fun analyze(imageProxy: ImageProxy) {
        if (!isScanning.get()) {
            imageProxy.close()
            return
        }

        val buffer: ByteBuffer = imageProxy.planes[0].buffer
        val data = ByteArray(buffer.remaining())
        buffer.get(data)

        val width = imageProxy.width
        val height = imageProxy.height

        val source = PlanarYUVLuminanceSource(
            data, width, height, 0, 0, width, height, false
        )
        val binaryBitmap = BinaryBitmap(HybridBinarizer(source))

        try {
            val result = reader.decodeWithState(binaryBitmap)
            if (result != null && isScanning.compareAndSet(true, false)) {
                onQrCodeScanned(result.text, this)
            }
        } catch (_: Exception) {
            // Frame did not contain a valid QR code
        } finally {
            reader.reset()
            imageProxy.close()
        }
    }

    fun resumeScanning() {
        isScanning.set(true)
    }

    fun pauseScanning() {
        isScanning.set(false)
    }
}

@Composable
fun QrCameraScanner(
    onPayloadScanned: (PairingPayload) -> Unit,
    modifier: Modifier = Modifier
) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current

    var hasCameraPermission by remember {
        mutableStateOf(
            ContextCompat.checkSelfPermission(
                context,
                Manifest.permission.CAMERA
            ) == PackageManager.PERMISSION_GRANTED
        )
    }

    val permissionLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.RequestPermission()
    ) { granted ->
        hasCameraPermission = granted
    }

    var camera by remember { mutableStateOf<Camera?>(null) }
    var isTorchOn by remember { mutableStateOf(false) }
    var manualInput by remember { mutableStateOf("") }
    var showManualInput by remember { mutableStateOf(false) }
    var parseError by remember { mutableStateOf<String?>(null) }

    val cameraExecutor = remember { Executors.newSingleThreadExecutor() }
    val analyzer = remember {
        QrCodeImageAnalyzer { rawText, analyzerInstance ->
            val payload = PairingPayload.parse(rawText)
            if (payload != null) {
                onPayloadScanned(payload)
            } else {
                analyzerInstance.resumeScanning()
            }
        }
    }

    DisposableEffect(Unit) {
        onDispose {
            cameraExecutor.shutdown()
        }
    }

    Box(modifier = modifier.fillMaxSize()) {
        if (!hasCameraPermission) {
            Column(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(24.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center
            ) {
                Text(
                    text = "Camera Permission Required",
                    style = ArchDropTheme.typography.title2,
                    color = ArchDropTheme.colors.label,
                    textAlign = androidx.compose.ui.text.style.TextAlign.Center
                )
                Spacer(modifier = Modifier.height(8.dp))
                Text(
                    text = "Camera access is needed to scan desktop or peer QR codes for cryptographic pairing.",
                    style = ArchDropTheme.typography.body,
                    color = ArchDropTheme.colors.secondaryLabel,
                    textAlign = androidx.compose.ui.text.style.TextAlign.Center
                )
                Spacer(modifier = Modifier.height(20.dp))
                CupertinoButton(
                    onClick = { permissionLauncher.launch(Manifest.permission.CAMERA) },
                    style = CupertinoButtonStyle.FILLED,
                    shape = ArchDropTheme.shapes.buttonLarge
                ) {
                    Text(
                        text = "Grant Camera Permission",
                        style = ArchDropTheme.typography.headline
                    )
                }
                Spacer(modifier = Modifier.height(12.dp))
                CupertinoButton(
                    onClick = { showManualInput = true },
                    style = CupertinoButtonStyle.PLAIN
                ) {
                    Text(
                        text = "Enter Pairing Code Manually",
                        style = ArchDropTheme.typography.headline,
                        color = ArchDropTheme.colors.systemBlue
                    )
                }
            }
        } else {
            AndroidView(
                factory = { ctx ->
                    val previewView = PreviewView(ctx)
                    val cameraProviderFuture = ProcessCameraProvider.getInstance(ctx)

                    cameraProviderFuture.addListener({
                        try {
                            val cameraProvider = cameraProviderFuture.get()
                            val preview = Preview.Builder().build().also {
                                it.surfaceProvider = previewView.surfaceProvider
                            }

                            val imageAnalysis = ImageAnalysis.Builder()
                                .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                                .build()
                                .also {
                                    it.setAnalyzer(cameraExecutor, analyzer)
                                }

                            val cameraSelector = CameraSelector.DEFAULT_BACK_CAMERA
                            cameraProvider.unbindAll()
                            val cam = cameraProvider.bindToLifecycle(
                                lifecycleOwner,
                                cameraSelector,
                                preview,
                                imageAnalysis
                            )
                            camera = cam
                        } catch (_: Exception) {
                            showManualInput = true
                        }
                    }, ContextCompat.getMainExecutor(ctx))

                    previewView
                },
                modifier = Modifier.fillMaxSize()
            )

            // Scanning Reticle Overlay with Apple continuous squircle
            Box(
                modifier = Modifier.fillMaxSize(),
                contentAlignment = Alignment.Center
            ) {
                Box(
                    modifier = Modifier
                        .size(260.dp)
                        .clip(ArchDropTheme.shapes.cardHero)
                        .border(
                            width = 2.5.dp,
                            color = ArchDropTheme.colors.systemBlue,
                            shape = ArchDropTheme.shapes.cardHero
                        )
                )
            }

            // Controls Overlay (Torch & Manual Fallback)
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(24.dp)
                    .align(Alignment.BottomCenter),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                CupertinoIconButton(
                    onClick = {
                        val newTorchState = !isTorchOn
                        camera?.cameraControl?.enableTorch(newTorchState)
                        isTorchOn = newTorchState
                    },
                    modifier = Modifier.size(44.dp),
                    backgroundColor = ArchDropTheme.colors.secondarySystemGroupedBackground
                ) {
                    Icon(
                        imageVector = if (isTorchOn) Icons.Default.FlashOff else Icons.Default.FlashOn,
                        contentDescription = "Toggle Torch",
                        tint = if (isTorchOn) ArchDropTheme.colors.systemYellow else ArchDropTheme.colors.label
                    )
                }

                CupertinoButton(
                    onClick = { showManualInput = true },
                    style = CupertinoButtonStyle.TONAL,
                    shape = ArchDropTheme.shapes.buttonMedium
                ) {
                    Text(
                        text = "Manual Code",
                        style = ArchDropTheme.typography.headline,
                        color = ArchDropTheme.colors.label
                    )
                }
            }
        }

        // Manual text fallback dialog / bottom sheet
        if (showManualInput) {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .background(Color.Black.copy(alpha = 0.5f))
                    .padding(24.dp),
                contentAlignment = Alignment.Center
            ) {
                CupertinoCard(
                    shape = ArchDropTheme.shapes.dialog,
                    backgroundColor = ArchDropTheme.colors.secondarySystemGroupedBackground,
                    modifier = Modifier.fillMaxWidth()
                ) {
                    Column(modifier = Modifier.padding(20.dp)) {
                        Text(
                            text = "Manual Pairing Entry",
                            style = ArchDropTheme.typography.title2,
                            color = ArchDropTheme.colors.label
                        )
                        Spacer(modifier = Modifier.height(8.dp))
                        Text(
                            text = "Paste an 'archdrop://pair' URI or pairing JSON payload:",
                            style = ArchDropTheme.typography.subheadline,
                            color = ArchDropTheme.colors.secondaryLabel
                        )
                        Spacer(modifier = Modifier.height(12.dp))
                        CupertinoTextField(
                            value = manualInput,
                            onValueChange = {
                                manualInput = it
                                parseError = null
                            },
                            placeholder = "archdrop://pair?v=1&...",
                            modifier = Modifier.fillMaxWidth()
                        )
                        if (parseError != null) {
                            Text(
                                text = parseError ?: "",
                                color = ArchDropTheme.colors.systemRed,
                                style = ArchDropTheme.typography.footnote,
                                modifier = Modifier.padding(top = 4.dp)
                            )
                        }
                        Spacer(modifier = Modifier.height(16.dp))
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.End,
                            verticalAlignment = Alignment.CenterVertically
                        ) {
                            CupertinoButton(
                                onClick = { showManualInput = false },
                                style = CupertinoButtonStyle.PLAIN
                            ) {
                                Text(
                                    text = "Cancel",
                                    style = ArchDropTheme.typography.headline,
                                    color = ArchDropTheme.colors.secondaryLabel
                                )
                            }
                            Spacer(modifier = Modifier.width(8.dp))
                            CupertinoButton(
                                onClick = {
                                    val parsed = PairingPayload.parse(manualInput)
                                    if (parsed != null) {
                                        showManualInput = false
                                        onPayloadScanned(parsed)
                                    } else {
                                        parseError = "Invalid pairing URI or JSON. Must contain a 64-character hex Ed25519 meshKey."
                                    }
                                },
                                style = CupertinoButtonStyle.FILLED,
                                shape = ArchDropTheme.shapes.buttonMedium
                            ) {
                                Text(
                                    text = "Parse & Pair",
                                    style = ArchDropTheme.typography.headline,
                                    fontWeight = FontWeight.SemiBold
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}
