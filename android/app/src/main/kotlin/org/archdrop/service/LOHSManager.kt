package org.archdrop.service

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.net.Inet4Address
import java.net.NetworkInterface
import java.util.Collections

/**
 * Manages the lifecycle of Android Local-Only Hotspot (LOHS) via WifiManager.
 *
 * Local-Only Hotspot allows Android to spawn a dedicated Wi-Fi SoftAP without
 * requiring carrier cellular tethering permissions or rooting. This enables
 * high-throughput offline peer-to-peer Wi-Fi transfers with Arch Linux while
 * completely bypassing Realtek RTL8852BE Multi-Channel Concurrency (MCC) bugs.
 */
class LOHSManager(private val context: Context) {

    companion object {
        private const val TAG = "ArchDrop:LOHSManager"
        const val DEFAULT_HTTP_PORT = 53317

        // Android SoftApConfiguration security types
        const val SECURITY_TYPE_OPEN = 0
        const val SECURITY_TYPE_WPA2_PSK = 1
        const val SECURITY_TYPE_WPA3_SAE_TRANSITION = 2
        const val SECURITY_TYPE_WPA3_SAE = 3

        // Hotspot inactivity auto-teardown default (60 seconds) for RTL8852BE hardware safety
        const val DEFAULT_IDLE_TIMEOUT_MS = 60_000L
    }

    data class LohsCredentials(
        val ssid: String,
        val passphrase: String?,
        val securityType: Int,
        val gatewayIp: String,
        val httpPort: Int = DEFAULT_HTTP_PORT
    )

    sealed class HotspotState {
        object Idle : HotspotState()
        object Starting : HotspotState()
        data class Active(val credentials: LohsCredentials) : HotspotState()
        data class Failed(val reasonCode: Int, val message: String) : HotspotState()
    }

    private val wifiManager: WifiManager? =
        context.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager

    private val mainHandler = Handler(Looper.getMainLooper())

    private val _state = MutableStateFlow<HotspotState>(HotspotState.Idle)
    val state: StateFlow<HotspotState> = _state.asStateFlow()

    private var activeReservation: WifiManager.LocalOnlyHotspotReservation? = null
    private val idleTimeoutRunnable = Runnable {
        Log.i(TAG, "LOHS idle timeout elapsed; shutting down SoftAP to preserve battery")
        stopHotspot()
    }

    @Synchronized
    fun startHotspot(onResult: ((Boolean, LohsCredentials?) -> Unit)? = null) {
        cancelIdleTimeout()

        if (activeReservation != null && _state.value is HotspotState.Active) {
            val creds = (_state.value as HotspotState.Active).credentials
            Log.i(TAG, "LOHS already active: SSID=${creds.ssid}, IP=${creds.gatewayIp}")
            onResult?.invoke(true, creds)
            return
        }

        if (wifiManager == null) {
            val err = "WifiManager service is not available on this device"
            Log.e(TAG, err)
            _state.value = HotspotState.Failed(-1, err)
            onResult?.invoke(false, null)
            return
        }

        _state.value = HotspotState.Starting
        Log.i(TAG, "Calling WifiManager.startLocalOnlyHotspot...")

        try {
            wifiManager.startLocalOnlyHotspot(object : WifiManager.LocalOnlyHotspotCallback() {
                override fun onStarted(reservation: WifiManager.LocalOnlyHotspotReservation) {
                    super.onStarted(reservation)
                    Log.i(TAG, "LocalOnlyHotspot callback: onStarted")
                    synchronized(this@LOHSManager) {
                        activeReservation = reservation
                        val creds = extractCredentials(reservation)
                        if (creds != null) {
                            Log.i(TAG, "LOHS active: SSID=${creds.ssid}, Gateway=${creds.gatewayIp}, SecType=${creds.securityType}")
                            _state.value = HotspotState.Active(creds)
                            scheduleIdleTimeout(DEFAULT_IDLE_TIMEOUT_MS)
                            onResult?.invoke(true, creds)
                        } else {
                            val err = "Failed to extract SSID and passphrase from Hotspot Reservation"
                            Log.e(TAG, err)
                            _state.value = HotspotState.Failed(-2, err)
                            onResult?.invoke(false, null)
                        }
                    }
                }

                override fun onStopped() {
                    super.onStopped()
                    Log.i(TAG, "LocalOnlyHotspot callback: onStopped")
                    synchronized(this@LOHSManager) {
                        activeReservation = null
                        _state.value = HotspotState.Idle
                    }
                }

                override fun onFailed(reason: Int) {
                    super.onFailed(reason)
                    val msg = when (reason) {
                        ERROR_NO_CHANNEL -> "ERROR_NO_CHANNEL: No supported Wi-Fi channel available"
                        ERROR_GENERIC -> "ERROR_GENERIC: Internal framework error starting LOHS"
                        ERROR_INCOMPATIBLE_MODE -> "ERROR_INCOMPATIBLE_MODE: Wi-Fi tethering conflict or inactive Wi-Fi"
                        ERROR_TETHERING_DISALLOWED -> "ERROR_TETHERING_DISALLOWED: Tethering disallowed by policy"
                        else -> "UNKNOWN_LOHS_ERROR ($reason)"
                    }
                    Log.e(TAG, "LocalOnlyHotspot callback: onFailed: $msg")
                    synchronized(this@LOHSManager) {
                        activeReservation = null
                        _state.value = HotspotState.Failed(reason, msg)
                        onResult?.invoke(false, null)
                    }
                }
            }, mainHandler)
        } catch (e: SecurityException) {
            Log.e(TAG, "SecurityException: Missing permissions to start LocalOnlyHotspot", e)
            _state.value = HotspotState.Failed(-3, "Permission denied: ${e.message}")
            onResult?.invoke(false, null)
        } catch (e: Exception) {
            Log.e(TAG, "Unexpected exception starting LocalOnlyHotspot", e)
            _state.value = HotspotState.Failed(-4, "Exception: ${e.message}")
            onResult?.invoke(false, null)
        }
    }

