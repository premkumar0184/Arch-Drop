package org.archdrop.db

import androidx.room.Entity
import androidx.room.ForeignKey
import androidx.room.Index
import androidx.room.PrimaryKey
import kotlinx.serialization.Serializable

/**
 * Durable outbox queue tracking local events pending delivery and acknowledgment by remote peers.
 */
@Serializable
@Entity(
    tableName = "outbox_queue",
    foreignKeys = [
        ForeignKey(
            entity = EventEntity::class,
            parentColumns = ["originDeviceId", "sequenceNumber"],
            childColumns = ["originDeviceId", "sequenceNumber"],
            onDelete = ForeignKey.CASCADE
        )
    ],
    indices = [
        Index(value = ["originDeviceId", "sequenceNumber"]),
        Index(value = ["targetPeerId", "status", "nextRetryMs"])
    ]
)
data class OutboxEntity(
    @PrimaryKey(autoGenerate = true)
    val queueId: Long = 0,
    val originDeviceId: String,
    val sequenceNumber: Long,
    val targetPeerId: String,
    val retryCount: Int = 0,
    val lastAttemptMs: Long = 0,
    val nextRetryMs: Long = 0,
    val status: String = "PENDING"
)
