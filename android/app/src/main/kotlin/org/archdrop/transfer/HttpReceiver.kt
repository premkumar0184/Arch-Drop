package org.archdrop.transfer

import android.os.Build
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.archdrop.clipboard.ClipboardSyncManager
import org.archdrop.clipboard.models.ClipboardClearPacket
import org.archdrop.clipboard.models.ClipboardDescriptorPacket
import org.archdrop.clipboard.models.ClipboardInlinePacket
import org.archdrop.transfer.models.InfoResponseDto
import org.archdrop.transfer.models.PrepareUploadRequestDto
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.FileOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.URLDecoder
import java.security.MessageDigest

/**
 * Android-native zero-dependency HTTP server for LocalSend v2 REST & streaming file transfers.
 *
 * Implemented using pure Java ServerSocket and Kotlin coroutines on Dispatchers.IO,
 * eliminating all dependencies on desktop JDK-internal com.sun.net.httpserver APIs.
 * Binds to 0.0.0.0:53317 (INADDR_ANY), making the receiver automatically reachable
 * over Wi-Fi LAN, Local-Only Hotspot (LOHS) SoftAP interfaces, and WireGuard tunnels.
 */
class HttpReceiver(
    val port: Int = 53317,
    private val downloadDir: File,
    private val deviceAlias: String = "Android Device",
    private val fingerprint: String = "ANDROID_ARCHDROP_FP",
    var clipboardManager: ClipboardSyncManager? = null,
    private val onFileReceived: ((File, String) -> Unit)? = null
) {
    var onClipboardReceived: ((String, String) -> Unit)? = null
    companion object {
        private const val TAG = "ArchDrop:HttpReceiver"
        private const val BUFFER_SIZE = 512 * 1024 // 512 KiB streaming buffer
    }

    private var serverSocket: ServerSocket? = null
    private var job: Job? = null
    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())
    private val sessionManager = TransferSessionManager(downloadDir)
    private val json = Json { ignoreUnknownKeys = true }

    @Synchronized
    fun start() {
        if (serverSocket != null) return
        try {
            val ss = ServerSocket()
            ss.reuseAddress = true
            ss.bind(InetSocketAddress("0.0.0.0", port))
            serverSocket = ss
            Log.i(TAG, "HttpReceiver bound to 0.0.0.0:$port")

            job = scope.launch {
                while (isActive && !ss.isClosed) {
                    try {
                        val clientSocket = ss.accept()
                        launch {
                            handleClient(clientSocket)
                        }
                    } catch (e: Exception) {
                        if (ss.isClosed) break
                        Log.w(TAG, "Exception accepting socket connection", e)
                    }
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "Failed to start HttpReceiver on port $port", e)
        }
    }

    @Synchronized
    fun stop() {
        try {
            serverSocket?.close()
        } catch (e: Exception) {
            Log.w(TAG, "Exception closing server socket", e)
        }
        serverSocket = null
        job?.cancel()
        job = null
        Log.i(TAG, "HttpReceiver stopped")
    }

    private fun handleClient(socket: Socket) {
        socket.use { s ->
            try {
                s.soTimeout = 30_000 // 30 second read timeout
                val input = BufferedInputStream(s.getInputStream(), BUFFER_SIZE)
                val output = BufferedOutputStream(s.getOutputStream())

                val requestLine = readHttpLine(input) ?: return
                val parts = requestLine.split(" ")
                if (parts.size < 3) return
                val method = parts[0].uppercase()
                val fullUri = parts[1]

                val headers = mutableMapOf<String, String>()
                while (true) {
                    val line = readHttpLine(input) ?: break
                    if (line.isEmpty()) break
                    val idx = line.indexOf(':')
                    if (idx != -1) {
                        val name = line.substring(0, idx).trim().lowercase()
                        val value = line.substring(idx + 1).trim()
                        headers[name] = value
                    }
                }

                val path = if (fullUri.contains('?')) fullUri.substringBefore('?') else fullUri
                val queryStr = if (fullUri.contains('?')) fullUri.substringAfter('?') else null

                if (method == "OPTIONS") {
                    sendEmptyResponse(output, 200, "OK")
                    return
                }

                when {
                    path == "/api/localsend/v2/info" && method == "GET" -> {
                        val info = InfoResponseDto(
                            alias = deviceAlias,
                            version = "2.2",
                            deviceModel = Build.MODEL,
                            deviceType = "mobile",
                            fingerprint = fingerprint,
                            download = true
                        )
                        sendJsonResponse(output, 200, json.encodeToString(info))
                    }
                    path == "/api/localsend/v2/register" && method == "POST" -> {
                        val resp = InfoResponseDto(
                            alias = deviceAlias,
                            version = "2.2",
                            deviceModel = Build.MODEL,
                            deviceType = "mobile",
                            fingerprint = fingerprint,
                            download = true
                        )
                        sendJsonResponse(output, 200, json.encodeToString(resp))
                    }
                    path == "/api/localsend/v2/prepare-upload" && method == "POST" -> {
                        val contentLength = headers["content-length"]?.toIntOrNull() ?: 0
                        val bodyBytes = ByteArray(contentLength)
                        var read = 0
                        while (read < contentLength) {
                            val r = input.read(bodyBytes, read, contentLength - read)
                            if (r == -1) break
                            read += r
                        }
                        val body = String(bodyBytes, 0, read, Charsets.UTF_8)
                        try {
                            val req = json.decodeFromString<PrepareUploadRequestDto>(body)
                            val res = sessionManager.createSession(req.info, req.files)
                            sendJsonResponse(output, 200, json.encodeToString(res))
                        } catch (e: Exception) {
                            Log.e(TAG, "prepare-upload JSON parsing failed", e)
                            sendEmptyResponse(output, 400, "Bad Request")
                        }
                    }
                    path == "/api/localsend/v2/upload" && method == "POST" -> {
                        val queryMap = parseQuery(queryStr)
                        val sessionId = queryMap["sessionId"]
                        val fileId = queryMap["fileId"]
                        val token = queryMap["token"]

                        if (sessionId == null || fileId == null || token == null) {
                            sendEmptyResponse(output, 400, "Bad Request: Missing query parameters")
                            return
                        }
                        val state = sessionManager.getFileState(sessionId, fileId, token)
                        if (state == null) {
                            sendEmptyResponse(output, 403, "Forbidden: Invalid transfer token")
                            return
                        }

                        val isChunked = headers["transfer-encoding"]?.equals("chunked", ignoreCase = true) == true
                        val contentLength = headers["content-length"]?.toLongOrNull() ?: -1L
                        val md = MessageDigest.getInstance("SHA-256")
                        var totalBytes = 0L
                        val buffer = ByteArray(BUFFER_SIZE)

                        FileOutputStream(state.tempFile).use { fos ->
                            if (isChunked) {
                                while (true) {
                                    val sizeLine = readHttpLine(input) ?: break
                                    val chunkSizeStr = sizeLine.trim().split(";")[0].trim()
                                    if (chunkSizeStr.isEmpty()) continue
                                    val chunkSize = chunkSizeStr.toIntOrNull(16) ?: break
                                    if (chunkSize == 0) {
                                        readHttpLine(input) // consume trailing CRLF
                                        break
                                    }
                                    var chunkRemaining = chunkSize
                                    while (chunkRemaining > 0) {
                                        val toRead = if (chunkRemaining > buffer.size) buffer.size else chunkRemaining
                                        val r = input.read(buffer, 0, toRead)
                                        if (r == -1) break
                                        fos.write(buffer, 0, r)
                                        md.update(buffer, 0, r)
                                        totalBytes += r
                                        chunkRemaining -= r
                                    }
                                    readHttpLine(input) // consume chunk trailing CRLF
                                }
                            } else {
                                var remaining = if (contentLength >= 0) contentLength else Long.MAX_VALUE
                                while (remaining > 0) {
                                    val toRead = if (remaining > buffer.size) buffer.size else remaining.toInt()
                                    val r = input.read(buffer, 0, toRead)
                                    if (r == -1) break
                                    fos.write(buffer, 0, r)
                                    md.update(buffer, 0, r)
                                    totalBytes += r
                                    if (contentLength >= 0) remaining -= r
                                }
                            }
                            fos.flush()
                        }

                        val calculatedHash = md.digest().joinToString("") { "%02x".format(it) }
                        val success = sessionManager.completeFile(sessionId, fileId, totalBytes, calculatedHash)
                        if (success) {
                            if (headers["x-clipboard-staged"]?.equals("true", ignoreCase = true) == true) {
                                clipboardManager?.setLastPastedHash(calculatedHash)
                            }
                            Log.i(TAG, "File received successfully: ${state.finalFile.name} ($totalBytes bytes, hash=$calculatedHash)")
                            try {
                                onFileReceived?.invoke(state.finalFile, calculatedHash)
                            } catch (e: Exception) {
                                Log.w(TAG, "Exception in onFileReceived callback", e)
                            }
                            sendEmptyResponse(output, 200, "OK")
                        } else {
                            Log.e(TAG, "File completion/checksum verification failed for ${state.finalFile.name}")
                            sendEmptyResponse(output, 400, "Verification Failed")
                        }
                    }
                    path == "/api/localsend/v2/cancel" && method == "POST" -> {
                        val queryMap = parseQuery(queryStr)
                        queryMap["sessionId"]?.let { sessionManager.cancelSession(it) }
                        sendEmptyResponse(output, 200, "OK")
                    }
                    path.startsWith("/api/v2/clipboard/payload/") && method == "GET" -> {
                        val payloadId = path.removePrefix("/api/v2/clipboard/payload/")
                        val queryMap = parseQuery(queryStr)
                        val token = queryMap["token"] ?: ""
                        val data = clipboardManager?.getCachedPayload(payloadId, token)
                        if (data != null) {
                            sendBinaryResponse(output, 200, data, "application/octet-stream")
                        } else {
                            sendEmptyResponse(output, 404, "Not Found")
                        }
                    }
                    path == "/api/v2/clipboard" && method == "POST" -> {
                        val contentLength = headers["content-length"]?.toIntOrNull() ?: 0
                        val bodyBytes = ByteArray(contentLength)
                        var read = 0
                        while (read < contentLength) {
                            val r = input.read(bodyBytes, read, contentLength - read)
                            if (r == -1) break
                            read += r
                        }
                        val body = String(bodyBytes, 0, read, Charsets.UTF_8)
                        val remoteHost = (s.remoteSocketAddress as? InetSocketAddress)?.address?.hostAddress
                            ?: s.inetAddress?.hostAddress
                            ?: "127.0.0.1"
                        handleIncomingClipboard(body, remoteHost, output)
                    }
                    else -> {
                        sendEmptyResponse(output, 404, "Not Found")
                    }
                }
            } catch (e: Exception) {
                Log.w(TAG, "Error handling client request", e)
            }
        }
    }

    private fun handleIncomingClipboard(body: String, remoteHost: String, output: OutputStream) {
        try {
            val jsonElement = json.parseToJsonElement(body)
            val type = jsonElement.jsonObject["type"]?.jsonPrimitive?.content ?: "clipboard.inline"
            when (type) {
                "clipboard.inline" -> {
                    val packet = json.decodeFromJsonElement<ClipboardInlinePacket>(jsonElement)
                    val hash = packet.payload.sha256
                    if (clipboardManager?.isRecentHash(hash) == true) {
                        Log.d(TAG, "Echo suppressed in HttpReceiver: matches last pasted inline hash ($hash)")
                        sendEmptyResponse(output, 200, "OK")
                        return
                    }
                    clipboardManager?.setLastPastedHash(hash)
                    val content = packet.payload.content
                    if (!packet.payload.mimeType.startsWith("image/") && !content.startsWith("data:")) {
                        clipboardManager?.pasteToSystem(content)
                    }
                    onClipboardReceived?.invoke(content, remoteHost)
                    sendEmptyResponse(output, 200, "OK")
                }
                "clipboard.descriptor" -> {
                    val packet = json.decodeFromJsonElement<ClipboardDescriptorPacket>(jsonElement)
                    val hash = packet.descriptor.sha256
                    if (clipboardManager?.isRecentHash(hash) == true) {
                        Log.d(TAG, "Echo suppressed in HttpReceiver: descriptor hash matches last pasted ($hash)")
                        sendEmptyResponse(output, 200, "OK")
                        return
                    }
                    clipboardManager?.setLastPastedHash(hash)
                    packet.descriptor.downloadUrl?.let { rawUrl ->
                        scope.launch {
                            try {
                                var url = rawUrl
                                val defaultPort = if (remoteHost == "127.0.0.1" || remoteHost == "localhost" || remoteHost == "::1") port else 53317
                                if (url.contains("://127.0.0.1:")) {
                                    url = url.replace("://127.0.0.1:", "://$remoteHost:")
                                } else if (url.contains("://localhost:")) {
                                    url = url.replace("://localhost:", "://$remoteHost:")
                                } else if (url.contains("://0.0.0.0:")) {
                                    url = url.replace("://0.0.0.0:", "://$remoteHost:")
                                } else if (url.startsWith("/")) {
                                    url = "http://$remoteHost:$defaultPort$url"
                                }

                                val client = okhttp3.OkHttpClient.Builder()
                                    .connectTimeout(5, java.util.concurrent.TimeUnit.SECONDS)
                                    .readTimeout(10, java.util.concurrent.TimeUnit.SECONDS)
                                    .build()
                                val request = okhttp3.Request.Builder().url(url).build()
                                client.newCall(request).execute().use { response ->
                                    if (response.isSuccessful) {
                                        val bytes = response.body?.bytes()
                                        if (bytes != null) {
                                            val md = MessageDigest.getInstance("SHA-256")
                                            val computedHash = md.digest(bytes).joinToString("") { "%02x".format(it) }
                                            if (computedHash.equals(hash, ignoreCase = true)) {
                                                clipboardManager?.setLastPastedHash(computedHash)
                                                if (packet.descriptor.primaryMime.startsWith("text/")) {
                                                    val text = String(bytes, Charsets.UTF_8)
                                                    clipboardManager?.pasteToSystem(text)
                                                    onClipboardReceived?.invoke(text, remoteHost)
                                                } else {
                                                    onClipboardReceived?.invoke("binary:${packet.descriptor.primaryMime}:${bytes.size}", remoteHost)
                                                }
                                            } else {
                                                Log.w(TAG, "Checksum mismatch for descriptor payload: $computedHash vs $hash")
                                            }
                                        }
                                    }
                                }
                            } catch (e: Exception) {
                                Log.w(TAG, "Failed to download clipboard descriptor payload: ${e.message}")
                            }
                        }
                    }
                    sendEmptyResponse(output, 200, "OK")
                }
                "clipboard.clear" -> {
                    clipboardManager?.clearSystemClipboard()
                    sendEmptyResponse(output, 200, "OK")
                }
                else -> {
                    val packet = json.decodeFromString<ClipboardInlinePacket>(body)
                    val hash = packet.payload.sha256
                    if (clipboardManager?.isRecentHash(hash) == true) {
                        Log.d(TAG, "Echo suppressed in HttpReceiver fallback: matches last pasted ($hash)")
                        sendEmptyResponse(output, 200, "OK")
                        return
                    }
                    clipboardManager?.setLastPastedHash(hash)
                    val content = packet.payload.content
                    if (!packet.payload.mimeType.startsWith("image/") && !content.startsWith("data:")) {
                        clipboardManager?.pasteToSystem(content)
                    }
                    onClipboardReceived?.invoke(content, remoteHost)
                    sendEmptyResponse(output, 200, "OK")
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "Failed to parse incoming clipboard packet", e)
            sendEmptyResponse(output, 400, "Bad Request")
        }
    }

    private fun readHttpLine(input: InputStream): String? {
        val baos = ByteArrayOutputStream()
        var b: Int
        while (input.read().also { b = it } != -1) {
            if (b == '\n'.code) {
                val bytes = baos.toByteArray()
                val len = if (bytes.isNotEmpty() && bytes.last() == '\r'.code.toByte()) bytes.size - 1 else bytes.size
                return String(bytes, 0, len, Charsets.US_ASCII)
            }
            baos.write(b)
        }
        return if (baos.size() > 0) String(baos.toByteArray(), Charsets.US_ASCII) else null
    }

    private fun sendBinaryResponse(output: OutputStream, statusCode: Int, data: ByteArray, contentType: String) {
        val header = "HTTP/1.1 $statusCode OK\r\n" +
                "Content-Type: $contentType\r\n" +
                "Content-Length: ${data.size}\r\n" +
                "Access-Control-Allow-Origin: *\r\n" +
                "Connection: close\r\n\r\n"
        output.write(header.toByteArray(Charsets.US_ASCII))
        output.write(data)
        output.flush()
    }

    private fun sendJsonResponse(output: OutputStream, statusCode: Int, jsonStr: String) {
        val bytes = jsonStr.toByteArray(Charsets.UTF_8)
        val header = "HTTP/1.1 $statusCode OK\r\n" +
                "Content-Type: application/json; charset=utf-8\r\n" +
                "Content-Length: ${bytes.size}\r\n" +
                "Access-Control-Allow-Origin: *\r\n" +
                "Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n" +
                "Access-Control-Allow-Headers: Content-Type, Authorization\r\n" +
                "Connection: close\r\n\r\n"
        output.write(header.toByteArray(Charsets.US_ASCII))
        output.write(bytes)
        output.flush()
    }

    private fun sendEmptyResponse(output: OutputStream, statusCode: Int, statusText: String) {
        val header = "HTTP/1.1 $statusCode $statusText\r\n" +
                "Content-Length: 0\r\n" +
                "Access-Control-Allow-Origin: *\r\n" +
                "Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n" +
                "Access-Control-Allow-Headers: Content-Type, Authorization\r\n" +
                "Connection: close\r\n\r\n"
        output.write(header.toByteArray(Charsets.US_ASCII))
        output.flush()
    }

    private fun parseQuery(query: String?): Map<String, String> {
        if (query.isNullOrEmpty()) return emptyMap()
        return query.split("&").mapNotNull { part ->
            val eqIdx = part.indexOf('=')
            if (eqIdx != -1) {
                val key = URLDecoder.decode(part.substring(0, eqIdx), "UTF-8")
                val value = URLDecoder.decode(part.substring(eqIdx + 1), "UTF-8")
                key to value
            } else {
                null
            }
        }.toMap()
    }
}
