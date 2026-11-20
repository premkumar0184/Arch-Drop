package org.archdrop.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LOHSManagerTest {

    @Test
    fun testDefaultIdleTimeoutIs60Seconds() {
        assertEquals(
            "DEFAULT_IDLE_TIMEOUT_MS must be 60,000L (60 seconds) for RTL8852BE hardware safety",
            60_000L,
            LOHSManager.DEFAULT_IDLE_TIMEOUT_MS
        )
    }

    @Test
    fun testLohsCredentialsProperties() {
        val creds = LOHSManager.LohsCredentials(
            ssid = "ArchDrop_Test",
            passphrase = "password_123",
            securityType = LOHSManager.SECURITY_TYPE_WPA2_PSK,
            gatewayIp = "192.168.43.1"
        )
        assertEquals("ArchDrop_Test", creds.ssid)
        assertEquals("password_123", creds.passphrase)
        assertEquals(LOHSManager.SECURITY_TYPE_WPA2_PSK, creds.securityType)
        assertEquals("192.168.43.1", creds.gatewayIp)
        assertEquals(53317, creds.httpPort)
    }

    @Test
    fun testHotspotStateTransitions() {
        val creds = LOHSManager.LohsCredentials(
            ssid = "ArchDrop_Active",
            passphrase = "test_passphrase",
            securityType = LOHSManager.SECURITY_TYPE_WPA3_SAE,
            gatewayIp = "192.168.43.1"
        )
        val activeState = LOHSManager.HotspotState.Active(creds)
        assertEquals(creds, activeState.credentials)

        val failedState = LOHSManager.HotspotState.Failed(2, "Generic Failure")
        assertEquals(2, failedState.reasonCode)
        assertEquals("Generic Failure", failedState.message)

        val idle: LOHSManager.HotspotState = LOHSManager.HotspotState.Idle
        val starting: LOHSManager.HotspotState = LOHSManager.HotspotState.Starting
        assertNotNull(idle)
        assertNotNull(starting)
    }
}
