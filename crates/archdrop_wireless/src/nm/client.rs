//! NetworkManager D-Bus station controller client and active LOHS session manager.
//!
//! Interacts with NetworkManager via system D-Bus (`zbus 5.0`) to manage 802.11 station mode,
//! perform targeted scans, add volatile in-memory connection profiles, and monitor activation.

use super::gateway::{
    parse_address_data, parse_dhcp_options, resolve_gateway_from_data, ResolvedGatewayInfo,
};
use super::profile::ConnectionProfileBuilder;
use super::types::{
    AccessPointInfo, ActiveConnectionState, LohsConnectionConfig, SecurityType,
    NM_ACTIVE_CONNECTION_STATE_DEACTIVATED, NM_DEVICE_TYPE_WIFI,
};
use crate::error::WirelessError;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};
use zbus::proxy;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::Connection;

#[proxy(
    default_service = "org.freedesktop.NetworkManager",
    interface = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager"
)]
pub trait NetworkManager {
    /// Add an in-memory connection and activate it immediately.
    fn add_and_activate_connection2(
        &self,
        connection: &HashMap<String, HashMap<String, Value<'_>>>,
        device: &ObjectPath<'_>,
        specific_object: &ObjectPath<'_>,
        options: &HashMap<String, Value<'_>>,
    ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath, HashMap<String, OwnedValue>)>;

    /// Deactivate an active connection by object path.
    fn deactivate_connection(&self, active_connection: &ObjectPath<'_>) -> zbus::Result<()>;

    /// Get device path by interface name (e.g. "wlan0").
    fn get_device_by_ip_iface(&self, iface: &str) -> zbus::Result<OwnedObjectPath>;

    /// Get list of all NetworkManager device paths.
    fn get_devices(&self) -> zbus::Result<Vec<OwnedObjectPath>>;

    /// Activate an existing connection profile.
    fn activate_connection(
        &self,
        connection: &ObjectPath<'_>,
        device: &ObjectPath<'_>,
        specific_object: &ObjectPath<'_>,
    ) -> zbus::Result<OwnedObjectPath>;
}

#[proxy(
    default_service = "org.freedesktop.NetworkManager",
    interface = "org.freedesktop.NetworkManager.Device"
)]
pub trait Device {
    #[zbus(property)]
    fn device_type(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn interface(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn active_connection(&self) -> zbus::Result<OwnedObjectPath>;
}

#[proxy(
    default_service = "org.freedesktop.NetworkManager",
    interface = "org.freedesktop.NetworkManager.Device.Wireless"
)]
pub trait DeviceWireless {
    /// Request an active Wi-Fi scan with optional SSID filter.
    fn request_scan(&self, options: &HashMap<&str, Value<'_>>) -> zbus::Result<()>;

    /// Retrieve all access points known to this wireless device.
    fn get_all_access_points(&self) -> zbus::Result<Vec<OwnedObjectPath>>;

    #[zbus(property)]
    fn last_scan(&self) -> zbus::Result<i64>;
}

#[proxy(
    default_service = "org.freedesktop.NetworkManager",
    interface = "org.freedesktop.NetworkManager.AccessPoint"
)]
pub trait AccessPoint {
    #[zbus(property)]
    fn ssid(&self) -> zbus::Result<Vec<u8>>;

