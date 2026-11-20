package org.archdrop.mesh

import android.content.Context
import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.archdrop.db.ArchDropDatabase
import org.archdrop.db.EventEntity
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.security.MessageDigest
import java.util.UUID

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class SyncServiceMeshTest {

    private lateinit var db: ArchDropDatabase

    @Before
    fun setUp() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        db = Room.inMemoryDatabaseBuilder(context, ArchDropDatabase::class.java)
            .allowMainThreadQueries()
            .build()
    }

    @After
    fun tearDown() {
        db.close()
    }

    private fun sha256(content: String): String {
        return MessageDigest.getInstance("SHA-256")
            .digest(content.toByteArray(Charsets.UTF_8))
            .joinToString("") { "%02x".format(it) }
    }

    @Test
    fun testEventLedgerIngestionAndOutboxLifecycle() {
        runBlocking {
            val origin = "ANDROID_TEST_NODE_1"
            val targetPeer = "DESKTOP_PEER_HEX_ABC"

            // 1. Allocate sequence and record event
            val seq = db.eventDao().getNextSequenceNumber(origin)
            assertEquals(1L, seq)

            val payload = "{\"text\":\"Synced clipboard message from Android\"}"
            val hash = sha256(payload)
            val event = EventEntity(
                originDeviceId = origin,
                sequenceNumber = seq,
                eventId = UUID.randomUUID().toString(),
                timestampMs = 1731062400000L,
                eventType = "clipboard.text",
                payload = payload,
                payloadHash = hash,
                ackStatus = 0
            )

            db.eventDao().insertEvent(event)
            db.vectorClockDao().advanceHighWaterMark(origin, seq, 1731062400000L, "COMMITTED")

            // Enqueue into outbox for remote desktop
            val outbox = org.archdrop.db.OutboxEntity(
                originDeviceId = origin,
                sequenceNumber = seq,
                targetPeerId = targetPeer,
                status = "PENDING"
            )
            val queueId = db.outboxDao().enqueueOne(outbox)
            assertTrue(queueId > 0)

            // Verify count
            assertEquals(1, db.eventDao().countEvents())
            assertEquals(1, db.outboxDao().countOutbox())
            assertEquals(1L, db.vectorClockDao().getMaxSequence(origin))

            // 2. Acknowledge outbox delivery from remote peer
            val acked = db.outboxDao().acknowledgeOutbox(targetPeer, origin, seq)
            assertEquals(1, acked)
            assertEquals(0, db.outboxDao().countOutbox())
        }
    }

    @Test
    fun testRemoteBatchIngestionAndHighWaterMarkAdvancement() {
        runBlocking {
            val desktopOrigin = "DESKTOP_NODE_99"
            val events = (1..5).map { i ->
                val p = "Desktop notification event $i"
                EventEntity(
                    originDeviceId = desktopOrigin,
                    sequenceNumber = i.toLong(),
                    eventId = "evt-desktop-$i",
                    timestampMs = 1731062400000L + (i * 100),
                    eventType = "transfer.notification",
                    payload = p,
                    payloadHash = sha256(p),
                    ackStatus = 1
                )
            }

            // Ingest batch
            db.eventDao().insertEvents(events)
            val maxSeq = events.maxOf { it.sequenceNumber }
            db.vectorClockDao().advanceHighWaterMark(desktopOrigin, maxSeq, System.currentTimeMillis(), "SYNCED")

            assertEquals(5, db.eventDao().countEvents())
            assertEquals(5L, db.vectorClockDao().getMaxSequence(desktopOrigin))

            // Duplicate batch re-insertion must be ignored (idempotent deduplication)
            val reinserted = db.eventDao().insertEvents(events)
            assertTrue("Duplicate events must return -1 in Room ignore strategy", reinserted.all { it == -1L })
            assertEquals(5, db.eventDao().countEvents())
        }
    }

    @Test
    fun testDeterministicLwwClipboardResolution() {
        val ts = 1731062500000L
        val eventA = EventEntity(
            originDeviceId = "AAA_ORIGIN",
            sequenceNumber = 1L,
            eventId = "id-A",
            timestampMs = ts,
            eventType = "clipboard.text",
            payload = "A text",
            payloadHash = sha256("A text")
        )
        val eventB = EventEntity(
            originDeviceId = "BBB_ORIGIN",
            sequenceNumber = 1L,
            eventId = "id-B",
            timestampMs = ts,
            eventType = "clipboard.text",
            payload = "B text",
            payloadHash = sha256("B text")
        )

        // Compare tie-breaker: timestamp first, then uppercase originDeviceId
        val candidates = listOf(eventA, eventB)
        val winner = candidates.maxWithOrNull { a, b ->
            val timeComp = a.timestampMs.compareTo(b.timestampMs)
            if (timeComp != 0) timeComp else a.originDeviceId.uppercase().compareTo(b.originDeviceId.uppercase())
        }

        assertNotNull(winner)
        assertEquals("BBB_ORIGIN", winner!!.originDeviceId)
        assertEquals("B text", winner.payload)
    }
}
