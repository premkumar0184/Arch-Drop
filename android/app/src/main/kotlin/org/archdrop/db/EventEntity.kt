package org.archdrop.db

import androidx.room.Entity
import androidx.room.Index
import kotlinx.serialization.Serializable

/**
 * Deterministic append-only state ledger event record mirroring the desktop SQLite schema.
 *
 * Primary compound key: `(originDeviceId, sequenceNumber)` ensuring monotonic, collision-free sequencing.
 */
@Serializable
@Entity(
    tableName = "events",
    primaryKeys = ["originDeviceId", "sequenceNumber"],
    indices = [
        Index(value = ["eventId"], unique = true),
        Index(value = ["originDeviceId", "sequenceNumber"]),
        Index(value = ["timestampMs"]),
        Index(value = ["eventType", "timestampMs"])
    ]
)
data class EventEntity(
    val originDeviceId: String,
    val sequenceNumber: Long,
    val eventId: String,
    val timestampMs: Long,
    val eventType: String,
    val payload: String,
    val payloadHash: String,
    val ackStatus: Int = 0,
    val createdAt: Long = System.currentTimeMillis()
)