    #[zbus(property)]
    fn hw_address(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn frequency(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn strength(&self) -> zbus::Result<u8>;

    #[zbus(property)]
    fn rsn_flags(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn wpa_flags(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn flags(&self) -> zbus::Result<u32>;
}

#[proxy(
    default_service = "org.freedesktop.NetworkManager",
    interface = "org.freedesktop.NetworkManager.Connection.Active"
)]
pub trait ActiveConnection {
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn connection(&self) -> zbus::Result<OwnedObjectPath>;

    #[zbus(property)]
    fn ip4_config(&self) -> zbus::Result<OwnedObjectPath>;

    #[zbus(property)]
    fn dhcp4_config(&self) -> zbus::Result<OwnedObjectPath>;

    #[zbus(property)]
    fn default(&self) -> zbus::Result<bool>;
}

#[proxy(
    default_service = "org.freedesktop.NetworkManager",
    interface = "org.freedesktop.NetworkManager.IP4Config"
)]
pub trait IP4Config {
    #[zbus(property)]
    fn gateway(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn address_data(&self) -> zbus::Result<Vec<HashMap<String, OwnedValue>>>;
}

#[proxy(
    default_service = "org.freedesktop.NetworkManager",
    interface = "org.freedesktop.NetworkManager.DHCP4Config"
)]
pub trait DHCP4Config {
    #[zbus(property)]
    fn options(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
}

/// NetworkManager station controller for scanning and activating LOHS connections.
pub struct NmStationController {
    connection: Connection,
    nm_proxy: NetworkManagerProxy<'static>,
}

impl NmStationController {
    /// Connect to the system D-Bus and initialize the NetworkManager proxy.
    pub async fn new() -> Result<Self, WirelessError> {
        let connection = Connection::system().await.map_err(|e| {
            WirelessError::NetworkManagerUnavailable(format!(
                "Failed to connect to system D-Bus: {e}"
            ))
        })?;

        let nm_proxy = NetworkManagerProxy::new(&connection).await.map_err(|e| {
            WirelessError::NetworkManagerUnavailable(format!(
                "NetworkManager D-Bus service is not responding: {e}"
            ))
        })?;

        Ok(Self {
            connection,
            nm_proxy,
        })
    }

    /// Access the underlying zbus D-Bus connection.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Identify the primary managed 802.11 wireless device path.
    ///
    /// If `iface_hint` is provided (e.g. `"wlan0"`), looks up that specific interface.
    /// Otherwise enumerates devices and selects the first managed Wi-Fi device (`DeviceType == 2`).
    pub async fn find_wifi_device(&self, iface_hint: Option<&str>) -> Result<OwnedObjectPath, WirelessError> {
        if let Some(iface) = iface_hint {
            let dev_path = self
                .nm_proxy
                .get_device_by_ip_iface(iface)
                .await
                .map_err(|e| {
                    WirelessError::WirelessDeviceNotFound(format!(
                        "Interface '{iface}' not found in NetworkManager: {e}"
                    ))
                })?;

            let dev_proxy = DeviceProxy::builder(&self.connection)
                .path(&dev_path)?
                .build()
                .await?;
            let dev_type = dev_proxy.device_type().await?;
            if dev_type == NM_DEVICE_TYPE_WIFI {
                return Ok(dev_path);
            } else {
                return Err(WirelessError::WirelessDeviceNotFound(format!(
                    "Interface '{iface}' is not an 802.11 Wi-Fi device (type: {dev_type})"
                )));
            }
        }

        let devices = self.nm_proxy.get_devices().await?;
        for dev_path in devices {
            if let Ok(dev_proxy) = DeviceProxy::builder(&self.connection).path(&dev_path)?.build().await {
                if let Ok(dev_type) = dev_proxy.device_type().await {
                    if dev_type == NM_DEVICE_TYPE_WIFI {
                        let name = dev_proxy.interface().await.unwrap_or_default();
                        debug!(
                            "Found managed Wi-Fi device: {} ({})",
                            name,
                            dev_path.as_str()
                        );
                        return Ok(dev_path);
                    }
                }
            }
        }

        Err(WirelessError::WirelessDeviceNotFound(
            "No managed 802.11 wireless device found on host".to_string(),
        ))
    }

    /// Retrieve all currently detected Access Points on the given wireless device.
    pub async fn list_access_points(
        &self,
        device_path: &ObjectPath<'_>,
    ) -> Result<Vec<AccessPointInfo>, WirelessError> {
        let wifi_proxy = DeviceWirelessProxy::builder(&self.connection)
            .path(device_path)?
            .build()
            .await?;

        let ap_paths = wifi_proxy.get_all_access_points().await?;
        let mut results = Vec::new();

        for ap_path in ap_paths {
            if let Ok(ap_proxy) = AccessPointProxy::builder(&self.connection).path(&ap_path)?.build().await {
                let ssid_bytes = ap_proxy.ssid().await.unwrap_or_default();
                let ssid = String::from_utf8_lossy(&ssid_bytes).trim_matches('\0').to_string();
                let bssid = ap_proxy.hw_address().await.unwrap_or_default();
                let freq = ap_proxy.frequency().await.unwrap_or_default();
                let strength = ap_proxy.strength().await.unwrap_or_default();
                let rsn = ap_proxy.rsn_flags().await.unwrap_or_default();
                let wpa = ap_proxy.wpa_flags().await.unwrap_or_default();
                let flags = ap_proxy.flags().await.unwrap_or_default();

                let security = SecurityType::from_ap_flags(flags, wpa, rsn);
                results.push(AccessPointInfo {
                    ssid,
                    bssid,
                    frequency_mhz: freq,
                    strength,
                    security,
                    dbus_path: ap_path,
                });
            }
        }

        Ok(results)
    }

