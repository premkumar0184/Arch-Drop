package org.archdrop.transfer

import org.archdrop.transfer.models.FileDto
import org.archdrop.transfer.models.PrepareUploadResponseDto
import org.archdrop.transfer.models.RegisterDto
import java.io.File
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

class TransferSessionManager(private val downloadDir: File) {

    data class FileTransferState(
        val dto: FileDto,
        val token: String,
        val tempFile: File,
        val finalFile: File,
        var bytesReceived: Long = 0,
        var isCompleted: Boolean = false
    )

    data class ActiveSession(
        val sessionId: String,
        val senderInfo: RegisterDto,
        val files: ConcurrentHashMap<String, FileTransferState>,
        val createdAt: Long = System.currentTimeMillis()
    )

    private val sessions = ConcurrentHashMap<String, ActiveSession>()

    fun createSession(
        senderInfo: RegisterDto,
        files: Map<String, FileDto>
    ): PrepareUploadResponseDto {
        val sessionId = UUID.randomUUID().toString()
        val sessionFiles = ConcurrentHashMap<String, FileTransferState>()
        val responseFiles = mutableMapOf<String, String>()

        if (!downloadDir.exists()) {
            downloadDir.mkdirs()
        }

        for ((fileId, fileDto) in files) {
            val token = UUID.randomUUID().toString()
            val rawName = File(fileDto.fileName).name
            val sanitized = rawName.replace(Regex("[/\\\\?%*:|\"<>\u0000]"), "_")
            val safeName = if (sanitized.isEmpty() || sanitized == "." || sanitized == "..") "archdrop_received_file" else sanitized
            val finalFile = resolveUniqueFile(downloadDir, safeName)
            val tempFile = File(downloadDir, ".${finalFile.name}.part.${sessionId.take(8)}")

            responseFiles[fileId] = token
            sessionFiles[fileId] = FileTransferState(
                dto = fileDto,
                token = token,
                tempFile = tempFile,
                finalFile = finalFile
            )
        }

        sessions[sessionId] = ActiveSession(
            sessionId = sessionId,
            senderInfo = senderInfo,
            files = sessionFiles
        )

        return PrepareUploadResponseDto(
            sessionId = sessionId,
            files = responseFiles
        )
    }

    fun getFileState(sessionId: String, fileId: String, token: String): FileTransferState? {
        val session = sessions[sessionId] ?: return null
        val state = session.files[fileId] ?: return null
        return if (state.token == token) state else null
    }

    fun completeFile(sessionId: String, fileId: String, receivedBytes: Long, calculatedSha256: String): Boolean {
        val session = sessions[sessionId] ?: return false
        val state = session.files[fileId] ?: return false

        if (state.dto.size > 0 && receivedBytes != state.dto.size) {
            state.tempFile.delete()
            return false
        }

        state.dto.sha256?.let { expected ->
            if (!expected.equals(calculatedSha256, ignoreCase = true)) {
                state.tempFile.delete()
                return false
            }
        }

        if (state.tempFile.renameTo(state.finalFile)) {
            state.bytesReceived = receivedBytes
            state.isCompleted = true
            return true
        }

        return false
    }

    fun cancelSession(sessionId: String) {
        sessions.remove(sessionId)?.let { session ->
            session.files.values.forEach { state ->
                if (state.tempFile.exists()) {
                    state.tempFile.delete()
                }
            }
        }
    }

    private fun resolveUniqueFile(dir: File, fileName: String): File {
        var candidate = File(dir, fileName)
        if (!candidate.exists()) return candidate

        val nameWithoutExt = candidate.nameWithoutExtension
        val ext = if (candidate.extension.isNotEmpty()) ".${candidate.extension}" else ""
        var count = 1
        while (candidate.exists()) {
            candidate = File(dir, "$nameWithoutExt ($count)$ext")
            count++
        }
        return candidate
    }
}
