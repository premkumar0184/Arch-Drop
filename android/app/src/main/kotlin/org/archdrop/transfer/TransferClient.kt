package org.archdrop.transfer

import android.content.Context
import android.net.Uri
import android.os.Build
import android.util.Log
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.MediaType.Companion.toMediaTypeOrNull
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import okio.BufferedSink
import okio.source
import org.archdrop.transfer.models.FileDto
import org.archdrop.transfer.models.PrepareUploadRequestDto
import org.archdrop.transfer.models.PrepareUploadResponseDto
import org.archdrop.transfer.models.RegisterDto
import java.io.File
import java.io.FileInputStream
import java.io.IOException
import java.io.InputStream
import java.util.UUID
import java.util.concurrent.TimeUnit

data class SendFileResult(
    val success: Boolean,
    val sessionId: String? = null,
    val fileName: String,
    val fileSize: Long,
    val errorMessage: String? = null
)

class TransferClient(
    private val context: Context,
    private val okHttpClient: OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(5, TimeUnit.SECONDS)
        .readTimeout(60, TimeUnit.SECONDS)
        .writeTimeout(60, TimeUnit.SECONDS)
        .build()
) {
    companion object {
        private const val TAG = "ArchDrop:TransferClient"
    }

    private val json = Json {
        encodeDefaults = true
        ignoreUnknownKeys = true
    }

    suspend fun sendStagedFile(
        targetHost: String,
        targetPort: Int = 53317,
        uri: Uri,
        fileName: String,
        fileSize: Long,
        mimeType: String = "application/octet-stream",
        senderAlias: String = "Android Device",
        senderFingerprint: String = "ANDROID_FP",
        sha256: String? = null
    ): SendFileResult = withContext(Dispatchers.IO) {
        val fileId = UUID.randomUUID().toString()

        // 1. Prepare Upload Request
        val prepareUrl = "http://$targetHost:$targetPort/api/localsend/v2/prepare-upload"
        val prepareDto = PrepareUploadRequestDto(
            info = RegisterDto(
                alias = senderAlias,
                version = "2.0",
                deviceModel = Build.MODEL ?: "Android",
                deviceType = "mobile",
                fingerprint = senderFingerprint,
                port = 53317,
                protocol = "http",
                download = true
            ),
            files = mapOf(
                fileId to FileDto(
                    id = fileId,
                    fileName = fileName,
                    size = fileSize,
                    fileType = mimeType,
                    sha256 = sha256
                )
            )
        )

        val prepareJson = json.encodeToString(prepareDto)
        val jsonMediaType = "application/json; charset=utf-8".toMediaType()
        val prepareReq = Request.Builder()
            .url(prepareUrl)
            .post(prepareJson.toRequestBody(jsonMediaType))
            .header("Content-Type", "application/json")
            .header("X-Clipboard-Staged", "true")
            .header("X-Fingerprint", senderFingerprint)
            .build()

        val respDto: PrepareUploadResponseDto
        try {
            okHttpClient.newCall(prepareReq).execute().use { response ->
                if (!response.isSuccessful) {
                    val code = response.code
                    val err = "prepare-upload rejected with HTTP $code"
                    Log.w(TAG, err)
                    return@withContext SendFileResult(
                        success = false,
                        fileName = fileName,
                        fileSize = fileSize,
                        errorMessage = err
                    )
                }
                val bodyStr = response.body?.string() ?: ""
                respDto = json.decodeFromString(bodyStr)
            }
        } catch (e: Exception) {
            val err = "Failed to connect to prepare-upload at $prepareUrl: ${e.message}"
            Log.w(TAG, err, e)
            return@withContext SendFileResult(
                success = false,
                fileName = fileName,
                fileSize = fileSize,
                errorMessage = err
            )
        }

        val sessionId = respDto.sessionId
        val fileToken = respDto.files[fileId]
        if (fileToken.isNullOrEmpty()) {
            val err = "No upload token returned for file $fileName in session $sessionId"
            Log.w(TAG, err)
            return@withContext SendFileResult(
                success = false,
                sessionId = sessionId,
                fileName = fileName,
                fileSize = fileSize,
                errorMessage = err
            )
        }

        // 2. Stream File Upload
        val uploadUrl = "http://$targetHost:$targetPort/api/localsend/v2/upload?sessionId=$sessionId&fileId=$fileId&token=$fileToken"
        val streamingBody = object : RequestBody() {
            override fun contentType() = (mimeType.ifEmpty { "application/octet-stream" }).toMediaTypeOrNull()

            override fun contentLength(): Long = if (fileSize > 0) fileSize else -1L

            override fun writeTo(sink: BufferedSink) {
                val input: InputStream = openStream(uri)
                    ?: throw IOException("Cannot open input stream for URI: $uri")
                input.use { inStream ->
                    inStream.source().use { source ->
                        sink.writeAll(source)
                    }
                }
            }
        }

        val uploadReq = Request.Builder()
            .url(uploadUrl)
            .post(streamingBody)
            .header("Content-Type", mimeType.ifEmpty { "application/octet-stream" })
            .header("X-Clipboard-Staged", "true")
            .header("X-Fingerprint", senderFingerprint)
            .build()

        try {
            okHttpClient.newCall(uploadReq).execute().use { response ->
                if (response.isSuccessful) {
                    Log.i(TAG, "Successfully uploaded staged file $fileName ($fileSize bytes) to $uploadUrl")
                    return@withContext SendFileResult(
                        success = true,
                        sessionId = sessionId,
                        fileName = fileName,
                        fileSize = fileSize,
                        errorMessage = null
                    )
                } else {
                    val err = "Upload failed with HTTP ${response.code}"
                    Log.w(TAG, err)
                    return@withContext SendFileResult(
                        success = false,
                        sessionId = sessionId,
                        fileName = fileName,
                        fileSize = fileSize,
                        errorMessage = err
                    )
                }
            }
        } catch (e: Exception) {
            val err = "Exception uploading file to $uploadUrl: ${e.message}"
            Log.w(TAG, err, e)
            return@withContext SendFileResult(
                success = false,
                sessionId = sessionId,
                fileName = fileName,
                fileSize = fileSize,
                errorMessage = err
            )
        }
    }

    private fun openStream(uri: Uri): InputStream? {
        return try {
            if (uri.scheme.equals("file", ignoreCase = true)) {
                val path = uri.path
                if (path != null) {
                    val f = File(path)
                    if (f.exists()) return FileInputStream(f)
                }
            }
            context.contentResolver.openInputStream(uri)
        } catch (e: Exception) {
            Log.w(TAG, "Error opening stream for $uri: ${e.message}")
            null
        }
    }
}
