package org.archdrop.db

import androidx.room.Dao
import androidx.room.Insert
import androidx.room.Query

@Dao
interface OutboxDao {
    @Insert
    suspend fun enqueue(entries: List<OutboxEntity>): List<Long>

    @Insert
    suspend fun enqueueOne(entry: OutboxEntity): Long

    @Query("""
        SELECT * FROM outbox_queue 
        WHERE targetPeerId = :peerId AND status = 'PENDING' AND nextRetryMs <= :nowMs 
        ORDER BY queueId ASC 
        LIMIT :limit
    """)
    suspend fun getPendingOutbox(peerId: String, nowMs: Long, limit: Int = 50): List<OutboxEntity>

    @Query("""
        DELETE FROM outbox_queue 
        WHERE targetPeerId = :peerId AND originDeviceId = :originId AND sequenceNumber <= :ackSeq
    """)
    suspend fun acknowledgeOutbox(peerId: String, originId: String, ackSeq: Long): Int

    @Query("""
        UPDATE outbox_queue 
        SET retryCount = retryCount + 1, 
            nextRetryMs = :nextRetryMs, 
            status = :newStatus 
        WHERE queueId = :queueId
    """)
    suspend fun recordAttempt(queueId: Long, nextRetryMs: Long, newStatus: String): Int

    @Query("SELECT COUNT(*) FROM outbox_queue WHERE originDeviceId = :originId AND sequenceNumber = :seq")
    suspend fun countByEvent(originId: String, seq: Long): Int

    @Query("SELECT COUNT(*) FROM outbox_queue")
    suspend fun countOutbox(): Int
}