    /// Perform a targeted scan for an Access Point with timeout.
    ///
    /// Checks cached APs first to minimize disruptive off-channel dwell time on single-synthesizer NICs.
    pub async fn scan_access_points(
        &self,
        device_path: &ObjectPath<'_>,
        target_ssid: Option<&str>,
        timeout: Duration,
    ) -> Result<Vec<AccessPointInfo>, WirelessError> {
        let wifi_proxy = DeviceWirelessProxy::builder(&self.connection)
            .path(device_path)?
            .build()
            .await?;

        // 1. Check existing AP cache
        let cached = self.list_access_points(device_path).await?;
        if let Some(target) = target_ssid {
            if let Some(found) = cached.iter().find(|ap| ap.ssid == target) {
                debug!("Target SSID '{target}' already present in NM cache");
                return Ok(vec![found.clone()]);
            }
        }

        // 2. Trigger targeted active scan
        let mut scan_options = HashMap::new();
        if let Some(target) = target_ssid {
            let ssids_arr = vec![target.as_bytes().to_vec()];
            scan_options.insert("ssids", Value::from(ssids_arr));
        }

        let scan_res = wifi_proxy.request_scan(&scan_options).await;
        if let Err(e) = scan_res {
            warn!("RequestScan returned error (scan may already be in progress): {e}");
        }

        // 3. Poll for AP visibility
        let start = Instant::now();
        while start.elapsed() < timeout {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let aps = self.list_access_points(device_path).await?;
            if let Some(target) = target_ssid {
                if let Some(found) = aps.iter().find(|ap| ap.ssid == target) {
                    debug!("Found target SSID '{target}' after {:?} of scanning", start.elapsed());
                    return Ok(vec![found.clone()]);
                }
            } else if !aps.is_empty() {
                return Ok(aps);
            }
        }

        if let Some(target) = target_ssid {
            Err(WirelessError::AccessPointNotFound(target.to_string()))
        } else {
            Err(WirelessError::DiscoveryFailed(format!(
                "Scan timed out after {timeout:?} with no APs discovered"
            )))
        }
    }

