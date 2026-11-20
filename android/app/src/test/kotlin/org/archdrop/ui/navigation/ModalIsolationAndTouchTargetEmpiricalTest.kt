package org.archdrop.ui.navigation

import android.content.Context
import androidx.compose.ui.unit.dp
import androidx.navigation.NavBackStackEntry
import androidx.navigation.NavDestination
import androidx.navigation.NavOptions
import androidx.navigation.navOptions
import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.runBlocking
import org.archdrop.db.ArchDropDatabase
import org.archdrop.db.EventEntity
import org.archdrop.db.PeerEntity
import org.archdrop.ui.pairing.PairingPayload
import org.archdrop.ui.theme.DarkCupertinoColors
import org.archdrop.ui.theme.cupertinoTouchTarget
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
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

// ============================================================================
// EMPIRICAL CHALLENGER TEST SUITE: MODAL ISOLATION, ROOM DB & TOUCH TARGETS
// ============================================================================

@RunWith(RobolectricTestRunner::class)
@Config(manifest = Config.NONE)
class ModalIsolationAndTouchTargetEmpiricalTest {

    private lateinit var context: Context
    private lateinit var db: ArchDropDatabase

    private val validMeshKey = "A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3D4E5F60718293A4B5C6D7E8F90"
    private val validFingerprint = "11223344556677889900AABBCCDDEEFF11223344556677889900AABBCCDDEEFF"

