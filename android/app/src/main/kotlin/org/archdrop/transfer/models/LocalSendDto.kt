package org.archdrop.transfer.models

import kotlinx.serialization.Serializable

@Serializable
data class RegisterDto(
    val alias: String,
    val version: String,
    val deviceModel: String? = null,
    val deviceType: String? = null,
    val fingerprint: String,
    val port: Int,
    val protocol: String,
    val download: Boolean = true
)

@Serializable
data class InfoResponseDto(
    val alias: String,
    val version: String,
    val deviceModel: String? = null,
    val deviceType: String? = null,
    val fingerprint: String,
    val download: Boolean = true
)

@Serializable
data class FileDto(
    val id: String,
    val fileName: String,
    val size: Long,
    val fileType: String,
    val sha256: String? = null,
    val preview: String? = null
)

@Serializable
data class PrepareUploadRequestDto(
    val info: RegisterDto,
    val files: Map<String, FileDto>
)

@Serializable
data class PrepareUploadResponseDto(
    val sessionId: String,
    val files: Map<String, String>
)
