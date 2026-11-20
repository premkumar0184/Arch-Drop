package org.archdrop.clipboard

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Log
import android.webkit.MimeTypeMap
import org.archdrop.clipboard.models.ClipboardClearPacket
import org.archdrop.clipboard.models.ClipboardDescriptorPacket
import org.archdrop.clipboard.models.ClipboardDescriptorPayload
import org.archdrop.clipboard.models.ClipboardInlinePacket
import org.archdrop.clipboard.models.ClipboardInlinePayload
import java.io.ByteArrayOutputStream
import java.io.File
import java.security.MessageDigest
import java.util.UUID

sealed class ClipContent {
    data class Text(val text: String) : ClipContent()
    data class Image(
        val bytes: ByteArray,
        val mimeType: String,
        val fileName: String,
        val sha256: String
    ) : ClipContent()
    data class FilePayload(
        val uri: Uri,
        val fileName: String,
        val fileSize: Long,
        val mimeType: String,
        val sha256: String? = null
    ) : ClipContent()
    data class Error(val message: String) : ClipContent()
    data object Empty : ClipContent()
}

class ClipboardSyncManager(private val context: Context, private val deviceId: String) {

    companion object {
        const val TAG = "ArchDrop:Clipboard"
        const val INLINE_THRESHOLD = 4096
        const val MAX_IMAGE_PAYLOAD_SIZE = 4 * 1024 * 1024 // 4 MB
        const val EPHEMERAL_TTL_SECS = 120L

        private val payloadCache = java.util.concurrent.ConcurrentHashMap<String, CachedClipboardPayload>()

        @Volatile
        private var lastPastedHash: String? = null

        fun sha256(bytes: ByteArray): String {
            val md = MessageDigest.getInstance("SHA-256")
            return md.digest(bytes).joinToString("") { "%02x".format(it) }
        }

        fun sha256(text: String): String = sha256(text.toByteArray(Charsets.UTF_8))
    }

    private val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager

    data class CachedClipboardPayload(
        val payloadId: String,
        val token: String,
        val data: ByteArray,
        val mimeType: String,
        val createdAt: Long = System.currentTimeMillis()
    )

    fun getCachedPayload(payloadId: String, token: String): ByteArray? {
        val cached = payloadCache[payloadId] ?: return null
        if (cached.token != token) return null
        if (System.currentTimeMillis() - cached.createdAt > EPHEMERAL_TTL_SECS * 1000) {
            payloadCache.remove(payloadId)
            return null
        }
        return cached.data
    }

    fun packText(text: String, downloadUrlBase: String? = null): Any {
        val bytes = text.toByteArray(Charsets.UTF_8)
        val hash = sha256(bytes)
        val now = System.currentTimeMillis()

        return if (bytes.size <= INLINE_THRESHOLD) {
            ClipboardInlinePacket(
                deviceId = deviceId,
                timestamp = now,
                ttlSeconds = EPHEMERAL_TTL_SECS,
                payload = ClipboardInlinePayload(
                    mimeType = "text/plain;charset=utf-8",
                    content = text,
                    sha256 = hash,
                    sizeBytes = bytes.size
                )
            )
        } else {
            val payloadId = UUID.randomUUID().toString()
            val token = UUID.randomUUID().toString()
            val url = downloadUrlBase?.let { "$it/api/v2/clipboard/payload/$payloadId?token=$token" }
                ?: "/api/v2/clipboard/payload/$payloadId?token=$token"

            payloadCache[payloadId] = CachedClipboardPayload(
                payloadId = payloadId,
                token = token,
                data = bytes,
                mimeType = "text/plain;charset=utf-8"
            )

            ClipboardDescriptorPacket(
                deviceId = deviceId,
                timestamp = now,
                ttlSeconds = EPHEMERAL_TTL_SECS,
                descriptor = ClipboardDescriptorPayload(
                    payloadId = payloadId,
                    primaryMime = "text/plain;charset=utf-8",
                    availableMimes = listOf("text/plain;charset=utf-8"),
                    sizeBytes = bytes.size,
                    sha256 = hash,
                    ephemeralToken = token,
                    downloadUrl = url
                )
            )
        }
    }

