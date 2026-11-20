package org.archdrop.db

import android.content.Context
import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
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
class RoomLedgerTest {

    private lateinit var db: ArchDropDatabase
    private lateinit var eventDao: EventDao
    private lateinit var vectorClockDao: VectorClockDao
    private lateinit var outboxDao: OutboxDao

    @Before
    fun setUp() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        db = Room.inMemoryDatabaseBuilder(context, ArchDropDatabase::class.java)
            .allowMainThreadQueries()
            .build()
        eventDao = db.eventDao()
        vectorClockDao = db.vectorClockDao()
        outboxDao = db.outboxDao()
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
    fun testEventInsertionAndCompoundKeyUniqueness() = runBlocking {
        val payload1 = "clipboard text 1"
        val hash1 = sha256(payload1)
        val event1 = EventEntity(
            originDeviceId = "NODE_A",
            sequenceNumber = 1,
            eventId = UUID.randomUUID().toString(),
            timestampMs = 1000L,
            eventType = "clipboard.text",
            payload = payload1,
            payloadHash = hash1,
            ackStatus = 1
        )

        val rowId1 = eventDao.insertEvent(event1)
        assertTrue(rowId1 > 0)
        assertEquals(1, eventDao.countEvents())

        // Duplicate insert on identical compound key (NODE_A, 1) should be ignored
        val rowIdDuplicate = eventDao.insertEvent(event1)
        assertEquals(-1L, rowIdDuplicate)
        assertEquals(1, eventDao.countEvents())

        // Same origin, different sequence number: allowed
        val event2 = event1.copy(sequenceNumber = 2, eventId = UUID.randomUUID().toString())
        val rowId2 = eventDao.insertEvent(event2)
        assertTrue(rowId2 > 0)
        assertEquals(2, eventDao.countEvents())

        // Different origin, same sequence number (NODE_B, 1): allowed due to compound key
        val eventOtherOrigin = event1.copy(originDeviceId = "NODE_B", sequenceNumber = 1, eventId = UUID.randomUUID().toString())
        val rowIdOther = eventDao.insertEvent(eventOtherOrigin)
        assertTrue(rowIdOther > 0)
        assertEquals(3, eventDao.countEvents())
    }

    @Test
    fun testMonotonicSequenceAllocation() = runBlocking {
        assertEquals(1L, eventDao.getNextSequenceNumber("DEVICE_MOBILE_1"))

        val event = EventEntity(
            originDeviceId = "DEVICE_MOBILE_1",
            sequenceNumber = 1L,
            eventId = UUID.randomUUID().toString(),
            timestampMs = System.currentTimeMillis(),
            eventType = "clipboard.inline",
            payload = "{\"content\":\"hello\"}",
            payloadHash = sha256("{\"content\":\"hello\"}"),
            ackStatus = 1
        )
        eventDao.insertEvent(event)

        assertEquals(2L, eventDao.getNextSequenceNumber("DEVICE_MOBILE_1"))

        val event2 = event.copy(sequenceNumber = 2L, eventId = UUID.randomUUID().toString())
        eventDao.insertEvent(event2)

        assertEquals(3L, eventDao.getNextSequenceNumber("DEVICE_MOBILE_1"))

        // Another device origin starts at 1
        assertEquals(1L, eventDao.getNextSequenceNumber("DEVICE_OTHER_2"))
    }

    @Test
    fun testMissingEventsOrdering() = runBlocking {
        for (i in 1..5) {
            val event = EventEntity(
                originDeviceId = "ORIGIN_X",
                sequenceNumber = i.toLong(),
                eventId = UUID.randomUUID().toString(),
                timestampMs = 1000L + i,
                eventType = "transfer.status",
                payload = "status $i",
                payloadHash = sha256("status $i"),
                ackStatus = 1
            )
            eventDao.insertEvent(event)
        }

        val missing = eventDao.getMissingEvents("ORIGIN_X", afterSeq = 2L, limit = 10)
        assertEquals(3, missing.size)
        assertEquals(3L, missing[0].sequenceNumber)
        assertEquals(4L, missing[1].sequenceNumber)
        assertEquals(5L, missing[2].sequenceNumber)
    }

