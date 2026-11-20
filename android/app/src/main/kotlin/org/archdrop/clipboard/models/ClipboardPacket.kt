package org.archdrop.clipboard.models

import kotlinx.serialization.Serializable

@Serializable
data class ClipboardInlinePayload(
    val mimeType: String,
    val content: String,
    val sha256: String,
    val sizeBytes: Int
)

@Serializable
data class ClipboardDescriptorPayload(
    val payloadId: String,
    val primaryMime: String,
    val availableMimes: List<String>,
    val sizeBytes: Int,
    val sha256: String,
    val ephemeralToken: String,
    val downloadUrl: String? = null
)

@Serializable
data class ClipboardInlinePacket(
    val type: String = "clipboard.inline",
    val version: Int = 2,
    val deviceId: String,
    val timestamp: Long,
    val ttlSeconds: Long = 120,
    val payload: ClipboardInlinePayload
)

@Serializable
data class ClipboardDescriptorPacket(
    val type: String = "clipboard.descriptor",
    val version: Int = 2,
    val deviceId: String,
    val timestamp: Long,
    val ttlSeconds: Long = 120,
    val descriptor: ClipboardDescriptorPayload
)

@Serializable
data class ClipboardClearPacket(
    val type: String = "clipboard.clear",
    val version: Int = 2,
    val deviceId: String,
    val timestamp: Long,
    val reason: String
)