    @Synchronized
    fun stopHotspot() {
        cancelIdleTimeout()
        if (activeReservation != null) {
            Log.i(TAG, "Closing active LocalOnlyHotspotReservation...")
            try {
                activeReservation?.close()
            } catch (e: Exception) {
                Log.w(TAG, "Exception while closing LocalOnlyHotspotReservation", e)
            }
            activeReservation = null
        }
        _state.value = HotspotState.Idle
    }

    fun isHotspotActive(): Boolean {
        return activeReservation != null && _state.value is HotspotState.Active
    }

    fun resetIdleTimeout(timeoutMs: Long = DEFAULT_IDLE_TIMEOUT_MS) {
        cancelIdleTimeout()
        if (isHotspotActive()) {
            scheduleIdleTimeout(timeoutMs)
        }
    }

    private fun scheduleIdleTimeout(timeoutMs: Long) {
        mainHandler.postDelayed(idleTimeoutRunnable, timeoutMs)
    }

    private fun cancelIdleTimeout() {
        mainHandler.removeCallbacks(idleTimeoutRunnable)
    }

    private fun extractCredentials(reservation: WifiManager.LocalOnlyHotspotReservation): LohsCredentials? {
        val gatewayIp = resolveHotspotIp()

        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val config = reservation.softApConfiguration ?: return null
            @Suppress("DEPRECATION")
            val ssid = config.ssid ?: return null
            val passphrase = config.passphrase
            val securityType = config.securityType
            LohsCredentials(
                ssid = ssid,
                passphrase = passphrase,
                securityType = securityType,
                gatewayIp = gatewayIp
            )
        } else {
            @Suppress("DEPRECATION")
            val config = reservation.wifiConfiguration ?: return null
            @Suppress("DEPRECATION")
            val rawSsid = config.SSID ?: return null
            val ssid = unquote(rawSsid)
            @Suppress("DEPRECATION")
            val rawKey = config.preSharedKey
            val passphrase = rawKey?.let { unquote(it) }
            LohsCredentials(
                ssid = ssid,
                passphrase = passphrase,
                securityType = SECURITY_TYPE_WPA2_PSK,
                gatewayIp = gatewayIp
            )
        }
    }

    private fun unquote(str: String): String {
        return if (str.startsWith("\"") && str.endsWith("\"") && str.length >= 2) {
            str.substring(1, str.length - 1)
        } else {
            str
        }
    }

    fun resolveHotspotIp(): String {
        try {
            val interfaces = Collections.list(NetworkInterface.getNetworkInterfaces())
            for (intf in interfaces) {
                val name = intf.name.lowercase()
                if (name.startsWith("ap") || name.startsWith("softap") || name.startsWith("wlan") || name.startsWith("swlan")) {
                    val addrs = Collections.list(intf.inetAddresses)
                    for (addr in addrs) {
                        if (!addr.isLoopbackAddress && addr is Inet4Address) {
                            val host = addr.hostAddress ?: continue
                            if (host.startsWith("192.168.") || host.startsWith("10.")) {
                                return host
                            }
                        }
                    }
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "Could not determine local hotspot interface IP address", e)
        }
        return "192.168.43.1"
    }
}
