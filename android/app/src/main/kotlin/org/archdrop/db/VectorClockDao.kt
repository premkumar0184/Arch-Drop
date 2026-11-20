package org.archdrop.db

import androidx.room.Dao
import androidx.room.Query
import androidx.room.Transaction
import androidx.room.Upsert
import kotlinx.coroutines.flow.Flow

@Dao
interface VectorClockDao {
    @Query("SELECT maxSequenceNumber FROM vector_clock WHERE peerDeviceId = :peerId")
    suspend fun getMaxSequence(peerId: String): Long?

    @Query("SELECT * FROM vector_clock ORDER BY peerDeviceId ASC")
    suspend fun getAllVectors(): List<VectorClockEntity>

    @Query("SELECT * FROM vector_clock ORDER BY peerDeviceId ASC")
    fun observeAllVectors(): Flow<List<VectorClockEntity>>

    @Upsert
    suspend fun upsertVector(vector: VectorClockEntity)

    @Query("""
        UPDATE vector_clock 
        SET maxSequenceNumber = MAX(maxSequenceNumber, :maxSeq), 
            lastSyncedAt = :timestampMs, 
            syncState = :state 
        WHERE peerDeviceId = :peerId
    """)
    suspend fun updateHighWaterMark(peerId: String, maxSeq: Long, timestampMs: Long, state: String = "SYNCED"): Int

    @Transaction
    suspend fun advanceHighWaterMark(peerId: String, maxSeq: Long, timestampMs: Long, state: String = "SYNCED"): Int {
        val updated = updateHighWaterMark(peerId, maxSeq, timestampMs, state)
        if (updated == 0) {
            upsertVector(VectorClockEntity(peerId, maxSeq, timestampMs, state))
            return 1
        }
        return updated
    }
}

