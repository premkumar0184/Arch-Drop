//! Connection profile builder for transient in-memory NetworkManager station profiles.
//!
//! Enforces strict host safety invariants:
//! - ipv4.never-default = true (prohibits replacing system default route)
//! - ipv4.ignore-auto-dns = true (prohibits altering /etc/resolv.conf)
//! - ipv4.route-metric = 2000 (preserves WireGuard and primary interface routing priorities)
//! - ipv6.method = "disabled" (blocks rogue IPv6 router advertisements)
//! - connection.autoconnect = false (prevents unwanted background reconnections)
//! - volatile in-memory storage (persist = "volatile", bind-activation = "dbus-client")

use super::types::{LohsConnectionConfig, SecurityType, DEFAULT_ROUTE_METRIC};
use crate::error::WirelessError;
use std::collections::HashMap;
use uuid::Uuid;
use zbus::zvariant::Value;

/// Builder for transient in-memory NetworkManager 802.11 connection settings dictionaries.
#[derive(Debug, Clone)]
pub struct ConnectionProfileBuilder {
    config: LohsConnectionConfig,
    uuid: Option<String>,
    connection_id: Option<String>,
}

impl ConnectionProfileBuilder {
    /// Create a new profile builder from a LOHS configuration.
    pub fn new(config: &LohsConnectionConfig) -> Self {
        Self {
            config: config.clone(),
            uuid: None,
            connection_id: None,
        }
    }

    /// Explicitly specify the connection UUID (defaults to a random v4 UUID).
    pub fn with_uuid(mut self, uuid: impl Into<String>) -> Self {
        self.uuid = Some(uuid.into());
        self
    }

    /// Explicitly specify the connection ID (defaults to "ArchDrop-LOHS-<ssid>").
    pub fn with_connection_id(mut self, id: impl Into<String>) -> Self {
        self.connection_id = Some(id.into());
        self
    }

    /// Build the complete NetworkManager connection settings dictionary (`a{sa{sv}}`).
    ///
    /// Validates all host safety invariants prior to emission.
    pub fn build(self) -> Result<HashMap<String, HashMap<String, Value<'static>>>, WirelessError> {
        self.validate_invariants()?;

        let mut settings = HashMap::new();

        // 1. Connection setting ("connection")
        let mut s_con = HashMap::new();
        let conn_id = self
            .connection_id
            .unwrap_or_else(|| format!("ArchDrop-LOHS-{}", self.config.ssid));
        let uuid_str = self
            .uuid
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        s_con.insert("id".to_string(), Value::from(conn_id));
        s_con.insert("type".to_string(), Value::from("802-11-wireless"));
        s_con.insert("uuid".to_string(), Value::from(uuid_str));
        s_con.insert("autoconnect".to_string(), Value::from(self.config.autoconnect));
        settings.insert("connection".to_string(), s_con);

        // 2. 802.11 Wireless setting ("802-11-wireless")
        let mut s_wifi = HashMap::new();
        s_wifi.insert(
            "ssid".to_string(),
            Value::from(self.config.ssid.as_bytes().to_vec()),
        );
        s_wifi.insert("mode".to_string(), Value::from("infrastructure"));
        if let Some(ref bssid) = self.config.bssid {
            s_wifi.insert("bssid".to_string(), Value::from(bssid.clone()));
        }
        settings.insert("802-11-wireless".to_string(), s_wifi);

        // 3. Wireless Security setting ("802-11-wireless-security")
        let mut s_sec = HashMap::new();
        match self.config.security {
            SecurityType::Open => {
                // Open network does not require security setting
            }
            SecurityType::Wpa2Psk | SecurityType::Wpa2Wpa3Transition => {
                s_sec.insert("key-mgmt".to_string(), Value::from("wpa-psk"));
                s_sec.insert("psk".to_string(), Value::from(self.config.passphrase.clone()));
            }
            SecurityType::Wpa3Sae => {
                s_sec.insert("key-mgmt".to_string(), Value::from("sae"));
                s_sec.insert("psk".to_string(), Value::from(self.config.passphrase.clone()));
                // PMF (Protected Management Frames) required for WPA3 SAE (value 3 = NM_SETTING_WIRELESS_SECURITY_PMF_REQUIRED)
                s_sec.insert("pmf".to_string(), Value::from(3i32));
            }
        }
        if !s_sec.is_empty() {
            settings.insert("802-11-wireless-security".to_string(), s_sec);
        }

        // 4. IPv4 setting ("ipv4") - CRITICAL HOST SAFETY INVARIANTS
        let mut s_ip4 = HashMap::new();
        s_ip4.insert("method".to_string(), Value::from("auto"));
        s_ip4.insert("never-default".to_string(), Value::from(self.config.never_default));
        s_ip4.insert("ignore-auto-dns".to_string(), Value::from(self.config.ignore_auto_dns));
        s_ip4.insert("route-metric".to_string(), Value::from(self.config.route_metric as i32));
        settings.insert("ipv4".to_string(), s_ip4);