    @Test
    fun testVectorClockUpsertAndHighWaterMarkAdvancement() = runBlocking {
        assertNull(vectorClockDao.getMaxSequence("PEER_LAPTOP"))

        val initialClock = VectorClockEntity(
            peerDeviceId = "PEER_LAPTOP",
            maxSequenceNumber = 5L,
            lastSyncedAt = 1000L,
            syncState = "COMMITTED"
        )
        vectorClockDao.upsertVector(initialClock)
        assertEquals(5L, vectorClockDao.getMaxSequence("PEER_LAPTOP"))

        // Advance to sequence 12
        vectorClockDao.advanceHighWaterMark("PEER_LAPTOP", maxSeq = 12L, timestampMs = 2000L, state = "SYNCED")
        assertEquals(12L, vectorClockDao.getMaxSequence("PEER_LAPTOP"))

        // Attempting to advance with an older sequence (8) should NOT regress high-water mark
        vectorClockDao.advanceHighWaterMark("PEER_LAPTOP", maxSeq = 8L, timestampMs = 3000L, state = "SYNCED")
        assertEquals(12L, vectorClockDao.getMaxSequence("PEER_LAPTOP"))
    }

    @Test
    fun testVectorClockAdvanceHighWaterMarkUpsertsNewPeer() = runBlocking {
        // Peer does not exist yet in vector clock
        assertNull(vectorClockDao.getMaxSequence("NEW_PEER"))

        // Direct advance without prior upsertVector must upsert the record
        val rows = vectorClockDao.advanceHighWaterMark("NEW_PEER", maxSeq = 42L, timestampMs = 5000L, state = "SYNCED")
        assertEquals(1, rows)
        assertEquals(42L, vectorClockDao.getMaxSequence("NEW_PEER"))

        // Advancing to higher sequence updates max sequence
        vectorClockDao.advanceHighWaterMark("NEW_PEER", maxSeq = 50L, timestampMs = 6000L, state = "SYNCED")
        assertEquals(50L, vectorClockDao.getMaxSequence("NEW_PEER"))

        // Advancing with lower sequence does not regress
        vectorClockDao.advanceHighWaterMark("NEW_PEER", maxSeq = 30L, timestampMs = 7000L, state = "SYNCED")
        assertEquals(50L, vectorClockDao.getMaxSequence("NEW_PEER"))
    }

    @Test
    fun testOutboxCascadeDeletionOnEventDelete() = runBlocking {
        val event = EventEntity(
            originDeviceId = "LOCAL_PHONE",
            sequenceNumber = 42L,
            eventId = UUID.randomUUID().toString(),
            timestampMs = 5000L,
            eventType = "clipboard.text",
            payload = "cascade test",
            payloadHash = sha256("cascade test"),
            ackStatus = 1
        )
        eventDao.insertEvent(event)

        val outbox = OutboxEntity(
            originDeviceId = "LOCAL_PHONE",
            sequenceNumber = 42L,
            targetPeerId = "REMOTE_LAPTOP",
            retryCount = 0,
            lastAttemptMs = 0L,
            nextRetryMs = 1000L,
            status = "PENDING"
        )
        outboxDao.enqueueOne(outbox)

        assertEquals(1, outboxDao.countByEvent("LOCAL_PHONE", 42L))
        assertEquals(1, outboxDao.countOutbox())

        // Deleting the parent event must cascade delete the associated outbox row
        eventDao.deleteEvent("LOCAL_PHONE", 42L)

        assertEquals(0, eventDao.countEvents())
        assertEquals(0, outboxDao.countByEvent("LOCAL_PHONE", 42L))
        assertEquals(0, outboxDao.countOutbox())
    }

    @Test
    fun testDeterministicLwwArbitrationOnAndroid() {
        val collisionTime = 1731062400000L
        val eventPhone = EventEntity(
            originDeviceId = "E8C2D3A1B4C5D6",
            sequenceNumber = 1L,
            eventId = "id-1",
            timestampMs = collisionTime,
            eventType = "clipboard.text",
            payload = "phone text",
            payloadHash = sha256("phone text")
        )
        val eventLaptop = EventEntity(
            originDeviceId = "F4A7B1C2D3E4F5",
            sequenceNumber = 1L,
            eventId = "id-2",
            timestampMs = collisionTime,
            eventType = "clipboard.text",
            payload = "laptop text",
            payloadHash = sha256("laptop text")
        )

        val candidates = listOf(eventPhone, eventLaptop)

        // Compare using Last-Write-Wins with uppercase hex device ID tie-breaker
        val winner = candidates.maxWithOrNull { a, b ->
            val timeComp = a.timestampMs.compareTo(b.timestampMs)
            if (timeComp != 0) {
                timeComp
            } else {
                val aUpper = a.originDeviceId.uppercase()
                val bUpper = b.originDeviceId.uppercase()
                val idComp = aUpper.compareTo(bUpper)
                if (idComp != 0) idComp else a.sequenceNumber.compareTo(b.sequenceNumber)
            }
        }

        assertNotNull(winner)
        // "F4A7B1C2D3E4F5" > "E8C2D3A1B4C5D6", so laptop text wins
        assertEquals("F4A7B1C2D3E4F5", winner!!.originDeviceId)
        assertEquals("laptop text", winner.payload)
    }
}
