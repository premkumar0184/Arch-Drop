package org.archdrop.service

import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattServer
import android.bluetooth.BluetoothGattServerCallback
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.BluetoothLeAdvertiser
import android.bluetooth.le.BluetoothLeScanner
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager
import android.os.Build
import android.os.ParcelUuid
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import org.archdrop.db.PeerDao
import org.archdrop.db.PeerEntity
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.CopyOnWriteArrayList
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * Manages the Bluetooth Low Energy (BLE) GATT Peripheral server, advertiser, and background scanner for Arch Drop.
 *
 * Exposes a standardized GATT service that enables the Arch Linux desktop daemon (archdropd)
 * to wake the Android device from standby, trigger Local-Only Hotspot (LOHS) activation,
 * and securely retrieve SoftAP credentials (SSID and pre-shared key) over BLE.
 *
 * Enforces cryptographic 0x0D wakeup frame verification (4-byte HMAC-SHA256 rolling epoch token)
 * in constant time with +/- 2 min clock skew tolerance before allowing SoftAP activation.
 */
class BlePeripheralManager(
    private val context: Context,
    private val lohsManager: LOHSManager,
    private val peerDao: PeerDao? = null,
    private val scope: CoroutineScope = CoroutineScope(Dispatchers.IO + SupervisorJob())
) {
    companion object {
        private const val TAG = "ArchDrop:BlePeripheral"

        // Primary Arch Drop GATT Service UUID matching crates/archdrop_wireless
        val SERVICE_UUID: UUID = UUID.fromString("a4c80001-7d1a-4c91-9e28-3e2840000001")

        // Characteristic 1: Wakeup & Hotspot Trigger Command (Write)
        val CHAR_WAKEUP_COMMAND: UUID = UUID.fromString("a4c80002-7d1a-4c91-9e28-3e2840000001")

        // Characteristic 2: Local-Only Hotspot (LOHS) Wi-Fi Credentials (Read / Notify)
        val CHAR_LOHS_CREDENTIALS: UUID = UUID.fromString("a4c80003-7d1a-4c91-9e28-3e2840000001")

        // Characteristic 3: Companion Device Telemetry & Radio State (Read / Notify)
        val CHAR_DEVICE_TELEMETRY: UUID = UUID.fromString("a4c80004-7d1a-4c91-9e28-3e2840000001")

        // Characteristic 4: Zero-Wi-Fi Clipboard Synchronization (Write / WriteWithoutResponse / Read / Notify)
        val CHAR_CLIPBOARD_SYNC: UUID = UUID.fromString("a4c80005-7d1a-4c91-9e28-3e2840000001")

        // Client Characteristic Configuration Descriptor (CCCD) standard UUID (0x2902)
        val CCCD_UUID: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")

        // Status code constants
        const val STATUS_IDLE: Byte = 0x00
        const val STATUS_STARTING: Byte = 0x01
        const val STATUS_ACTIVE: Byte = 0x02
        const val STATUS_ERROR: Byte = 0x04

        // Continuity protocol constants
        const val APPLE_COMPANY_ID: Int = 0x004C
        const val CONTINUITY_TYPE_PROXIMITY_0X05: Byte = 0x05
        const val CONTINUITY_TYPE_HOTSPOT_WAKEUP: Byte = 0x0D
        const val CONTINUITY_TYPE_HOTSPOT_STATUS: Byte = 0x0E
        const val WAKEUP_PAYLOAD_LEN: Byte = 0x08
        const val STATUS_PAYLOAD_LEN: Byte = 0x06
        const val STATE_FLAG_SOFTAP_UP: Byte = 0x01
        const val STATE_FLAG_WPA3_ACTIVE: Byte = 0x02
        const val STATE_FLAG_5GHZ_ACTIVE: Byte = 0x04

        // Clipboard framing constants
        const val MIME_TYPE_TEXT_PLAIN: Byte = 0x01
        const val MIME_TYPE_URI_LIST: Byte = 0x02
        const val MAX_CLIPBOARD_PAYLOAD: Int = 512

        // Domain separation tag for BLE paired keys
        const val BLE_KEY_DERIVATION_SALT: String = "ARCHDROP-BLE-KEY-DERIVATION-V1"
        val FALLBACK_SECRET: ByteArray = "ARCHDROP_SECRET".toByteArray(Charsets.UTF_8)

        data class ParsedClipboardPacket(
            val authToken: ByteArray,
            val mimeType: Byte,
            val payloadLength: Int,
            val payload: ByteArray,
            val text: String
        ) {
            override fun equals(other: Any?): Boolean {
                if (this === other) return true
                if (javaClass != other?.javaClass) return false
                other as ParsedClipboardPacket
                return authToken.contentEquals(other.authToken) &&
                    mimeType == other.mimeType &&
                    payloadLength == other.payloadLength &&
                    payload.contentEquals(other.payload) &&
                    text == other.text
            }

            override fun hashCode(): Int {
                var result = authToken.contentHashCode()
                result = 31 * result + mimeType
                result = 31 * result + payloadLength
                result = 31 * result + payload.contentHashCode()
                result = 31 * result + text.hashCode()
                return result
            }
        }

        /**
         * Parses a compact BLE GATT clipboard packet.
         * Format: 4B auth token, 1B mime type, 2B big-endian length, NB UTF-8 text.
         */
        fun parseClipboardPacket(data: ByteArray?): ParsedClipboardPacket? {
            if (data == null || data.size < 7) return null
            val authToken = data.copyOfRange(0, 4)
            val mimeType = data[4]
            val payloadLen = ((data[5].toInt() and 0xFF) shl 8) or (data[6].toInt() and 0xFF)
            if (payloadLen > MAX_CLIPBOARD_PAYLOAD || data.size < 7 + payloadLen) return null
            val payload = data.copyOfRange(7, 7 + payloadLen)
            val text = try {
                String(payload, Charsets.UTF_8)
            } catch (e: Exception) {
                return null
            }
            return ParsedClipboardPacket(authToken, mimeType, payloadLen, payload, text)
        }

        /**
         * Serializes a clipboard text payload into compact BLE GATT framing bytes.
         */
        fun encodeClipboardPacket(
            key: ByteArray,
            mimeType: Byte,
            text: String,
            epochMinute: Long = System.currentTimeMillis() / 60_000L
        ): ByteArray {
            val payload = text.toByteArray(Charsets.UTF_8)
            val safePayload = if (payload.size > MAX_CLIPBOARD_PAYLOAD) {
                payload.copyOfRange(0, MAX_CLIPBOARD_PAYLOAD)
            } else {
                payload
            }
            val token = computeHmacToken(key, epochMinute)
            val buf = ByteBuffer.allocate(7 + safePayload.size).order(ByteOrder.BIG_ENDIAN)
            buf.put(token)
            buf.put(mimeType)
            buf.putShort(safePayload.size.toShort())
            buf.put(safePayload)
            return buf.array()
        }

        /**
         * Verifies the 4-byte auth token in constant time with +/- 2 minute clock skew tolerance.
         */
        fun verifyClipboardPacket(
            packet: ByteArray?,
            pairedKeys: List<ByteArray>,
            currentEpochMinute: Long = System.currentTimeMillis() / 60_000L
        ): Boolean {
            if (packet == null || pairedKeys.isEmpty()) return false
            val parsed = parseClipboardPacket(packet) ?: return false
            for (key in pairedKeys) {
                if (key.isEmpty()) continue
                for (offset in listOf(0L, -1L, 1L, -2L, 2L)) {
                    val testMinute = (currentEpochMinute + offset).coerceAtLeast(0L)
                    val expectedToken = computeHmacToken(key, testMinute)
                    if (constantTimeEq4(parsed.authToken, expectedToken) &&
                        MessageDigest.isEqual(parsed.authToken, expectedToken)
                    ) {
                        return true
                    }
                }
            }
            return false
        }

        data class ParsedWakeupPacket(
            val controlFlags: Byte,
            val targetBand: Byte,
            val authToken: ByteArray,
            val transferPort: Int
        )

        /**
         * Parses a 10-byte (sub-payload) or 12-byte (raw manufacturer data) 0x0D wakeup packet.
         */
        fun parseWakeupPayload(data: ByteArray?): ParsedWakeupPacket? {
            if (data == null || data.size < 10) return null
            val payload = if (data.size >= 12 &&
                data[0] == (APPLE_COMPANY_ID and 0xFF).toByte() &&
                data[1] == ((APPLE_COMPANY_ID shr 8) and 0xFF).toByte()
            ) {
                data.copyOfRange(2, data.size)
            } else {
                data
            }
            if (payload.size < 10) return null
            if (payload[0] != CONTINUITY_TYPE_HOTSPOT_WAKEUP) return null
            if (payload[1] != WAKEUP_PAYLOAD_LEN) return null

            val controlFlags = payload[2]
            val targetBand = payload[3]
            val authToken = payload.copyOfRange(4, 8)
            val transferPort = ((payload[8].toInt() and 0xFF) shl 8) or (payload[9].toInt() and 0xFF)

            return ParsedWakeupPacket(controlFlags, targetBand, authToken, transferPort)
        }

        /**
         * Computes the 4-byte HMAC-SHA256 rolling token for a given 64-bit epoch minute.
         */
        fun computeHmacToken(key: ByteArray, epochMinute: Long): ByteArray {
            val mac = Mac.getInstance("HmacSHA256")
            mac.init(SecretKeySpec(key, "HmacSHA256"))
            val minuteBytes = ByteBuffer.allocate(8)
                .order(ByteOrder.BIG_ENDIAN)
                .putLong(epochMinute)
                .array()
            val fullTag = mac.doFinal(minuteBytes)
            return fullTag.copyOfRange(0, 4)
        }

        /**
         * Compares two 4-byte arrays in constant time using bitwise XOR accumulation.
         */
        fun constantTimeEq4(a: ByteArray, b: ByteArray): Boolean {
            if (a.size != 4 || b.size != 4) return false
            var diff = 0
            for (i in 0 until 4) {
                diff = diff or (a[i].toInt() xor b[i].toInt())
            }
            return diff == 0
        }

        /**
         * Verifies the 4-byte auth token in constant time with +/- 2 minute clock skew tolerance.
         */
        fun verifyWakeupPayload(
            payload: ByteArray?,
            pairedKeys: List<ByteArray>,
            currentEpochMinute: Long = System.currentTimeMillis() / 60_000L
        ): Boolean {
            if (payload == null || pairedKeys.isEmpty()) return false
            val parsed = parseWakeupPayload(payload) ?: return false

            for (key in pairedKeys) {
                if (key.isEmpty()) continue
                for (offset in listOf(0L, -1L, 1L, -2L, 2L)) {
                    val testMinute = (currentEpochMinute + offset).coerceAtLeast(0L)
                    val expectedToken = computeHmacToken(key, testMinute)
                    if (constantTimeEq4(parsed.authToken, expectedToken) &&
                        MessageDigest.isEqual(parsed.authToken, expectedToken)
                    ) {
                        return true
                    }
                }
            }
            return false
        }

        /**
         * Derives a deterministic 32-byte symmetric paired key from a peer identity string.
         */
        fun derivePairedKey(identity: String): ByteArray {
            val cleaned = identity.filter { it.isDigit() || it in 'a'..'f' || it in 'A'..'F' }
            val rawBytes = if (cleaned.isNotEmpty() && cleaned.length % 2 == 0) {
                hexToByteArray(cleaned) ?: identity.toByteArray(Charsets.UTF_8)
            } else {
                identity.toByteArray(Charsets.UTF_8)
            }
            val mac = Mac.getInstance("HmacSHA256")
            mac.init(SecretKeySpec(BLE_KEY_DERIVATION_SALT.toByteArray(Charsets.UTF_8), "HmacSHA256"))
            return mac.doFinal(rawBytes)
        }

        /**
         * Converts hex-encoded string to raw bytes safely.
         */
        fun hexToByteArray(hex: String): ByteArray? {
            val clean = hex.trim()
            if (clean.length % 2 != 0) return null
            return try {
                ByteArray(clean.length / 2) { i ->
                    clean.substring(i * 2, i * 2 + 2).toInt(16).toByte()
                }
            } catch (e: Exception) {
                null
            }
        }

        /**
         * Extracts candidate HMAC secret keys from a paired PeerEntity.
         */
        fun extractKeysFromPeer(peer: PeerEntity): List<ByteArray> {
            val keys = mutableListOf<ByteArray>()
            // 1. Derived symmetric keys
            if (peer.meshKey.isNotEmpty()) keys.add(derivePairedKey(peer.meshKey))
            if (peer.fingerprint.isNotEmpty()) keys.add(derivePairedKey(peer.fingerprint))
            // 2. Direct binary decoded hex and raw UTF-8 keys
            hexToByteArray(peer.meshKey)?.let { if (it.isNotEmpty()) keys.add(it) }
            if (peer.meshKey.isNotEmpty()) keys.add(peer.meshKey.toByteArray(Charsets.UTF_8))
            hexToByteArray(peer.fingerprint)?.let { if (it.isNotEmpty()) keys.add(it) }
            if (peer.fingerprint.isNotEmpty()) keys.add(peer.fingerprint.toByteArray(Charsets.UTF_8))
            return keys
        }
    }

    @Serializable
    data class BleCredentialsPayload(
        val ssid: String,
        val passphrase: String = "",
        @SerialName("gateway_ip") val gateway_ip: String = "192.168.43.1",
        val gatewayIp: String = "192.168.43.1",
        val port: Int = 53317,
        val httpPort: Int = 53317,
        val securityType: Int = 1
    )

    private val bluetoothManager = context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
    private val bluetoothAdapter = bluetoothManager?.adapter
    private var advertiser: BluetoothLeAdvertiser? = null
    private var gattServer: BluetoothGattServer? = null
    private var bleScanner: BluetoothLeScanner? = null
    private var scanCallback: ScanCallback? = null

    private var currentStatus: Byte = STATUS_IDLE
    private var currentCredentialsJson: ByteArray? = null
    private var currentClipboardPacket: ByteArray? = null
    var clipboardSyncManager: org.archdrop.clipboard.ClipboardSyncManager? = null
    var onClipboardReceived: ((String, String) -> Unit)? = null
    private val connectedDevices = mutableSetOf<BluetoothDevice>()
    private val cachedPairedKeys = CopyOnWriteArrayList<ByteArray>()
    private val manualPairedKeys = CopyOnWriteArrayList<ByteArray>()
    private val localIdentityKeys = CopyOnWriteArrayList<ByteArray>()
    @Volatile private var peersLoaded = false
    private val clipboardLock = Any()
    private val prepareWriteBuffers = mutableMapOf<String, java.io.ByteArrayOutputStream>()

    private val json = Json {
        encodeDefaults = true
        ignoreUnknownKeys = true
    }

    init {
        addLocalIdentityKeys()
        val dao = peerDao ?: try {
            org.archdrop.db.ArchDropDatabase.getInstance(context).peerDao()
        } catch (e: Exception) {
            null
        }
        if (dao != null) {
            scope.launch {
                try {
                    dao.observeTrustedPeers().collect { peers ->
                        val keys = peers.flatMap { extractKeysFromPeer(it) }
                        cachedPairedKeys.clear()
                        cachedPairedKeys.addAll(keys)
                        peersLoaded = true
                        Log.d(TAG, "Refreshed cached paired keys: count=${cachedPairedKeys.size}")
                    }
                } catch (e: Exception) {
                    Log.w(TAG, "Could not observe trusted peers in BLE peripheral manager", e)
                }
            }
        }
    }

    private fun addLocalIdentityKeys() {
        try {
            val keyFile = java.io.File(context.filesDir, "mesh_identity.key")
            if (keyFile.exists()) {
                val hex = org.archdrop.mesh.MeshIdentity.loadOrGenerate(keyFile).toHex()
                val derived = derivePairedKey(hex)
                if (!localIdentityKeys.any { it.contentEquals(derived) }) {
                    localIdentityKeys.add(derived)
                }
                hexToByteArray(hex)?.let { b ->
                    if (b.isNotEmpty() && !localIdentityKeys.any { it.contentEquals(b) }) {
                        localIdentityKeys.add(b)
                    }
                }
            }
        } catch (e: Exception) {
            Log.d(TAG, "Could not load local identity for BLE auth: ${e.message}")
        }
    }

    fun getPairedKeys(): List<ByteArray> {
        val dao = peerDao ?: try {
            org.archdrop.db.ArchDropDatabase.getInstance(context).peerDao()
        } catch (e: Exception) {
            null
        }
        if (dao != null && !peersLoaded) {
            try {
                val peers = runBlocking(Dispatchers.IO) { dao.getAllPeers() }
                val keys = peers.filter { it.isTrusted }.flatMap { extractKeysFromPeer(it) }
                for (k in keys) {
                    if (!cachedPairedKeys.any { it.contentEquals(k) }) {
                        cachedPairedKeys.add(k)
                    }
                }
                peersLoaded = true
            } catch (e: Exception) {
                Log.w(TAG, "Fallback synchronous peer key fetch failed", e)
            }
        }
        val all = mutableListOf<ByteArray>()
        all.addAll(cachedPairedKeys)
        all.addAll(manualPairedKeys)
        all.addAll(localIdentityKeys)
        all.add(FALLBACK_SECRET)
        return all
    }

    fun addPairedKey(key: ByteArray) {
        if (key.isNotEmpty() && !manualPairedKeys.any { it.contentEquals(key) }) {
            manualPairedKeys.add(key)
        }
    }

    private val advertiseCallback = object : AdvertiseCallback() {
        override fun onStartSuccess(settingsInEffect: AdvertiseSettings?) {
            Log.i(TAG, "BLE advertising successfully started for Service UUID $SERVICE_UUID")
        }

        override fun onStartFailure(errorCode: Int) {
            val err = when (errorCode) {
                ADVERTISE_FAILED_ALREADY_STARTED -> "ADVERTISE_FAILED_ALREADY_STARTED"
                ADVERTISE_FAILED_DATA_TOO_LARGE -> "ADVERTISE_FAILED_DATA_TOO_LARGE"
                ADVERTISE_FAILED_FEATURE_UNSUPPORTED -> "ADVERTISE_FAILED_FEATURE_UNSUPPORTED"
                ADVERTISE_FAILED_INTERNAL_ERROR -> "ADVERTISE_FAILED_INTERNAL_ERROR"
                ADVERTISE_FAILED_TOO_MANY_ADVERTISERS -> "ADVERTISE_FAILED_TOO_MANY_ADVERTISERS"
                else -> "ERROR_CODE_$errorCode"
            }
            Log.e(TAG, "BLE advertising failed: $err")
        }
    }

    private val gattServerCallback = object : BluetoothGattServerCallback() {
        override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
            super.onConnectionStateChange(device, status, newState)
            Log.d(TAG, "Device ${device.address} connection state changed: status=$status, newState=$newState")
            synchronized(connectedDevices) {
                if (newState == BluetoothProfile.STATE_CONNECTED) {
                    connectedDevices.add(device)
                    Log.i(TAG, "Central connected: ${device.address}")
                } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                    connectedDevices.remove(device)
                    synchronized(prepareWriteBuffers) {
                        prepareWriteBuffers.remove(device.address)
                    }
                    Log.i(TAG, "Central disconnected: ${device.address}")
                }
            }
        }

        override fun onExecuteWrite(device: BluetoothDevice, requestId: Int, execute: Boolean) {
            super.onExecuteWrite(device, requestId, execute)
            val assembled = synchronized(prepareWriteBuffers) {
                val stream = prepareWriteBuffers.remove(device.address)
                if (execute) stream?.toByteArray() else null
            }

            var success = true
            if (execute && assembled != null) {
                success = handleClipboardPayload(device, assembled)
            }

            val status = if (success) BluetoothGatt.GATT_SUCCESS else BluetoothGatt.GATT_FAILURE
            gattServer?.sendResponse(device, requestId, status, 0, null)
        }

        override fun onCharacteristicWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            characteristic: BluetoothGattCharacteristic,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray?
        ) {
            super.onCharacteristicWriteRequest(device, requestId, characteristic, preparedWrite, responseNeeded, offset, value)
            synchronized(connectedDevices) {
                connectedDevices.add(device)
            }
            if (characteristic.uuid == CHAR_WAKEUP_COMMAND) {
                val keys = getPairedKeys()
                val isValid = verifyWakeupPayload(value, keys)

                if (!isValid) {
                    Log.w(TAG, "Wakeup command authentication failed from ${device.address}; rejecting write")
                    if (responseNeeded) {
                        gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_FAILURE, offset, null)
                    }
                    return
                }

                Log.i(TAG, "Authenticated wakeup command verified from ${device.address} (len=${value?.size ?: 0})")
                if (responseNeeded) {
                    gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, value)
                }

                currentStatus = STATUS_STARTING
                notifyTelemetryChanged()

                lohsManager.startHotspot { success, creds ->
                    if (success && creds != null) {
                        currentStatus = STATUS_ACTIVE
                        val payload = BleCredentialsPayload(
                            ssid = creds.ssid,
                            passphrase = creds.passphrase ?: "",
                            gateway_ip = creds.gatewayIp,
                            gatewayIp = creds.gatewayIp,
                            port = creds.httpPort,
                            httpPort = creds.httpPort,
                            securityType = creds.securityType
                        )
                        currentCredentialsJson = json.encodeToString(payload).toByteArray(Charsets.UTF_8)
                        Log.i(TAG, "LOHS active; credentials prepared for GATT read: SSID=${creds.ssid}")
                    } else {
                        currentStatus = STATUS_ERROR
                        currentCredentialsJson = null
                        Log.e(TAG, "LOHS start failed after wakeup command")
                    }
                    notifyTelemetryChanged()
                    notifyCredentialsReady()
                }
            } else if (characteristic.uuid == CHAR_CLIPBOARD_SYNC) {
                if (preparedWrite) {
                    val stream = synchronized(prepareWriteBuffers) {
                        prepareWriteBuffers.getOrPut(device.address) { java.io.ByteArrayOutputStream() }
                    }
                    if (value == null || stream.size() + value.size > MAX_CLIPBOARD_PAYLOAD + 16) {
                        if (responseNeeded) {
                            gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_FAILURE, offset, null)
                        }
                        return
                    }
                    synchronized(prepareWriteBuffers) {
                        stream.write(value)
                    }
                    if (responseNeeded) {
                        gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, value)
                    }
                    return
                }

                if (value == null) {
                    if (responseNeeded) {
                        gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_FAILURE, offset, null)
                    }
                    return
                }

                val ok = handleClipboardPayload(device, value)
                if (responseNeeded) {
                    val status = if (ok) BluetoothGatt.GATT_SUCCESS else BluetoothGatt.GATT_FAILURE
                    val respVal = if (ok) value else null
                    gattServer?.sendResponse(device, requestId, status, offset, respVal)
                }
            } else {
                Log.w(TAG, "Write request on unsupported characteristic: ${characteristic.uuid}")
                if (responseNeeded) {
                    gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_REQUEST_NOT_SUPPORTED, offset, null)
                }
            }
        }

        override fun onCharacteristicReadRequest(
            device: BluetoothDevice,
            requestId: Int,
            offset: Int,
            characteristic: BluetoothGattCharacteristic
        ) {
            super.onCharacteristicReadRequest(device, requestId, offset, characteristic)
            when (characteristic.uuid) {
                CHAR_DEVICE_TELEMETRY -> {
                    val data = buildTelemetryFrame()
                    val slice = if (offset < data.size) data.copyOfRange(offset, data.size) else byteArrayOf()
                    gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, slice)
                }
                CHAR_LOHS_CREDENTIALS -> {
                    val data = currentCredentialsJson ?: byteArrayOf()
                    val slice = if (offset < data.size) data.copyOfRange(offset, data.size) else byteArrayOf()
                    gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, slice)
                }
                CHAR_CLIPBOARD_SYNC -> {
                    val data = currentClipboardPacket ?: byteArrayOf()
                    val slice = if (offset < data.size) data.copyOfRange(offset, data.size) else byteArrayOf()
                    gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, slice)
                }
                else -> {
                    gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_READ_NOT_PERMITTED, offset, null)
                }
            }
        }

        override fun onDescriptorWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            descriptor: android.bluetooth.BluetoothGattDescriptor,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray?
        ) {
            super.onDescriptorWriteRequest(device, requestId, descriptor, preparedWrite, responseNeeded, offset, value)
            if (descriptor.uuid == CCCD_UUID) {
                synchronized(connectedDevices) {
                    if (value != null && (value.contentEquals(android.bluetooth.BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE) ||
                        value.contentEquals(android.bluetooth.BluetoothGattDescriptor.ENABLE_INDICATION_VALUE))) {
                        connectedDevices.add(device)
                        Log.i(TAG, "Central ${device.address} subscribed to notifications for ${descriptor.characteristic?.uuid}")
                    }
                }
            }
            if (responseNeeded) {
                gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, value)
            }
        }

        override fun onDescriptorReadRequest(
            device: BluetoothDevice,
            requestId: Int,
            offset: Int,
            descriptor: android.bluetooth.BluetoothGattDescriptor
        ) {
            super.onDescriptorReadRequest(device, requestId, offset, descriptor)
            val response = if (descriptor.uuid == CCCD_UUID) {
                android.bluetooth.BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
            } else {
                byteArrayOf()
            }
            val slice = if (offset < response.size) response.copyOfRange(offset, response.size) else byteArrayOf()
            gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, slice)
        }

        override fun onMtuChanged(device: BluetoothDevice, mtu: Int) {
            super.onMtuChanged(device, mtu)
            Log.d(TAG, "GATT MTU negotiated for ${device.address}: $mtu bytes")
        }
    }

    @Synchronized
    fun start() {
        if (bluetoothAdapter == null || !bluetoothAdapter.isEnabled) {
            Log.w(TAG, "BluetoothAdapter is disabled or unavailable")
            return
        }

        advertiser = bluetoothAdapter.bluetoothLeAdvertiser
        if (advertiser == null) {
            Log.w(TAG, "BluetoothLeAdvertiser is not supported on this device")
            return
        }

        setupGattServer()
        startAdvertising()
        startScanning()
    }

    private fun startScanning() {
        val adapter = bluetoothAdapter ?: return
        if (!adapter.isEnabled) return
        val scanner = adapter.bluetoothLeScanner ?: run {
            Log.w(TAG, "BluetoothLeScanner not available")
            return
        }

        val filters = listOf(
            ScanFilter.Builder()
                .setManufacturerData(
                    APPLE_COMPANY_ID,
                    byteArrayOf(CONTINUITY_TYPE_HOTSPOT_WAKEUP),
                    byteArrayOf(0xFF.toByte())
                )
                .build(),
            ScanFilter.Builder()
                .setManufacturerData(
                    APPLE_COMPANY_ID,
                    byteArrayOf(CONTINUITY_TYPE_PROXIMITY_0X05),
                    byteArrayOf(0xFF.toByte())
                )
                .build(),
            ScanFilter.Builder()
                .setServiceUuid(ParcelUuid(SERVICE_UUID))
                .build()
        )

        val settings = ScanSettings.Builder()
            .setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY)
            .setReportDelay(0L)
            .build()

        val callback = object : ScanCallback() {
            override fun onScanResult(callbackType: Int, result: ScanResult?) {
                super.onScanResult(callbackType, result)
                handleScanResult(result)
            }

            override fun onBatchScanResults(results: MutableList<ScanResult>?) {
                super.onBatchScanResults(results)
                results?.forEach { handleScanResult(it) }
            }

            override fun onScanFailed(errorCode: Int) {
                super.onScanFailed(errorCode)
                Log.w(TAG, "BLE background scan failed with error code $errorCode")
            }
        }

        try {
            stopScanning()
            scanner.startScan(filters, settings, callback)
            bleScanner = scanner
            scanCallback = callback
            Log.i(TAG, "BLE background scan started with ${filters.size} ScanFilters")
        } catch (e: SecurityException) {
            Log.w(TAG, "Missing BLUETOOTH_SCAN permission; scanning omitted", e)
        } catch (e: Exception) {
            Log.w(TAG, "Exception starting BLE scan", e)
        }
    }

    private fun handleScanResult(result: ScanResult?) {
        val res = result ?: return
        val scanRecord = res.scanRecord ?: return
        val mfgData = scanRecord.getManufacturerSpecificData(APPLE_COMPANY_ID) ?: return
        if (mfgData.isEmpty()) return

        when (mfgData[0]) {
            CONTINUITY_TYPE_PROXIMITY_0X05 -> {
                Log.d(TAG, "Proximity beacon 0x05 detected from ${res.device.address}, RSSI=${res.rssi}")
            }
            CONTINUITY_TYPE_HOTSPOT_WAKEUP -> {
                val keys = getPairedKeys()
                if (verifyWakeupPayload(mfgData, keys)) {
                    Log.i(TAG, "Authenticated 0x0D wakeup beacon verified from ${res.device.address}; activating LOHS")
                    currentStatus = STATUS_STARTING
                    notifyTelemetryChanged()
                    lohsManager.startHotspot { success, creds ->
                        if (success && creds != null) {
                            currentStatus = STATUS_ACTIVE
                            val payload = BleCredentialsPayload(
                                ssid = creds.ssid,
                                passphrase = creds.passphrase ?: "",
                                gateway_ip = creds.gatewayIp,
                                gatewayIp = creds.gatewayIp,
                                port = creds.httpPort,
                                httpPort = creds.httpPort,
                                securityType = creds.securityType
                            )
                            currentCredentialsJson = json.encodeToString(payload).toByteArray(Charsets.UTF_8)
                        } else {
                            currentStatus = STATUS_ERROR
                            currentCredentialsJson = null
                        }
                        notifyTelemetryChanged()
                        notifyCredentialsReady()
                    }
                } else {
                    Log.w(TAG, "Unauthenticated 0x0D wakeup beacon ignored from ${res.device.address}")
                }
            }
        }
    }

    private fun stopScanning() {
        try {
            scanCallback?.let { bleScanner?.stopScan(it) }
        } catch (e: Exception) {
            Log.w(TAG, "Exception stopping BLE scan", e)
        }
        bleScanner = null
        scanCallback = null
    }

    private fun setupGattServer() {
        if (gattServer != null) return

        val server = bluetoothManager?.openGattServer(context, gattServerCallback) ?: run {
            Log.e(TAG, "Failed to open GATT server")
            return
        }
        gattServer = server

        val service = BluetoothGattService(SERVICE_UUID, BluetoothGattService.SERVICE_TYPE_PRIMARY)

        val charWake = BluetoothGattCharacteristic(
            CHAR_WAKEUP_COMMAND,
            BluetoothGattCharacteristic.PROPERTY_WRITE or BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE,
            BluetoothGattCharacteristic.PERMISSION_WRITE
        )

        val charCreds = BluetoothGattCharacteristic(
            CHAR_LOHS_CREDENTIALS,
            BluetoothGattCharacteristic.PROPERTY_READ or BluetoothGattCharacteristic.PROPERTY_NOTIFY,
            BluetoothGattCharacteristic.PERMISSION_READ
        )

        val charTelemetry = BluetoothGattCharacteristic(
            CHAR_DEVICE_TELEMETRY,
            BluetoothGattCharacteristic.PROPERTY_READ or BluetoothGattCharacteristic.PROPERTY_NOTIFY,
            BluetoothGattCharacteristic.PERMISSION_READ
        )

        val charClipboard = BluetoothGattCharacteristic(
            CHAR_CLIPBOARD_SYNC,
            BluetoothGattCharacteristic.PROPERTY_WRITE or
                BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE or
                BluetoothGattCharacteristic.PROPERTY_READ or
                BluetoothGattCharacteristic.PROPERTY_NOTIFY,
            BluetoothGattCharacteristic.PERMISSION_WRITE or BluetoothGattCharacteristic.PERMISSION_READ
        )

        fun createCccd(): android.bluetooth.BluetoothGattDescriptor {
            return android.bluetooth.BluetoothGattDescriptor(
                CCCD_UUID,
                android.bluetooth.BluetoothGattDescriptor.PERMISSION_READ or android.bluetooth.BluetoothGattDescriptor.PERMISSION_WRITE
            )
        }
        charCreds.addDescriptor(createCccd())
        charTelemetry.addDescriptor(createCccd())
        charClipboard.addDescriptor(createCccd())

        service.addCharacteristic(charWake)
        service.addCharacteristic(charCreds)
        service.addCharacteristic(charTelemetry)
        service.addCharacteristic(charClipboard)

        val added = server.addService(service)
        Log.i(TAG, "Arch Drop GATT service added: success=$added, UUID=$SERVICE_UUID")
    }

    private fun startAdvertising() {
        val adv = advertiser ?: return

        try {
            adv.stopAdvertising(advertiseCallback)
        } catch (e: Exception) {
            Log.d(TAG, "Previous advertisement stop ignored: ${e.message}")
        }

        val settings = AdvertiseSettings.Builder()
            .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_BALANCED)
            .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_HIGH)
            .setConnectable(true)
            .setTimeout(0)
            .build()

        val data = AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .setIncludeTxPowerLevel(false)
            .addServiceUuid(ParcelUuid(SERVICE_UUID))
            .build()

        val scanResponse = AdvertiseData.Builder()
            .setIncludeDeviceName(true)
            .addManufacturerData(APPLE_COMPANY_ID, buildTelemetryFrame())
            .build()

        adv.startAdvertising(settings, data, scanResponse, advertiseCallback)
    }

    private fun buildTelemetryFrame(): ByteArray {
        val batteryPct = getBatteryPercentage()
        var stateFlags: Byte = 0
        if (currentStatus == STATUS_ACTIVE) {
            stateFlags = (stateFlags.toInt() or STATE_FLAG_SOFTAP_UP.toInt()).toByte()
        }

        // Apple Continuity 0x0E format:
        // [0x0E, 0x06, batteryPct, networkType, signalBars, stateFlags, freqHi, freqLo]
        return byteArrayOf(
            CONTINUITY_TYPE_HOTSPOT_STATUS,
            STATUS_PAYLOAD_LEN,
            batteryPct.toByte(),
            0x01, // Wi-Fi RAT type
            0x04, // 4 signal bars
            stateFlags,
            (2412 shr 8).toByte(),
            (2412 and 0xFF).toByte()
        )
    }

    private fun getBatteryPercentage(): Int {
        return try {
            val intent = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
            val level = intent?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: -1
            val scale = intent?.getIntExtra(BatteryManager.EXTRA_SCALE, -1) ?: -1
            if (level >= 0 && scale > 0) {
                (level * 100) / scale
            } else {
                80
            }
        } catch (e: Exception) {
            80
        }
    }

    private fun notifyTelemetryChanged() {
        val server = gattServer ?: return
        val service = server.getService(SERVICE_UUID) ?: return
        val charTelemetry = service.getCharacteristic(CHAR_DEVICE_TELEMETRY) ?: return
        val data = buildTelemetryFrame()

        synchronized(connectedDevices) {
            for (dev in connectedDevices) {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    server.notifyCharacteristicChanged(dev, charTelemetry, false, data)
                } else {
                    @Suppress("DEPRECATION")
                    charTelemetry.value = data
                    @Suppress("DEPRECATION")
                    server.notifyCharacteristicChanged(dev, charTelemetry, false)
                }
            }
        }
    }

    private fun notifyCredentialsReady() {
        val server = gattServer ?: return
        val service = server.getService(SERVICE_UUID) ?: return
        val charCreds = service.getCharacteristic(CHAR_LOHS_CREDENTIALS) ?: return
        val data = currentCredentialsJson ?: byteArrayOf()

        synchronized(connectedDevices) {
            for (dev in connectedDevices) {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    server.notifyCharacteristicChanged(dev, charCreds, false, data)
                } else {
                    @Suppress("DEPRECATION")
                    charCreds.value = data
                    @Suppress("DEPRECATION")
                    server.notifyCharacteristicChanged(dev, charCreds, false)
                }
            }
        }
    }

    fun notifyClipboardChanged(packet: ByteArray) {
        currentClipboardPacket = packet
        val server = gattServer ?: return
        val service = server.getService(SERVICE_UUID) ?: return
        val charClip = service.getCharacteristic(CHAR_CLIPBOARD_SYNC) ?: return

        val targets = mutableSetOf<BluetoothDevice>()
        synchronized(connectedDevices) {
            targets.addAll(connectedDevices)
        }
        try {
            bluetoothManager?.getConnectedDevices(android.bluetooth.BluetoothProfile.GATT)?.let {
                targets.addAll(it)
            }
        } catch (e: Exception) {
            Log.d(TAG, "Could not query BluetoothManager connected devices: ${e.message}")
        }

        for (dev in targets) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                server.notifyCharacteristicChanged(dev, charClip, false, packet)
            } else {
                @Suppress("DEPRECATION")
                charClip.value = packet
                @Suppress("DEPRECATION")
                server.notifyCharacteristicChanged(dev, charClip, false)
            }
        }
    }

    fun handleClipboardPayload(device: BluetoothDevice, raw: ByteArray): Boolean {
        val keys = getPairedKeys()
        val isValid = verifyClipboardPacket(raw, keys)
        if (!isValid) {
            Log.w(TAG, "Clipboard sync authentication failed from ${device.address}; rejecting write")
            return false
        }

        val parsed = parseClipboardPacket(raw)
        if (parsed == null) {
            Log.w(TAG, "Clipboard packet malformed from ${device.address}")
            return false
        }

        Log.i(TAG, "Authenticated clipboard sync verified from ${device.address} (len=${parsed.payloadLength})")
        val text = parsed.text
        val hash = org.archdrop.clipboard.ClipboardSyncManager.sha256(text)
        synchronized(clipboardLock) {
            val cm = clipboardSyncManager
            if (cm != null) {
                if (cm.isRecentHash(hash)) {
                    Log.d(TAG, "Echo suppressed in BLE peripheral: hash matches recent remote paste")
                } else {
                    cm.setLastPastedHash(hash)
                    cm.pasteToSystem(text)
                    currentClipboardPacket = raw
                    onClipboardReceived?.invoke(text, device.address)
                }
            } else {
                currentClipboardPacket = raw
                onClipboardReceived?.invoke(text, device.address)
            }
        }
        return true
    }

    fun writeOrNotifyClipboard(text: String, mimeType: Byte = MIME_TYPE_TEXT_PLAIN): Boolean {
        val payload = text.toByteArray(Charsets.UTF_8)
        if (payload.size > MAX_CLIPBOARD_PAYLOAD) {
            return false
        }
        // Only select genuine paired peer keys for outgoing signing; if none exist, fall back to FALLBACK_SECRET
        val peerKeys = getPairedKeys().filter { key ->
            !key.contentEquals(FALLBACK_SECRET) && !localIdentityKeys.any { it.contentEquals(key) }
        }
        val key = peerKeys.firstOrNull() ?: FALLBACK_SECRET
        val packet = encodeClipboardPacket(key, mimeType, text)
        notifyClipboardChanged(packet)
        return true
    }

    @Synchronized
    fun stop() {
        stopScanning()
        try {
            advertiser?.stopAdvertising(advertiseCallback)
        } catch (e: Exception) {
            Log.w(TAG, "Exception stopping BLE advertising", e)
        }
        advertiser = null

        try {
            gattServer?.close()
        } catch (e: Exception) {
            Log.w(TAG, "Exception closing GATT server", e)
        }
        gattServer = null

        synchronized(connectedDevices) {
            connectedDevices.clear()
        }
        synchronized(prepareWriteBuffers) {
            prepareWriteBuffers.clear()
        }
        currentStatus = STATUS_IDLE
        currentCredentialsJson = null
        currentClipboardPacket = null
        scope.cancel()
    }
}