        // 5. IPv6 setting ("ipv6") - CRITICAL HOST SAFETY INVARIANTS
        let mut s_ip6 = HashMap::new();
        let ip6_method = if self.config.disable_ipv6 {
            "disabled"
        } else {
            "ignore"
        };
        s_ip6.insert("method".to_string(), Value::from(ip6_method));
        s_ip6.insert("never-default".to_string(), Value::from(true));
        settings.insert("ipv6".to_string(), s_ip6);

        // Double check generated settings
        Self::verify_safety_invariants(&settings)?;

        Ok(settings)
    }

    /// Build the standard options dictionary for `AddAndActivateConnection2`.
    ///
    /// Configures:
    /// - `persist = "volatile"`: Connection profile lives exclusively in RAM, never written to disk.
    /// - `bind-activation = "dbus-client"`: Profile automatically purged if client disconnects.
    pub fn build_options() -> HashMap<String, Value<'static>> {
        let mut options = HashMap::new();
        options.insert("persist".to_string(), Value::from("volatile"));
        options.insert("bind-activation".to_string(), Value::from("dbus-client"));
        options
    }

    /// Validates configuration parameters against safety constraints before dictionary generation.
    fn validate_invariants(&self) -> Result<(), WirelessError> {
        if !self.config.never_default {
            return Err(WirelessError::SafetyInvariantViolated(
                "never_default must be true to protect the system default gateway".to_string(),
            ));
        }
        if !self.config.ignore_auto_dns {
            return Err(WirelessError::SafetyInvariantViolated(
                "ignore_auto_dns must be true to protect host DNS resolution".to_string(),
            ));
        }
        if self.config.route_metric < DEFAULT_ROUTE_METRIC {
            return Err(WirelessError::SafetyInvariantViolated(format!(
                "route_metric ({}) must be >= {} to prevent overtaking primary routes",
                self.config.route_metric, DEFAULT_ROUTE_METRIC
            )));
        }
        if !self.config.disable_ipv6 {
            return Err(WirelessError::SafetyInvariantViolated(
                "disable_ipv6 must be true to suppress rogue IPv6 router advertisements".to_string(),
            ));
        }
        if self.config.autoconnect {
            return Err(WirelessError::SafetyInvariantViolated(
                "autoconnect must be false for transient LOHS profiles".to_string(),
            ));
        }
        Ok(())
    }

    /// Verify that an arbitrary generated NetworkManager settings dictionary satisfies all safety invariants.
    pub fn verify_safety_invariants(
        settings: &HashMap<String, HashMap<String, Value<'_>>>,
    ) -> Result<(), WirelessError> {
        // 1. Verify ipv4.never-default == true
        let ip4 = settings.get("ipv4").ok_or_else(|| {
            WirelessError::SafetyInvariantViolated("Missing 'ipv4' section in profile".to_string())
        })?;
        match ip4.get("never-default") {
            Some(Value::Bool(true)) => {}
            other => {
                return Err(WirelessError::SafetyInvariantViolated(format!(
                    "ipv4.never-default must be true, got: {other:?}"
                )));
            }
        }

        // 2. Verify ipv4.ignore-auto-dns == true
        match ip4.get("ignore-auto-dns") {
            Some(Value::Bool(true)) => {}
            other => {
                return Err(WirelessError::SafetyInvariantViolated(format!(
                    "ipv4.ignore-auto-dns must be true, got: {other:?}"
                )));
            }
        }

        // 3. Verify ipv4.route-metric >= 2000
        match ip4.get("route-metric") {
            Some(Value::I32(m)) if *m >= DEFAULT_ROUTE_METRIC as i32 => {}
            Some(Value::U32(m)) if *m >= DEFAULT_ROUTE_METRIC => {}
            other => {
                return Err(WirelessError::SafetyInvariantViolated(format!(
                    "ipv4.route-metric must be >= {DEFAULT_ROUTE_METRIC}, got: {other:?}"
                )));
            }
        }

        // 4. Verify ipv6.method == "disabled"
        let ip6 = settings.get("ipv6").ok_or_else(|| {
            WirelessError::SafetyInvariantViolated("Missing 'ipv6' section in profile".to_string())
        })?;
        match ip6.get("method") {
            Some(Value::Str(s)) if s.as_str() == "disabled" => {}
            other => {
                return Err(WirelessError::SafetyInvariantViolated(format!(
                    "ipv6.method must be 'disabled', got: {other:?}"
                )));
            }
        }

        // 5. Verify connection.autoconnect == false
        let conn = settings.get("connection").ok_or_else(|| {
            WirelessError::SafetyInvariantViolated("Missing 'connection' section in profile".to_string())
        })?;
        match conn.get("autoconnect") {
            Some(Value::Bool(false)) => {}
            other => {
                return Err(WirelessError::SafetyInvariantViolated(format!(
                    "connection.autoconnect must be false, got: {other:?}"
                )));
            }
        }

        Ok(())
    }
}