    fun packImage(bytes: ByteArray, mimeType: String = "image/png", downloadUrlBase: String? = null): Any {
        val hash = sha256(bytes)
        val now = System.currentTimeMillis()

        return if (bytes.size <= INLINE_THRESHOLD) {
            val b64 = java.util.Base64.getEncoder().encodeToString(bytes)
            val content = "data:$mimeType;base64,$b64"
            ClipboardInlinePacket(
                deviceId = deviceId,
                timestamp = now,
                ttlSeconds = EPHEMERAL_TTL_SECS,
                payload = ClipboardInlinePayload(
                    mimeType = mimeType,
                    content = content,
                    sha256 = hash,
                    sizeBytes = bytes.size
                )
            )
        } else {
            val payloadId = UUID.randomUUID().toString()
            val token = UUID.randomUUID().toString()
            val url = downloadUrlBase?.let { "$it/api/v2/clipboard/payload/$payloadId?token=$token" }
                ?: "/api/v2/clipboard/payload/$payloadId?token=$token"

            payloadCache[payloadId] = CachedClipboardPayload(
                payloadId = payloadId,
                token = token,
                data = bytes,
                mimeType = mimeType
            )

            ClipboardDescriptorPacket(
                deviceId = deviceId,
                timestamp = now,
                ttlSeconds = EPHEMERAL_TTL_SECS,
                descriptor = ClipboardDescriptorPayload(
                    payloadId = payloadId,
                    primaryMime = mimeType,
                    availableMimes = listOf(mimeType),
                    sizeBytes = bytes.size,
                    sha256 = hash,
                    ephemeralToken = token,
                    downloadUrl = url
                )
            )
        }
    }

    fun inspectPrimaryClip(): ClipContent {
        val clip = clipboard?.primaryClip ?: return ClipContent.Empty
        if (clip.itemCount == 0) return ClipContent.Empty
        return resolveClip(clip)
    }

    fun resolveClip(clip: ClipData?): ClipContent {
        if (clip == null || clip.itemCount == 0) return ClipContent.Empty
        val item = clip.getItemAt(0)
        val description = clip.description

        if (item.uri != null) {
            return resolveUri(item.uri, description)
        }

        val text = item.text?.toString()
        if (!text.isNullOrEmpty()) {
            val trimmed = text.trim()
            val firstLine = trimmed.lines().firstOrNull { it.isNotBlank() && !it.startsWith("#") } ?: ""
            if (firstLine.startsWith("content://", ignoreCase = true) || firstLine.startsWith("file://", ignoreCase = true)) {
                try {
                    val parsed = Uri.parse(firstLine)
                    return resolveUri(parsed, description)
                } catch (e: SecurityException) {
                    return ClipContent.Error("Permission denied for URI: ${e.message}")
                } catch (e: Exception) {
                    Log.d(TAG, "Failed parsing text as URI: ${e.message}")
                }
            }
            return ClipContent.Text(text)
        }

        return ClipContent.Empty
    }

