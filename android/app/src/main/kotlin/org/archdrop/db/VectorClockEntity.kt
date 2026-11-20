package org.archdrop.db

import androidx.room.Entity
import androidx.room.PrimaryKey
import kotlinx.serialization.Serializable

/**
 * Vector clock tracking contiguous high-water marks for each known node.
 */
@Serializable
@Entity(tableName = "vector_clock")
data class VectorClockEntity(
    @PrimaryKey
    val peerDeviceId: String,
    val maxSequenceNumber: Long = 0,
    val lastSyncedAt: Long = 0,
    val syncState: String = "IDLE"
)
