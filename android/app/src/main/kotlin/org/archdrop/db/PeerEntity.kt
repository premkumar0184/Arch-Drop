package org.archdrop.db

import androidx.room.Entity
import androidx.room.PrimaryKey

/**
 * Persisted peer record populated via out-of-band QR code pairing or mutual discovery.
 */
@Entity(tableName = "peers")
data class PeerEntity(
    @PrimaryKey
    val meshKey: String, // 64 hex characters (32-byte Ed25519 public key)
    val alias: String,
    val fingerprint: String, // 64 hex characters (32-byte SHA-256 certificate digest)
    val port: Int = 53317,
    val pairedAtMs: Long = System.currentTimeMillis(),
    val lastSeenMs: Long = System.currentTimeMillis(),
    val isTrusted: Boolean = true
)