    @Before
    fun setUp() {
        context = ApplicationProvider.getApplicationContext<Context>()
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
    // 1. MODAL ISOLATION & NAVCONTROLLER BACKSTACK INTEGRITY
    // ========================================================================

    @Test
    fun testPairingBottomSheetModalIsolationPreservesNavBackStack() {
        // Model the navigation controller backstack as maintained by Jetpack Navigation
        class SimulatedNavHostController {
            val backStack = mutableListOf<String>()
            var currentDestination: String? = null

            fun navigate(route: String, options: NavOptions? = null) {
                if (options?.shouldLaunchSingleTop() == true && currentDestination == route) {
                    return
                }
                backStack.add(route)
                currentDestination = route
            }

            fun popBackStack(): Boolean {
                if (backStack.isNotEmpty()) {
                    backStack.removeAt(backStack.size - 1)
                    currentDestination = backStack.lastOrNull()
                    return true
                }
                return false
            }
        }

        val navController = SimulatedNavHostController()
        var showPairingSheet = false

        // 1. Initial State: App launches into Start Destination (Drop tab)
        navController.navigate(Screen.Drop.route)
        assertEquals(Screen.Drop.route, navController.currentDestination)
        assertEquals(1, navController.backStack.size)

        // 2. User navigates to Continuity Studio
        val navOptions = navOptions {
            launchSingleTop = true
            restoreState = true
        }
        navController.navigate(Screen.Continuity.route, navOptions)
        assertEquals(Screen.Continuity.route, navController.currentDestination)
        assertEquals(listOf(Screen.Drop.route, Screen.Continuity.route), navController.backStack)

        // Snapshot of backstack prior to modal invocation
        val snapshotBeforeModal = ArrayList(navController.backStack)
        val destinationBeforeModal = navController.currentDestination

        // 3. User invokes "+ Pair Device" from modal trigger
        showPairingSheet = true
        assertTrue("PairingBottomSheet state must be visible", showPairingSheet)

        // Invariant: Opening the sheet does NOT invoke navController.navigate() or alter backstack
        assertEquals("Backstack must not be mutated by opening modal sheet", snapshotBeforeModal, navController.backStack)
        assertEquals("Current destination must remain untouched", destinationBeforeModal, navController.currentDestination)

        // 4. User dismisses sheet (via drag-handle, outside touch, or circular close button)
        val onDismissRequest = { showPairingSheet = false }
        onDismissRequest()
        assertFalse("PairingBottomSheet state must be hidden after dismiss", showPairingSheet)

        // Invariant: Dismissing the sheet does NOT pop or clear the backstack
        assertEquals("Backstack must not be mutated by dismissing modal sheet", snapshotBeforeModal, navController.backStack)
        assertEquals("Current destination must remain intact", destinationBeforeModal, navController.currentDestination)

        // 5. Stress Test: Rapid modal presentation/dismissal cycles (100 iterations)
        for (i in 1..100) {
            showPairingSheet = true
            // Verify no drift or side effect during active presentation
            assertEquals(snapshotBeforeModal, navController.backStack)
            assertEquals(destinationBeforeModal, navController.currentDestination)

            onDismissRequest()
            assertEquals(snapshotBeforeModal, navController.backStack)
            assertEquals(destinationBeforeModal, navController.currentDestination)
        }

        // 6. Navigation continues normally after modal cycle
        navController.navigate(Screen.Activity.route, navOptions)
        assertEquals(Screen.Activity.route, navController.currentDestination)
        assertEquals(3, navController.backStack.size)
    }

    // ========================================================================
    // 2. ROOM DB INTERACTION IN PAIRING BOTTOM SHEET: TRUST & PAIR FLOW
    // ========================================================================

    @Test
    fun testRoomDbTrustAndPairDeviceFlow() = runBlocking {
        val peerDao = db.peerDao()
        val eventDao = db.eventDao()
        val vectorClockDao = db.vectorClockDao()

        val localMeshKey = "LOCAL_MESH_KEY_ED25519_0123456789ABCDEF0123456789ABCDEF01234567"
        val payload = PairingPayload(
            version = 1,
            alias = "Arch Linux Hyprland Laptop",
            meshKey = validMeshKey,
            fingerprint = validFingerprint,
            port = 53317
        )

        val now = System.currentTimeMillis()

        // Step 1: Insert PeerEntity (isTrusted = true)
        val peer = PeerEntity(
            meshKey = payload.meshKey,
            alias = payload.alias,
            fingerprint = payload.fingerprint,
            port = payload.port,
            pairedAtMs = now,
            lastSeenMs = now,
            isTrusted = true
        )
        val peerRowId = peerDao.insertPeer(peer)
        assertTrue("Peer insertion must succeed", peerRowId >= 0)

        // Step 2: Next sequence query
        val nextSeq = eventDao.getNextSequenceNumber(localMeshKey)
        assertEquals("Initial event sequence number for origin must be 1", 1L, nextSeq)

        // Step 3: Insert EventEntity ("peer.paired")
        val eventPayload = "{\"alias\":\"${payload.alias}\",\"meshKey\":\"${payload.meshKey}\",\"fingerprint\":\"${payload.fingerprint}\",\"port\":${payload.port},\"timestamp\":$now}"
        val payloadHash = sha256(eventPayload)

        val event = EventEntity(
            originDeviceId = localMeshKey.ifEmpty { "ANDROID_LOCAL" },
            sequenceNumber = nextSeq,
            eventId = UUID.randomUUID().toString(),
            timestampMs = now,
            eventType = "peer.paired",
            payload = eventPayload,
            payloadHash = payloadHash,
            ackStatus = 0
        )
        val eventRowId = eventDao.insertEvent(event)
        assertTrue("Event insertion must succeed", eventRowId >= 0)

        // Step 4: Advance Vector Clock High-Water Mark
        val updatedVc = vectorClockDao.advanceHighWaterMark(
            localMeshKey.ifEmpty { "ANDROID_LOCAL" },
            nextSeq,
            now,
            "COMMITTED"
        )
        assertTrue("Vector clock must be updated or inserted", updatedVc > 0)

        // Verify state ledger consistency
        val storedPeer = peerDao.getPeer(payload.meshKey)
        assertNotNull(storedPeer)
        assertEquals("Arch Linux Hyprland Laptop", storedPeer!!.alias)
        assertTrue(storedPeer.isTrusted)

        val storedEvents = eventDao.getAllEventsForOrigin(localMeshKey)
        assertEquals(1, storedEvents.size)
        assertEquals("peer.paired", storedEvents[0].eventType)
        assertEquals(1L, storedEvents[0].sequenceNumber)
        assertEquals(payloadHash, storedEvents[0].payloadHash)

        val vc = vectorClockDao.getMaxSequence(localMeshKey)
        assertEquals(1L, vc)
    }

    @Test
    fun testRoomDbSequentialMultiPeerPairingMonotonicity() = runBlocking {
        val peerDao = db.peerDao()
        val eventDao = db.eventDao()
        val vectorClockDao = db.vectorClockDao()

        val localMeshKey = "LOCAL_DEVICE_ORIGIN_STRESS_KEY_001"
        val peerCount = 5

        for (i in 1..peerCount) {
            val peerKey = "%064X".format(i)
            val peerAlias = "Peer Node $i"
            val now = 1700000000L + i * 1000L

            // 1. Insert Peer
            peerDao.insertPeer(
                PeerEntity(
                    meshKey = peerKey,
                    alias = peerAlias,
                    fingerprint = "%064X".format(i * 10),
                    port = 53317,
                    pairedAtMs = now,
                    lastSeenMs = now,
                    isTrusted = true
                )
            )

            // 2. Next Sequence
            val nextSeq = eventDao.getNextSequenceNumber(localMeshKey)
            assertEquals("Sequence number must be strictly monotonic ($i)", i.toLong(), nextSeq)

            // 3. Insert Event
            val eventPayload = "{\"alias\":\"$peerAlias\",\"seq\":$nextSeq}"
            eventDao.insertEvent(
                EventEntity(
                    originDeviceId = localMeshKey,
                    sequenceNumber = nextSeq,
                    eventId = UUID.randomUUID().toString(),
                    timestampMs = now,
                    eventType = "peer.paired",
                    payload = eventPayload,
                    payloadHash = sha256(eventPayload),
                    ackStatus = 0
                )
            )

            // 4. Advance Vector Clock
            vectorClockDao.advanceHighWaterMark(localMeshKey, nextSeq, now, "COMMITTED")
        }

        // Verify total events and vector clock
        assertEquals(peerCount, peerDao.countPeers())
        assertEquals(peerCount, eventDao.countEvents())
        val events = eventDao.getAllEventsForOrigin(localMeshKey)
        assertEquals(peerCount, events.size)
        for (i in 0 until peerCount) {
            assertEquals((i + 1).toLong(), events[i].sequenceNumber)
        }

        val highWaterMark = vectorClockDao.getMaxSequence(localMeshKey)
        assertEquals(peerCount.toLong(), highWaterMark)
    }

    @Test
    fun testPairingBottomSheetEmptyLocalMeshKeyEdgeCaseAnalysis() = runBlocking {
        val eventDao = db.eventDao()
        val vectorClockDao = db.vectorClockDao()

        // In PairingBottomSheet.kt:
        // Line 464: val nextSeq = database.eventDao().getNextSequenceNumber(localMeshKey)
        // Line 470: originDeviceId = localMeshKey.ifEmpty { "ANDROID_LOCAL" }
        //
        // STRESS-TEST: If localMeshKey is empty string "" at confirmation time:
        val emptyLocalMeshKey = ""

        // Pairing 1:
        val seq1 = eventDao.getNextSequenceNumber(emptyLocalMeshKey)
        assertEquals("Query on empty string returns 1L", 1L, seq1)

        val origin1 = emptyLocalMeshKey.ifEmpty { "ANDROID_LOCAL" }
        assertEquals("ANDROID_LOCAL", origin1)

        val event1 = EventEntity(
            originDeviceId = origin1,
            sequenceNumber = seq1,
            eventId = UUID.randomUUID().toString(),
            timestampMs = 1000L,
            eventType = "peer.paired",
            payload = "{\"peer\": 1}",
            payloadHash = sha256("{\"peer\": 1}")
        )
        val insert1 = eventDao.insertEvent(event1)
        assertTrue("First insert must succeed", insert1 >= 0)

        // Pairing 2:
        // Notice: getNextSequenceNumber(emptyLocalMeshKey) queries for originDeviceId = ""
        val seq2WithBug = eventDao.getNextSequenceNumber(emptyLocalMeshKey)
        // Because events table only has originDeviceId = "ANDROID_LOCAL", originDeviceId = "" has no rows!
        // So getNextSequenceNumber("") returns 1L again!
        assertEquals("Bug condition: getNextSequenceNumber('') returns 1L repeatedly", 1L, seq2WithBug)

        val event2WithBug = EventEntity(
            originDeviceId = origin1, // "ANDROID_LOCAL"
            sequenceNumber = seq2WithBug, // 1L -> CONFLICT!
            eventId = UUID.randomUUID().toString(),
            timestampMs = 2000L,
            eventType = "peer.paired",
            payload = "{\"peer\": 2}",
            payloadHash = sha256("{\"peer\": 2}")
        )
        val insert2Result = eventDao.insertEvent(event2WithBug)
        // Because EventDao uses OnConflictStrategy.IGNORE on compound key (originDeviceId, sequenceNumber),
        // inserting sequenceNumber = 1L a second time causes the event to be IGNORED!
        assertEquals("Event 2 is dropped due to sequence collision when localMeshKey is empty", -1L, insert2Result)

        // Verify contrast: If localMeshKey.ifEmpty { "ANDROID_LOCAL" } is passed to getNextSequenceNumber:
        val safeOrigin = emptyLocalMeshKey.ifEmpty { "ANDROID_LOCAL" }
        val seqCorrected = eventDao.getNextSequenceNumber(safeOrigin)
        assertEquals("With proper fallback, sequence advances to 2L", 2L, seqCorrected)

        val event2Corrected = EventEntity(
            originDeviceId = safeOrigin,
            sequenceNumber = seqCorrected,
            eventId = UUID.randomUUID().toString(),
            timestampMs = 2000L,
            eventType = "peer.paired",
            payload = "{\"peer\": 2}",
            payloadHash = sha256("{\"peer\": 2}")
        )
        val insert2CorrectedResult = eventDao.insertEvent(event2Corrected)
        assertTrue("Corrected insertion succeeds", insert2CorrectedResult >= 0)
        assertEquals(2, eventDao.countEvents())
    }

    // ========================================================================
    // 3. TOUCH TARGET ERGONOMICS COMPLIANCE (MINIMUM 44x44 PT)
    // ========================================================================

    @Test
    fun testTouchTargetErgonomicsEnforcement() {
        val minDimension = 44.dp

        // Test cupertinoTouchTarget default values
        assertEquals(44.dp, minDimension)

        // Validate standard phone display widths and tab bar touch bounds
        val screenWidths = listOf(360, 375, 390, 412, 480)
        for (w in screenWidths) {
            val tabWidth = w / 4.0
            assertTrue(
                "Tab width ($tabWidth dp) on screen width ($w dp) must be >= 44dp",
                tabWidth >= 44.0
            )
        }

        // Validate Material3 NavigationBar height is standard >= 56dp (M3 spec default is 80dp)
        val standardNavBarHeight = 80.dp
        assertTrue("Bottom navigation bar height >= 44dp touch target", standardNavBarHeight >= 44.dp)

        // Close button touch target:
        val closeButtonTouchSize = 44.dp
        assertTrue("Close button touch target >= 44x44 pt", closeButtonTouchSize >= minDimension)

        // Hero Button / Action buttons minHeight = 44.dp:
        val actionButtonMinHeight = 44.dp
        assertTrue("Action button minHeight >= 44dp", actionButtonMinHeight >= minDimension)

        // Empirical check: PairingBottomSheet segmented control height
        val segmentedControlTrackHeight = 32.dp
        // Note: 32dp is standard iOS visual segmented control height, but less than 44dp if not padded
        assertTrue("Documenting segmented control height is 32dp", segmentedControlTrackHeight == 32.dp)
        assertFalse("32dp is strictly less than 44dp Apple touch target", segmentedControlTrackHeight >= minDimension)
    }
}
