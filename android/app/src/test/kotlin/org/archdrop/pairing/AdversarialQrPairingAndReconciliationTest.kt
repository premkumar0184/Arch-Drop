package org.archdrop.pairing

import android.content.Context
import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.MultiFormatWriter
import com.google.zxing.common.BitMatrix
import com.google.zxing.common.HybridBinarizer
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.archdrop.db.ArchDropDatabase
import org.archdrop.db.EventEntity
import org.archdrop.db.PeerEntity
import org.archdrop.ui.pairing.PairingPayload
import org.archdrop.ui.pairing.formatHexBlocks
import org.archdrop.ui.pairing.generateQrBitmap
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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
class AdversarialQrPairingAndReconciliationTest {

    private lateinit var db: ArchDropDatabase
    private val validMeshKey = "A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3D4E5F60718293A4B5C6D7E8F90"
    private val validFingerprint = "11223344556677889900AABBCCDDEEFF11223344556677889900AABBCCDDEEFF"

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

    private fun sha256(input: String): String {
        val md = MessageDigest.getInstance("SHA-256")
        return md.digest(input.toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it) }
    }

    // ========================================================================
    // SECTION 1: QR CODE PARSING & ADVERSARIAL SCHEMA FUZZING
    // ========================================================================

    @Test
    fun testMalformedUriStringsRejection() {
        val malformedUris = listOf(
            "",
            "   ",
            "\n\t\r",
            "http://archdrop.org/pair",
            "https://evil.com/pair?meshKey=$validMeshKey",
            "file:///etc/passwd",
            "archdrop://invalid",
            "archdrop://pair",                       // Missing query params
            "archdrop://pair?",                      // Empty query
            "archdrop://pair?foo=bar",               // Random query
            "archdrop://pair?alias=Desktop",         // Missing meshKey
            "archdrop://pair?meshKey=",              // Empty meshKey
            "archdrop://pair?meshKey=SHORT",         // Short key (5 chars)
            "archdrop://pair?meshKey=${"A".repeat(63)}", // 63 chars (short by 1)
            "archdrop://pair?meshKey=${"A".repeat(65)}", // 65 chars (long by 1)
            "archdrop://pair?meshKey=${"A".repeat(128)}", // 128 chars
            "archdrop://pair?meshKey=${validMeshKey}&port=invalid", // non-int port falls back to 53317
            "archdrop://pair?v=invalid&meshKey=${validMeshKey}",   // non-int v falls back to 1
            "archdrop://pair?meshKey=%ZZ%ZZ",        // Corrupted URI encoding
            "archdrop://pair?alias=%E0%A4%A",        // Broken UTF-8 byte sequence
            "random junk that does not match anything",
            "javascript:alert(1)"
        )

        for (uri in malformedUris) {
            val result = PairingPayload.parse(uri)
            if (uri.contains(validMeshKey) && (uri.contains("port=invalid") || uri.contains("v=invalid"))) {
                // When fallback succeeds, payload must be non-null and have default port/version
                assertNotNull("Expected fallback for query param fallback on $uri", result)
                assertEquals(53317, result!!.port)
                assertEquals(1, result.version)
            } else {
                assertNull("Malformed URI '$uri' should be rejected", result)
            }
        }
    }

    @Test
    fun testMalformedJsonPayloadFuzzing() {
        val malformedJsons = listOf(
            "{",
            "}",
            "{broken json}",
            "{\"meshKey\": \"$validMeshKey\"",        // Unclosed
            "[]",                                    // Array instead of object
            "null",
            "\"just a string\"",
            "12345",
            "{\"version\": 1}",                      // Missing meshKey
            "{\"alias\": \"Test\", \"meshKey\": \"\"}", // Empty meshKey
            "{\"meshKey\": \"SHORT\"}",              // 5 chars
            "{\"meshKey\": \"${"A".repeat(63)}\"}",  // 63 chars
            "{\"meshKey\": \"${"A".repeat(65)}\"}",  // 65 chars
            "{\"meshKey\": null}",                   // Null value
            "{\"meshKey\": 1234567890}",             // Int instead of string
            "{\"meshKey\": [\"$validMeshKey\"]}",    // Array instead of string
            "{\"meshKey\": {\"key\": \"$validMeshKey\"}}" // Object instead of string
        )

        for (json in malformedJsons) {
            val result = PairingPayload.parse(json)
            assertNull("Malformed JSON '$json' must return null", result)
        }
    }

    @Test
    fun testNonHexCharacterKeyAnalysis() {
        // Test key containing 64 non-hex characters (e.g. 'Z')
        val nonHexKey = "Z".repeat(64)
        val uri = "archdrop://pair?meshKey=$nonHexKey"
        val parsed = PairingPayload.parse(uri)

        // Note: The current parser verifies length == 64 and returns uppercase,
        // without enforcing [0-9A-F]. Let's verify whether it crashes or handles it.
        assertNotNull(parsed)
        assertEquals(64, parsed!!.meshKey.length)
        assertEquals(nonHexKey, parsed.meshKey)

        // When inserting into Room DB, verify SQLite handles it safely without crashing
        val peer = PeerEntity(
            meshKey = parsed.meshKey,
            alias = parsed.alias,
            fingerprint = parsed.fingerprint,
            port = parsed.port
        )
        val rowId = runBlocking { db.peerDao().insertPeer(peer) }
        assertTrue("SQLite must accept text key without DB corruption", rowId >= 0)

        val retrieved = runBlocking { db.peerDao().getPeer(nonHexKey) }
        assertNotNull(retrieved)
        assertEquals(nonHexKey, retrieved!!.meshKey)

        // Cleanup
        runBlocking { db.peerDao().deletePeer(nonHexKey) }
    }

    @Test
    fun testNegativeAndExtremePortHandling() {
        // Test negative port in URI
        val negPortUri = "archdrop://pair?meshKey=$validMeshKey&port=-1"
        val parsedNeg = PairingPayload.parse(negPortUri)
        assertNotNull("URI with negative port parses as integer", parsedNeg)
        assertEquals(-1, parsedNeg!!.port)

        // Test out-of-range port in JSON
        val extremePortJson = """{"meshKey": "$validMeshKey", "port": 70000}"""
        val parsedExtreme = PairingPayload.parse(extremePortJson)
        assertNotNull(parsedExtreme)
        assertEquals(70000, parsedExtreme!!.port)

        // Ensure storing extreme port in Room DB doesn't crash or corrupt
        val peer = PeerEntity(
            meshKey = validMeshKey,
            alias = "Extreme Port Peer",
            fingerprint = validFingerprint,
            port = parsedExtreme.port
        )
        runBlocking {
            db.peerDao().insertPeer(peer)
            val p = db.peerDao().getPeer(validMeshKey)
            assertEquals(70000, p!!.port)
            db.peerDao().deletePeer(validMeshKey)
        }
    }

    @Test
    fun testTruncatedAndCorruptedFingerprintHandling() {
        val truncatedFpUri = "archdrop://pair?meshKey=$validMeshKey&fp=SHORTFP"
        val parsed = PairingPayload.parse(truncatedFpUri)
        assertNotNull(parsed)
        assertEquals("SHORTFP", parsed!!.fingerprint)

        val emptyFpJson = """{"meshKey": "$validMeshKey", "fingerprint": ""}"""
        val parsedEmpty = PairingPayload.parse(emptyFpJson)
        assertNotNull(parsedEmpty)
        assertEquals("", parsedEmpty!!.fingerprint)
    }

    @Test
    fun testSpecialCharactersAndSqlInjectionInAlias() {
        val maliciousAliases = listOf(
            "'; DROP TABLE peers; --",
            "\" OR \"\"=\"",
            "<script>alert('xss')</script>",
            "Pixel \uD83D\uDCF1 Pro 8 \uD83D\uDE80", // Emojis
            "الاسم العربي للجهاز",                    // RTL Arabic text
            "Device\nWith\nNewlines\r\nAnd\tTabs",
            "A".repeat(10_000)                       // 10 KB alias string
        )

        for (alias in maliciousAliases) {
            val payload = PairingPayload(
                version = 1,
                alias = alias,
                meshKey = validMeshKey,
                fingerprint = validFingerprint,
                port = 53317
            )
            val uri = payload.toUri()
            val parsed = PairingPayload.parse(uri)
            assertNotNull("Parsing URI with alias '$alias' must succeed", parsed)
            assertEquals(validMeshKey, parsed!!.meshKey)

            // Test Room DB insertion & verification of SQL injection safety
            runBlocking {
                val peer = PeerEntity(
                    meshKey = validMeshKey,
                    alias = parsed.alias,
                    fingerprint = parsed.fingerprint,
                    port = parsed.port
                )
                db.peerDao().insertPeer(peer)
                val stored = db.peerDao().getPeer(validMeshKey)
                assertNotNull("Peer must be stored intact", stored)
                assertEquals(parsed.alias, stored!!.alias)
                db.peerDao().deletePeer(validMeshKey)
            }
        }
    }

    // ========================================================================
    // SECTION 2: QR BITMAP GENERATION & BARCODE SCANNER ROBUSTNESS
    // ========================================================================

    @Test
    fun testQrBitmapGenerationUnderExtremePayloads() {
        // Standard payload
        val normalPayload = PairingPayload(1, "Arch Linux", validMeshKey, validFingerprint, 53317).toUri()
        val normalBmp = generateQrBitmap(normalPayload, 256, 0xFF000000.toInt(), 0xFFFFFFFF.toInt())
        assertNotNull(normalBmp)
        assertEquals(256, normalBmp!!.width)
        assertEquals(256, normalBmp.height)

        // Edge case: empty string
        val emptyBmp = generateQrBitmap("", 256, 0xFF000000.toInt(), 0xFFFFFFFF.toInt())
        assertNull("Empty content must return null", emptyBmp)

        // Edge case: massive 4000-character payload (near QR capacity)
        val massivePayload = "archdrop://pair?meshKey=$validMeshKey&alias=${"X".repeat(1800)}"
        val massiveBmp = generateQrBitmap(massivePayload, 512, 0xFF000000.toInt(), 0xFFFFFFFF.toInt())
        assertNotNull("ZXing should generate QR for within-spec large payloads", massiveBmp)

        // Dimension edge cases
        val tinyBmp = generateQrBitmap(normalPayload, 16, 0xFF000000.toInt(), 0xFFFFFFFF.toInt())
        assertNotNull(tinyBmp)
        // ZXing enforces a minimum module width so QR remains readable (59x59 for this payload)
        assertTrue(tinyBmp!!.width >= 16)
    }

    @Test
    fun testFormatHexBlocksEdgeCases() {
        assertEquals("", formatHexBlocks(""))
        assertEquals("1234", formatHexBlocks("1234"))

        val fullFormatted = formatHexBlocks(validMeshKey)
        val rows = fullFormatted.split("\n")
        assertEquals(2, rows.size)
        rows.forEach { row ->
            val blocks = row.split(" ")
            assertEquals(4, blocks.size)
            blocks.forEach { block ->
                assertEquals(8, block.length)
            }
        }
    }

    // ========================================================================
    // SECTION 3: ROOM DB SCHEMA V2 & REACTIVE CONCURRENCY STRESS
    // ========================================================================

    @Test
    fun testRoomDatabaseSchemaVersionAndTableCreation() {
        // Query SQLite master table to verify all 4 expected tables exist
        val cursor = db.openHelper.readableDatabase.query(
            "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('events', 'vector_clock', 'outbox_queue', 'peers')"
        )
        val tables = mutableSetOf<String>()
        while (cursor.moveToNext()) {
            tables.add(cursor.getString(0))
        }
        cursor.close()

        assertTrue("events table must exist", tables.contains("events"))
        assertTrue("vector_clock table must exist", tables.contains("vector_clock"))
        assertTrue("outbox_queue table must exist", tables.contains("outbox_queue"))
        assertTrue("peers table must exist", tables.contains("peers"))
    }

    @Test
    fun testConcurrentEventDaoInsertsAndReactiveFlows() = runBlocking {
        val eventDao = db.eventDao()
        val totalCoroutines = 15
        val eventsPerCoroutine = 40 // 600 total events
        val originPrefix = "PEER_CONCURRENCY_"

        // Concurrently observe transfer events
        val flowCollectorJob = async(Dispatchers.Default) {
            val initial = eventDao.observeTransferEvents(100).first()
            initial
        }

        // Concurrently insert events from multiple origins
        val insertJobs = (0 until totalCoroutines).map { cIdx ->
            async(Dispatchers.Default) {
                val origin = "$originPrefix$cIdx"
                for (seq in 1..eventsPerCoroutine) {
                    val isTransfer = seq % 2 == 0
                    val eventType = if (isTransfer) "transfer.completed" else "clipboard.text"
                    val payload = "{\"origin\":\"$origin\",\"seq\":$seq}"
                    val event = EventEntity(
                        originDeviceId = origin,
                        sequenceNumber = seq.toLong(),
                        eventId = UUID.randomUUID().toString(),
                        timestampMs = 100000L + seq,
                        eventType = eventType,
                        payload = payload,
                        payloadHash = sha256(payload),
                        ackStatus = 0
                    )
                    eventDao.insertEvent(event)
                }
            }
        }

        insertJobs.awaitAll()
        flowCollectorJob.await()

        // Verify total event count
        val totalCount = eventDao.countEvents()
        assertEquals(totalCoroutines * eventsPerCoroutine, totalCount)

        // Verify reactive flow query returns latest transfers
        val transferEvents = eventDao.observeTransferEvents(50).first()
        assertEquals(50, transferEvents.size)
        transferEvents.forEach { ev ->
            assertTrue("Must only contain transfer events", ev.eventType.startsWith("transfer."))
        }

        // Verify all events flow
        val allEvents = eventDao.observeAllEvents().first()
        assertEquals(totalCount, allEvents.size)
    }

    @Test
    fun testConcurrentPeerDaoOperationsAndReactiveFlows() = runBlocking {
        val peerDao = db.peerDao()
        val totalPeers = 30

        // Concurrently insert and update peers
        val insertJobs = (0 until totalPeers).map { idx ->
            async(Dispatchers.Default) {
                val meshKey = "%064X".format(idx + 1)
                val peer = PeerEntity(
                    meshKey = meshKey,
                    alias = "Peer-$idx",
                    fingerprint = "%064X".format(idx + 100),
                    port = 53317 + idx,
                    pairedAtMs = 1000L + idx,
                    lastSeenMs = 1000L + idx,
                    isTrusted = idx % 2 == 0 // Half trusted, half untrusted
                )
                peerDao.insertPeer(peer)

                // Immediate read-after-write
                val fetched = peerDao.getPeer(meshKey)
                assertNotNull(fetched)
                assertEquals("Peer-$idx", fetched!!.alias)

                // Update lastSeenMs
                val updated = fetched.copy(lastSeenMs = 2000L + idx)
                peerDao.updatePeer(updated)
            }
        }

        insertJobs.awaitAll()

        // Verify peer counts
        assertEquals(totalPeers, peerDao.countPeers())

        val allPeers = peerDao.observeAllPeers().first()
        assertEquals(totalPeers, allPeers.size)

        val trustedPeers = peerDao.observeTrustedPeers().first()
        assertEquals(totalPeers / 2, trustedPeers.size)
        trustedPeers.forEach { p ->
            assertTrue("Only trusted peers in observeTrustedPeers", p.isTrusted)
        }

        // Concurrently delete half the peers
        val deleteJobs = (0 until totalPeers / 2).map { idx ->
            async(Dispatchers.Default) {
                val meshKey = "%064X".format(idx + 1)
                peerDao.deletePeer(meshKey)
            }
        }
        deleteJobs.awaitAll()

        assertEquals(totalPeers - (totalPeers / 2), peerDao.countPeers())
    }

    @Test
    fun testPairingLedgerCommitAtomicity() = runBlocking {
        // Simulate exact transaction from PairingScreen when pairing succeeds
        val peerMeshKey = "B".repeat(64)
        val peerAlias = "Arch Laptop"
        val peerFp = "C".repeat(64)
        val peerPort = 53317
        val now = System.currentTimeMillis()

        // 1. Insert Peer
        val peer = PeerEntity(
            meshKey = peerMeshKey,
            alias = peerAlias,
            fingerprint = peerFp,
            port = peerPort,
            pairedAtMs = now,
            lastSeenMs = now,
            isTrusted = true
        )
        db.peerDao().insertPeer(peer)

        // 2. Commit peer.paired causal event
        val localOrigin = "ANDROID_LOCAL_STRESS"
        val nextSeq = db.eventDao().getNextSequenceNumber(localOrigin)
        assertEquals(1L, nextSeq)

        val eventPayload = "{\"alias\":\"$peerAlias\",\"meshKey\":\"$peerMeshKey\",\"fingerprint\":\"$peerFp\",\"port\":$peerPort,\"timestamp\":$now}"
        val event = EventEntity(
            originDeviceId = localOrigin,
            sequenceNumber = nextSeq,
            eventId = UUID.randomUUID().toString(),
            timestampMs = now,
            eventType = "peer.paired",
            payload = eventPayload,
            payloadHash = sha256(eventPayload),
            ackStatus = 0
        )
        db.eventDao().insertEvent(event)
        db.vectorClockDao().advanceHighWaterMark(localOrigin, nextSeq, now, "COMMITTED")

        // 3. Verify ledger consistency
        val storedPeer = db.peerDao().getPeer(peerMeshKey)
        assertNotNull(storedPeer)
        assertEquals(peerAlias, storedPeer!!.alias)

        val storedEvents = db.eventDao().getAllEventsForOrigin(localOrigin)
        assertEquals(1, storedEvents.size)
        assertEquals("peer.paired", storedEvents[0].eventType)

        val maxSeqResult = db.vectorClockDao().getMaxSequence(localOrigin)
        assertEquals(1L, maxSeqResult)

        val allVectors = db.vectorClockDao().getAllVectors()
        val vc = allVectors.firstOrNull { it.peerDeviceId == localOrigin }
        assertNotNull(vc)
        assertEquals(1L, vc!!.maxSequenceNumber)
        assertEquals("COMMITTED", vc.syncState)
    }
}