    fun resolveUri(uri: Uri, description: ClipDescription? = null): ClipContent {
        try {
            var mime: String? = null
            if (uri.scheme.equals("content", ignoreCase = true)) {
                try {
                    mime = context.contentResolver.getType(uri)
                } catch (e: SecurityException) {
                    return ClipContent.Error("Permission denied accessing item: ${e.message}")
                }
            }

            if (mime.isNullOrEmpty() || mime == "application/octet-stream") {
                val ext = getExtensionFromUri(uri)
                if (ext.isNotEmpty()) {
                    mime = getMimeFromExtension(ext)
                }
            }

            if (mime.isNullOrEmpty() || mime == "application/octet-stream") {
                if (description != null && description.mimeTypeCount > 0) {
                    for (i in 0 until description.mimeTypeCount) {
                        val m = description.getMimeType(i)
                        if (!m.startsWith("text/plain") && !m.startsWith("text/uri-list")) {
                            mime = m
                            break
                        }
                    }
                }
            }

            val finalMime = mime ?: "application/octet-stream"

            var displayName: String = uri.lastPathSegment ?: "clipboard_file"
            var fileSize: Long = -1L

            if (uri.scheme.equals("content", ignoreCase = true)) {
                try {
                    context.contentResolver.query(
                        uri,
                        arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE),
                        null,
                        null,
                        null
                    )?.use { cursor ->
                        if (cursor.moveToFirst()) {
                            val nameIdx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                            if (nameIdx != -1 && !cursor.isNull(nameIdx)) {
                                val n = cursor.getString(nameIdx)
                                if (!n.isNullOrBlank()) displayName = n
                            }
                            val sizeIdx = cursor.getColumnIndex(OpenableColumns.SIZE)
                            if (sizeIdx != -1 && !cursor.isNull(sizeIdx)) {
                                fileSize = cursor.getLong(sizeIdx)
                            }
                        }
                    }
                } catch (e: SecurityException) {
                    return ClipContent.Error("Permission denied accessing item: ${e.message}")
                } catch (e: Exception) {
                    Log.d(TAG, "Exception querying OpenableColumns: ${e.message}")
                }
            } else if (uri.scheme.equals("file", ignoreCase = true)) {
                try {
                    val path = uri.path
                    if (path != null) {
                        val file = File(path)
                        if (file.exists()) {
                            displayName = file.name
                            fileSize = file.length()
                        }
                    }
                } catch (e: SecurityException) {
                    return ClipContent.Error("Permission denied reading file: ${e.message}")
                } catch (e: Exception) {
                    Log.d(TAG, "Exception accessing file: ${e.message}")
                }
            }

            val isImage = finalMime.startsWith("image/", ignoreCase = true)
            if (isImage) {
                if (fileSize > MAX_IMAGE_PAYLOAD_SIZE) {
                    return ClipContent.FilePayload(
                        uri = uri,
                        fileName = displayName,
                        fileSize = fileSize,
                        mimeType = finalMime
                    )
                }

                return try {
                    val stream = context.contentResolver.openInputStream(uri)
                        ?: return ClipContent.Error("Cannot open input stream for $uri")
                    val baos = ByteArrayOutputStream()
                    val buf = ByteArray(8192)
                    var total = 0
                    val limit = MAX_IMAGE_PAYLOAD_SIZE + 1
                    stream.use { input ->
                        while (true) {
                            val r = input.read(buf)
                            if (r == -1) break
                            baos.write(buf, 0, r)
                            total += r
                            if (total >= limit) break
                        }
                    }
                    val bytes = baos.toByteArray()
                    if (bytes.size > MAX_IMAGE_PAYLOAD_SIZE) {
                        ClipContent.FilePayload(
                            uri = uri,
                            fileName = displayName,
                            fileSize = fileSize.coerceAtLeast(bytes.size.toLong()),
                            mimeType = finalMime
                        )
                    } else {
                        val hash = sha256(bytes)
                        ClipContent.Image(
                            bytes = bytes,
                            mimeType = finalMime,
                            fileName = displayName,
                            sha256 = hash
                        )
                    }
                } catch (e: SecurityException) {
                    ClipContent.Error("Permission denied reading image: ${e.message}")
                } catch (e: Exception) {
                    ClipContent.Error("Failed to read image stream: ${e.message}")
                }
            } else {
                return ClipContent.FilePayload(
                    uri = uri,
                    fileName = displayName,
                    fileSize = if (fileSize > 0) fileSize else 0L,
                    mimeType = finalMime
                )
            }
        } catch (e: SecurityException) {
            return ClipContent.Error("Permission denied accessing item: ${e.message}")
        } catch (e: Exception) {
            return ClipContent.Error("Failed to resolve item: ${e.message}")
        }
    }

    fun pasteToSystem(text: String): Boolean {
        val hash = sha256(text.toByteArray(Charsets.UTF_8))
        lastPastedHash = hash

        try {
            val clip = ClipData.newPlainText("Arch Drop", text)
            clipboard?.setPrimaryClip(clip)
        } catch (_: Exception) {
            // Ignored in test / background context
        }
        return true
    }

    fun isRecentHash(hash: String): Boolean = lastPastedHash?.equals(hash, ignoreCase = true) == true

    fun isRecentPaste(text: String): Boolean {
        val hash = sha256(text.toByteArray(Charsets.UTF_8))
        return isRecentHash(hash)
    }

    fun getLastPastedHash(): String? = lastPastedHash

    fun setLastPastedHash(hash: String?) {
        lastPastedHash = hash
    }

    fun clearSystemClipboard() {
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.P) {
            clipboard?.clearPrimaryClip()
        } else {
            clipboard?.setPrimaryClip(ClipData.newPlainText("", ""))
        }
    }

    private fun getExtensionFromUri(uri: Uri): String {
        val segment = uri.lastPathSegment ?: uri.path ?: ""
        val ext = segment.substringAfterLast('.', "")
        if (ext.isNotEmpty() && ext.length <= 10) {
            return ext
        }
        val fromUrl = MimeTypeMap.getFileExtensionFromUrl(uri.toString())
        return fromUrl ?: ""
    }

    private fun getMimeFromExtension(extension: String): String? {
        val ext = extension.lowercase().trimStart('.')
        return when (ext) {
            "png" -> "image/png"
            "jpg", "jpeg" -> "image/jpeg"
            "webp" -> "image/webp"
            "gif" -> "image/gif"
            "bmp" -> "image/bmp"
            "svg" -> "image/svg+xml"
            "pdf" -> "application/pdf"
            "txt" -> "text/plain"
            "json" -> "application/json"
            "zip" -> "application/zip"
            "mp4" -> "video/mp4"
            "mp3" -> "audio/mpeg"
            else -> MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext)
        }
    }
}
