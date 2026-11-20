//! Gateway IP resolution for Android Local-Only Hotspot (LOHS).
//!
//! Provides a three-tier deterministic resolution strategy:
//! 1. Tier 1: `IP4Config.Gateway` property announced by NetworkManager.
//! 2. Tier 2: `DHCP4Config.Options` containing `dhcp_server_identifier` or `routers`.
//! 3. Tier 3: Subnet calculation from local assigned IP and prefix (e.g. first usable host `192.168.x.1`).

use crate::error::WirelessError;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::ops::Deref;
use tracing::{debug, warn};
use zbus::zvariant::{OwnedValue, Value};

/// Resolved network addressing information for the station link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGatewayInfo {
    /// Peer Android LOHS gateway IP address (to dial for LocalSend transfer)
    pub gateway_ip: Ipv4Addr,
    /// Local IP assigned to wlan0 on the LOHS subnet
    pub local_ip: Ipv4Addr,
    /// Subnet CIDR prefix length (e.g. 24)
    pub prefix: u32,
    /// Resolution tier used (1, 2, or 3)
    pub tier: u8,
}

/// Calculate the default gateway (first usable host) in a given IPv4 subnet.
///
/// For standard /24 subnets (common across Android vendors), `192.168.43.15/24` -> `192.168.43.1`.
/// Supports CIDR prefixes 8..=30.
pub fn calculate_subnet_gateway(local_ip: Ipv4Addr, prefix: u32) -> Result<Ipv4Addr, WirelessError> {
    if !(8..=30).contains(&prefix) {
        return Err(WirelessError::GatewayResolutionFailed(format!(
            "Unsupported IPv4 prefix length: {prefix}"
        )));
    }

    let ip_u32 = u32::from(local_ip);
    let mask = if prefix == 0 {
        0
    } else {
        !((1u32 << (32 - prefix)) - 1)
    };
    let network = ip_u32 & mask;
    let gateway_u32 = network + 1;

    // Verify gateway is not the local IP itself
    let gw = Ipv4Addr::from(gateway_u32);
    Ok(gw)
}

/// Pure resolution function applying the three-tier resolution strategy.
///
/// - `gateway_str`: Value from `IP4Config.Gateway` (Tier 1)
/// - `dhcp_options`: String key-value pairs from `DHCP4Config.Options` (Tier 2)
/// - `local_ip`: Assigned local IP from `IP4Config.AddressData` (Tier 3 fallback)
/// - `prefix`: Subnet prefix length (e.g. 24)
pub fn resolve_gateway_from_data(
    gateway_str: Option<&str>,
    dhcp_options: &HashMap<String, String>,
    local_ip: Ipv4Addr,
    prefix: u32,
) -> Result<ResolvedGatewayInfo, WirelessError> {
    // Tier 1: Check IP4Config Gateway property
    if let Some(gw_raw) = gateway_str {
        let trimmed = gw_raw.trim();
        if !trimmed.is_empty() && trimmed != "0.0.0.0" {
            if let Ok(gw_ip) = trimmed.parse::<Ipv4Addr>() {
                if !gw_ip.is_unspecified() && !gw_ip.is_broadcast() {
                    debug!("Resolved gateway via Tier 1 (IP4Config.Gateway): {gw_ip}");
                    return Ok(ResolvedGatewayInfo {
                        gateway_ip: gw_ip,
                        local_ip,
                        prefix,
                        tier: 1,
                    });
                }
            }
        }
    }

    // Tier 2: Check DHCP4Config options (dhcp_server_identifier, routers)
    for key in &["dhcp_server_identifier", "routers", "server_id"] {
        if let Some(val) = dhcp_options.get(*key) {
            // Options might contain space-separated or comma-separated lists of routers
            for part in val.split_whitespace().flat_map(|s| s.split(',')) {
                let trimmed = part.trim();
                if let Ok(gw_ip) = trimmed.parse::<Ipv4Addr>() {
                    if !gw_ip.is_unspecified() && !gw_ip.is_broadcast() {
                        debug!("Resolved gateway via Tier 2 (DHCP4Config '{key}'): {gw_ip}");
                        return Ok(ResolvedGatewayInfo {
                            gateway_ip: gw_ip,
                            local_ip,
                            prefix,
                            tier: 2,
                        });
                    }
                }
            }
        }
    }

    // Tier 3: Subnet host calculation
    let calculated = calculate_subnet_gateway(local_ip, prefix)?;
    warn!(
        "Tier 1 and Tier 2 resolution empty; fell back to Tier 3 subnet calculation: {calculated}"
    );
    Ok(ResolvedGatewayInfo {
        gateway_ip: calculated,
        local_ip,
        prefix,
        tier: 3,
    })
}

/// Extract primary local IP address and prefix from NetworkManager `AddressData` property (`aa{sv}`).
pub fn parse_address_data(
    address_data: &[HashMap<String, OwnedValue>],
) -> Option<(Ipv4Addr, u32)> {
    for addr_entry in address_data {
        let ip_str = addr_entry.get("address").and_then(|v| match v.deref() {
            Value::Str(s) => Some(s.as_str()),
            _ => None,
        })?;

        let prefix = addr_entry
            .get("prefix")
            .and_then(|v| match v.deref() {
                Value::U32(p) => Some(*p),
                Value::I32(p) => Some(*p as u32),
                _ => None,
            })
            .unwrap_or(24);

        if let Ok(ip) = ip_str.parse::<Ipv4Addr>() {
            if !ip.is_unspecified() && !ip.is_loopback() {
                return Some((ip, prefix));
            }
        }
    }
    None
}

/// Convert D-Bus `DHCP4Config.Options` dictionary (`a{sv}`) into string map.
pub fn parse_dhcp_options(
    options: &HashMap<String, OwnedValue>,
) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (k, v) in options {
        let s = match v.deref() {
            Value::Str(s) => s.to_string(),
            Value::U32(n) => n.to_string(),
            Value::I32(n) => n.to_string(),
            _ => continue,
        };
        map.insert(k.clone(), s);
    }
    map
}
