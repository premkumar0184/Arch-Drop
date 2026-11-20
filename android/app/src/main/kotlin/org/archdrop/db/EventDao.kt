package org.archdrop.db

import androidx.room.Dao
import androidx.room.Insert
import androidx.room.OnConflictStrategy
import androidx.room.Query
import kotlinx.coroutines.flow.Flow

@Dao
interface EventDao {
    @Insert(onConflict = OnConflictStrategy.IGNORE)
    suspend fun insertEvents(events: List<EventEntity>): List<Long>

    @Insert(onConflict = OnConflictStrategy.IGNORE)
    suspend fun insertEvent(event: EventEntity): Long

    @Query("SELECT COALESCE(MAX(sequenceNumber), 0) + 1 FROM events WHERE originDeviceId = :originDeviceId")
    suspend fun getNextSequenceNumber(originDeviceId: String): Long

    @Query("SELECT * FROM events WHERE originDeviceId = :originId AND sequenceNumber > :afterSeq ORDER BY sequenceNumber ASC LIMIT :limit")
    suspend fun getMissingEvents(originId: String, afterSeq: Long, limit: Int = 50): List<EventEntity>

    @Query("SELECT * FROM events WHERE originDeviceId = :originId ORDER BY sequenceNumber ASC")
    suspend fun getAllEventsForOrigin(originId: String): List<EventEntity>

    @Query("SELECT * FROM events ORDER BY timestampMs ASC")
    suspend fun getAllEvents(): List<EventEntity>

    @Query("SELECT * FROM events WHERE eventType LIKE 'clipboard.%' ORDER BY timestampMs DESC LIMIT 1")
    fun observeLatestClipboardEvent(): Flow<EventEntity?>

    @Query("SELECT * FROM events WHERE eventType LIKE 'transfer.%' ORDER BY timestampMs DESC LIMIT :limit")
    fun observeTransferEvents(limit: Int = 100): Flow<List<EventEntity>>

    @Query("SELECT * FROM events ORDER BY timestampMs DESC")
    fun observeAllEvents(): Flow<List<EventEntity>>

    @Query("SELECT * FROM events ORDER BY timestampMs DESC LIMIT :limit")
    fun observeRecentEvents(limit: Int = 50): Flow<List<EventEntity>>

    @Query("DELETE FROM events WHERE timestampMs < :cutoffMs AND ackStatus = 3")
    suspend fun pruneAcknowledgedEvents(cutoffMs: Long): Int

    @Query("DELETE FROM events WHERE originDeviceId = :originId AND sequenceNumber = :seq")
    suspend fun deleteEvent(originId: String, seq: Long): Int

    @Query("SELECT COUNT(*) FROM events")
    suspend fun countEvents(): Int
}