    /// Establish a transient station association to an Android Local-Only Hotspot.
    ///
    /// Performs in-memory volatile profile creation, waits for link activation,
    /// and dynamically resolves peer gateway IP address.
    pub async fn connect_lohs(
        &self,
        device_path: &ObjectPath<'_>,
        config: &LohsConnectionConfig,
        timeout: Duration,
    ) -> Result<ActiveLohsSession, WirelessError> {
        info!(
            "Initiating LOHS station association to SSID '{}' (security: {:?})",
            config.ssid, config.security
        );

        // Record any active connection on device to allow potential state restoration
        let dev_proxy = DeviceProxy::builder(&self.connection)
            .path(device_path)?
            .build()
            .await?;
        let prev_active_conn = dev_proxy
            .active_connection()
            .await
            .ok()
            .filter(|p| p.as_str() != "/");

        let prev_settings_path = if let Some(ref active_path) = prev_active_conn {
            if let Ok(builder) = ActiveConnectionProxy::builder(&self.connection).path(active_path) {
                if let Ok(act_proxy) = builder.build().await {
                    act_proxy.connection().await.ok()
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // Build in-memory connection profile with verified host safety invariants
        let builder = ConnectionProfileBuilder::new(config);
        let settings = builder.build()?;
        let options = ConnectionProfileBuilder::build_options();

        let null_obj = ObjectPath::try_from("/").map_err(|e| WirelessError::Dbus(zbus::Error::from(e)))?;

        let (conn_path, active_conn_path, _) = self
            .nm_proxy
            .add_and_activate_connection2(&settings, device_path, &null_obj, &options)
            .await
            .map_err(|e| {
                WirelessError::ConnectionActivationFailed(format!(
                    "AddAndActivateConnection2 failed: {e}"
                ))
            })?;

        debug!(
            "Transient profile added: {} (active: {})",
            conn_path.as_str(),
            active_conn_path.as_str()
        );

        // Monitor connection state until Activated or timeout
        let active_proxy = ActiveConnectionProxy::builder(&self.connection)
            .path(&active_conn_path)?
            .build()
            .await?;

        let start = Instant::now();
        let mut activated = false;

        while start.elapsed() < timeout {
            let raw_state = active_proxy.state().await.unwrap_or(0);
            let state = ActiveConnectionState::from(raw_state);

            if state.is_activated() {
                activated = true;
                break;
            }

            if raw_state == NM_ACTIVE_CONNECTION_STATE_DEACTIVATED {
                let _ = self.nm_proxy.deactivate_connection(&active_conn_path).await;
                return Err(WirelessError::ConnectionActivationFailed(format!(
                    "Connection transitioned to deactivated state prematurely (state {raw_state})"
                )));
            }

            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        if !activated {
            let _ = self.nm_proxy.deactivate_connection(&active_conn_path).await;
            return Err(WirelessError::ConnectionActivationFailed(format!(
                "Connection activation timed out after {timeout:?}"
            )));
        }

        info!("LOHS station connection activated, resolving gateway IP...");

        // Resolve gateway and local assigned IP via IP4Config / DHCP4Config
        let resolved = self
            .resolve_gateway(&active_conn_path, Duration::from_secs(8))
            .await?;

        info!(
            "LOHS session ready: peer gateway = {} (tier {}), local IP = {}",
            resolved.gateway_ip, resolved.tier, resolved.local_ip
        );

        Ok(ActiveLohsSession::new(
            self.connection.clone(),
            conn_path,
            active_conn_path,
            resolved.gateway_ip,
            resolved.local_ip,
            prev_settings_path,
            device_path.to_owned().into(),
        ))
    }

    /// Dynamically resolve peer gateway IP and assigned IP from an active connection.
    pub async fn resolve_gateway(
        &self,
        active_conn_path: &ObjectPath<'_>,
        timeout: Duration,
    ) -> Result<ResolvedGatewayInfo, WirelessError> {
        let active_proxy = ActiveConnectionProxy::builder(&self.connection)
            .path(active_conn_path)?
            .build()
            .await?;

        let start = Instant::now();
        while start.elapsed() < timeout {
            let ip4_path = active_proxy
                .ip4_config()
                .await
                .unwrap_or_else(|_| ObjectPath::try_from("/").unwrap().into());

            if ip4_path.as_str() != "/" {
                if let Ok(ip4_proxy) = IP4ConfigProxy::builder(&self.connection)
                    .path(&ip4_path)?
                    .build()
                    .await
                {
                    let address_data = ip4_proxy.address_data().await.unwrap_or_default();
                    let gateway_str = ip4_proxy.gateway().await.ok();

                    if let Some((local_ip, prefix)) = parse_address_data(&address_data) {
                        let dhcp_path = active_proxy
                            .dhcp4_config()
                            .await
                            .unwrap_or_else(|_| ObjectPath::try_from("/").unwrap().into());

                        let mut dhcp_options = HashMap::new();
                        if dhcp_path.as_str() != "/" {
                            if let Ok(dhcp_proxy) = DHCP4ConfigProxy::builder(&self.connection)
                                .path(&dhcp_path)?
                                .build()
                                .await
                            {
                                if let Ok(opts) = dhcp_proxy.options().await {
                                    dhcp_options = parse_dhcp_options(&opts);
                                }
                            }
                        }

                        return resolve_gateway_from_data(
                            gateway_str.as_deref(),
                            &dhcp_options,
                            local_ip,
                            prefix,
                        );
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        Err(WirelessError::DhcpTimeout(format!(
            "Timed out after {timeout:?} waiting for IP4Config / DHCP assignment"
        )))
    }

    /// Deactivate an active connection by path.
    pub async fn deactivate(&self, active_path: &ObjectPath<'_>) -> Result<(), WirelessError> {
        self.nm_proxy
            .deactivate_connection(active_path)
            .await
            .map_err(WirelessError::Dbus)
    }
}

/// RAII session guard for an active LOHS connection.
///
/// Ensures the transient connection profile is deactivated and cleaned up
/// upon explicit call to `deactivate()` or when dropped.
pub struct ActiveLohsSession {
    connection: Option<Connection>,
    connection_path: OwnedObjectPath,
    active_path: OwnedObjectPath,
    gateway_ip: Ipv4Addr,
    local_ip: Ipv4Addr,
    prev_settings_path: Option<OwnedObjectPath>,
    device_path: OwnedObjectPath,
    deactivated: bool,
}

impl ActiveLohsSession {
    /// Create a new session wrapper with a live D-Bus connection.
    pub fn new(
        connection: Connection,
        connection_path: OwnedObjectPath,
        active_path: OwnedObjectPath,
        gateway_ip: Ipv4Addr,
        local_ip: Ipv4Addr,
        prev_settings_path: Option<OwnedObjectPath>,
        device_path: OwnedObjectPath,
    ) -> Self {
        Self {
            connection: Some(connection),
            connection_path,
            active_path,
            gateway_ip,
            local_ip,
            prev_settings_path,
            device_path,
            deactivated: false,
        }
    }

    /// Create a mock session wrapper for unit tests without D-Bus connection.
    pub fn new_mock(gateway_ip: Ipv4Addr, local_ip: Ipv4Addr) -> Self {
        let dummy_path = ObjectPath::try_from("/").unwrap().into();
        Self {
            connection: None,
            connection_path: dummy_path,
            active_path: ObjectPath::try_from("/mock/active").unwrap().into(),
            gateway_ip,
            local_ip,
            prev_settings_path: None,
            device_path: ObjectPath::try_from("/mock/device").unwrap().into(),
            deactivated: false,
        }
    }

    /// Resolved gateway IP address of the Android LOHS SoftAP.
    pub fn gateway_ip(&self) -> Ipv4Addr {
        self.gateway_ip
    }

    /// Assigned IP address of the local wireless interface on the LOHS subnet.
    pub fn local_ip(&self) -> Ipv4Addr {
        self.local_ip
    }

    /// D-Bus object path of the active connection.
    pub fn active_path(&self) -> &OwnedObjectPath {
        &self.active_path
    }

    /// D-Bus object path of the connection settings profile.
    pub fn connection_path(&self) -> &OwnedObjectPath {
        &self.connection_path
    }

    /// Previously active connection settings profile prior to LOHS association, if any.
    pub fn prev_connection_path(&self) -> Option<&OwnedObjectPath> {
        self.prev_settings_path.as_ref()
    }

    /// Previously active connection settings profile path for baseline Wi-Fi restoration.
    pub fn prev_settings_path(&self) -> Option<&OwnedObjectPath> {
        self.prev_settings_path.as_ref()
    }

    /// Managed device path for this connection session.
    pub fn device_path(&self) -> &OwnedObjectPath {
        &self.device_path
    }

    /// Gracefully deactivate the connection and release the LOHS session, restoring baseline Wi-Fi.
    pub async fn deactivate(&mut self) -> Result<(), WirelessError> {
        if self.deactivated {
            return Ok(());
        }
        self.deactivated = true;

        info!(
            "Deactivating LOHS session: {}",
            self.active_path.as_str()
        );

        if let Some(ref conn) = self.connection {
            let nm = NetworkManagerProxy::new(conn).await?;
            let _ = nm.deactivate_connection(&self.active_path).await;

            if let Some(ref prev_settings) = self.prev_settings_path {
                info!("Restoring baseline Wi-Fi connection profile: {}", prev_settings.as_str());
                let null_obj = ObjectPath::try_from("/").map_err(|e| WirelessError::Dbus(zbus::Error::from(e)))?;
                if let Err(e) = nm.activate_connection(prev_settings, &self.device_path, &null_obj).await {
                    warn!("Failed to reactivate baseline connection {}: {e}", prev_settings.as_str());
                } else {
                    info!("Successfully reactivated baseline Wi-Fi connection: {}", prev_settings.as_str());
                }
            }
        }

        Ok(())
    }
}

impl Drop for ActiveLohsSession {
    fn drop(&mut self) {
        if !self.deactivated {
            self.deactivated = true;
            if let Some(ref conn) = self.connection {
                let conn = conn.clone();
                let active = self.active_path.clone();
                let prev_settings = self.prev_settings_path.clone();
                let dev = self.device_path.clone();
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        if let Ok(nm) = NetworkManagerProxy::new(&conn).await {
                            let _ = nm.deactivate_connection(&active).await;
                            if let Some(prev) = prev_settings {
                                let null_obj = ObjectPath::try_from("/").unwrap_or_else(|_| ObjectPath::from_str_unchecked("/"));
                                let _ = nm.activate_connection(&prev, &dev, &null_obj).await;
                            }
                        }
                    });
                }
            }
        }
    }
}
