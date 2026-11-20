package org.archdrop.db

import androidx.room.Dao
import androidx.room.Insert
import androidx.room.OnConflictStrategy
import androidx.room.Query
import androidx.room.Update
import kotlinx.coroutines.flow.Flow

@Dao
interface PeerDao {
    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun insertPeer(peer: PeerEntity): Long

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun insertPeers(peers: List<PeerEntity>)

    @Update
    suspend fun updatePeer(peer: PeerEntity): Int

    @Query("SELECT * FROM peers WHERE meshKey = :meshKey LIMIT 1")
    suspend fun getPeer(meshKey: String): PeerEntity?

    @Query("SELECT * FROM peers ORDER BY lastSeenMs DESC")
    suspend fun getAllPeers(): List<PeerEntity>

    @Query("SELECT * FROM peers ORDER BY lastSeenMs DESC")
    fun observeAllPeers(): Flow<List<PeerEntity>>

    @Query("SELECT * FROM peers WHERE isTrusted = 1 ORDER BY lastSeenMs DESC")
    fun observeTrustedPeers(): Flow<List<PeerEntity>>

    @Query("DELETE FROM peers WHERE meshKey = :meshKey")
    suspend fun deletePeer(meshKey: String): Int

    @Query("SELECT COUNT(*) FROM peers")
    suspend fun countPeers(): Int
}
