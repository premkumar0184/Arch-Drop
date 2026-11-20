//! High-level Linux Bluetooth Low Energy controller managing BlueZ session,
//! advertising (Apple 0x05 / 0x0D), central discovery, and GATT credential transactions.

use bluer::adv::{Advertisement, AdvertisementHandle, Type};
use bluer::gatt::local::ApplicationHandle;
use bluer::{Adapter, AdapterEvent, Address, DiscoveryFilter, DiscoveryTransport, Session};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use crate::ble::continuity::hotspot_0x0d::{HotspotStatus0x0E, HotspotWakeup0x0D};
use crate::ble::continuity::proximity_0x05::{ProximityBeacon0x05, APPLE_COMPANY_ID};
use crate::ble::gatt::{
    build_gatt_application, decode_credentials, encode_wakeup_command, BleClipboardPacket,
    GattServerState, ARCHDROP_SERVICE_UUID, CHAR_CLIPBOARD_SYNC_UUID, CHAR_LOHS_CREDENTIALS_UUID,
    CHAR_WAKEUP_COMMAND_UUID,
};
use crate::ble::types::{BleConfig, DiscoveredDevice, HotspotCredentials};
use crate::error::WirelessError;

/// High-level Linux BLE controller leveraging BlueZ via `bluer`.
pub struct BleController {
    /// Active BlueZ D-Bus session.
    session: Session,
    /// Active Bluetooth HCI adapter.
    adapter: Adapter,
    /// Controller configuration.
    config: BleConfig,
    /// Active LE advertisement registration handle.
    active_adv: Option<AdvertisementHandle>,
    /// Active GATT server application registration handle.
    active_app: Option<ApplicationHandle>,
}

impl BleController {
    /// Initializes the BLE controller, establishing a connection to system `bluetoothd`
    /// and ensuring the HCI adapter is powered on.
    ///
    /// If `bluetoothd` is stopped or unreachable, returns `WirelessError::BluetoothServiceUnavailable`.
    pub async fn init(config: BleConfig) -> Result<Self, WirelessError> {
        let session = Session::new()
            .await
            .map_err(|e| WirelessError::BluetoothServiceUnavailable(e.to_string()))?;

        let adapter = match &config.adapter_name {
            Some(name) => session
                .adapter(name)
                .map_err(|e| WirelessError::AdapterNotFound(format!("{name}: {e}")))?,
            None => session
                .default_adapter()
                .await
                .map_err(|e| WirelessError::AdapterNotFound(format!("No default Bluetooth adapter: {e}")))?,
        };

        let adapter_desc = config.adapter_name.as_deref().unwrap_or("default");
        let powered = adapter
            .is_powered()
            .await
            .map_err(|e| WirelessError::AdapterNotFound(format!("Bluetooth adapter '{adapter_desc}' unavailable: {e}")))?;

        if !powered {
            tracing::info!("Bluetooth adapter is powered off; powering on...");
            if let Err(e) = adapter.set_powered(true).await {
                tracing::warn!("Failed to power on Bluetooth adapter: {e}");
            }
        }

        Ok(Self {
            session,
            adapter,
            config,
            active_adv: None,
            active_app: None,
        })
    }

    /// Returns a reference to the active `bluer::Adapter`.
    pub fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    /// Returns a reference to the active `bluer::Session`.
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Returns the current configuration.
    pub fn config(&self) -> &BleConfig {
        &self.config
    }

    /// Stops any currently active LE advertisement.
    pub fn stop_advertising(&mut self) {
        self.active_adv = None;
    }

    /// Starts advertising the primary Arch Drop GATT Service UUID.
    pub async fn advertise_service(
        &mut self,
        service_uuid: Uuid,
        local_name: Option<String>,
    ) -> Result<(), WirelessError> {
        self.stop_advertising();

        let mut service_uuids = BTreeSet::new();
        service_uuids.insert(service_uuid);

        let adv = Advertisement {
            advertisement_type: Type::Peripheral,
            service_uuids,
            discoverable: Some(true),
            local_name,
            ..Default::default()
        };

        let handle = self
            .adapter
            .advertise(adv)
            .await
            .map_err(|e| WirelessError::AdvertisingFailed(format!("Failed to register service advertisement: {e}")))?;

        self.active_adv = Some(handle);
        tracing::info!(%service_uuid, "Started BLE GATT service advertisement");
        Ok(())
    }

    /// Starts advertising an Apple 0x05 Proximity Presence Beacon.
    ///
    /// Registers a legacy BLE advertising instance with 22-byte Manufacturer Data (Company ID 0x004C).
    pub async fn advertise_presence(&mut self, beacon: ProximityBeacon0x05) -> Result<(), WirelessError> {
        self.stop_advertising();

        let mut manufacturer_data = BTreeMap::new();
        let (company_id, payload) = beacon.to_bluer_payload();
        manufacturer_data.insert(company_id, payload);

        let adv = Advertisement {
            advertisement_type: Type::Peripheral,
            manufacturer_data,
            discoverable: Some(true),
            ..Default::default()
        };

        let handle = self
            .adapter
            .advertise(adv)
            .await
            .map_err(|e| WirelessError::AdvertisingFailed(format!("Failed to register 0x05 presence beacon: {e}")))?;

        self.active_adv = Some(handle);
        tracing::debug!(salt = beacon.salt, "Started BLE 0x05 presence advertisement");
        Ok(())
    }

    /// Transmits an Apple 0x0D Remote Hotspot Wakeup Trigger to awaken the Android companion device.
    pub async fn send_hotspot_wakeup(&mut self, wakeup: HotspotWakeup0x0D) -> Result<(), WirelessError> {
        self.stop_advertising();

        let mut manufacturer_data = BTreeMap::new();
        let (company_id, payload) = wakeup.to_bluer_payload();
        manufacturer_data.insert(company_id, payload);

        let adv = Advertisement {
            advertisement_type: Type::Peripheral,
            manufacturer_data,
            discoverable: Some(true),
            ..Default::default()
        };

        let handle = self
            .adapter
            .advertise(adv)
            .await
            .map_err(|e| WirelessError::AdvertisingFailed(format!("Failed to register 0x0D wakeup trigger: {e}")))?;

        self.active_adv = Some(handle);
        tracing::info!(port = wakeup.transfer_port, "Started BLE 0x0D hotspot wakeup advertisement");
        Ok(())
    }

    /// Begins scanning for nearby companion devices advertising 0x05 presence, 0x0D wakeup, or 0x0E status.
    ///
    /// Emits parsed `DiscoveredDevice` events through the returned receiver channel.
    pub async fn scan_for_peers(&self) -> Result<mpsc::Receiver<DiscoveredDevice>, WirelessError> {
        let filter = DiscoveryFilter {
            transport: DiscoveryTransport::Le,
            duplicate_data: true,
            discoverable: false,
            ..Default::default()
        };

        self.adapter
            .set_discovery_filter(filter)
            .await
            .map_err(|e| WirelessError::DiscoveryFailed(format!("Failed to set discovery filter: {e}")))?;

        let mut stream = self
            .adapter
            .discover_devices()
            .await
            .map_err(|e| WirelessError::DiscoveryFailed(format!("Failed to initiate LE discovery: {e}")))?;

        let (tx, rx) = mpsc::channel(64);
        let adapter = self.adapter.clone();

        tokio::spawn(async move {
            // 1. Immediately inspect devices already known to the adapter cache that are actively connected or advertising
            if let Ok(addrs) = adapter.device_addresses().await {
                for addr in addrs {
                    if addr == Address::any() {
                        continue;
                    }
                    if let Ok(dev) = adapter.device(addr) {
                        let is_conn = dev.is_connected().await.unwrap_or(false);
                        let has_rssi = dev.rssi().await.ok().flatten().is_some();
                        if !is_conn && !has_rssi {
                            continue;
                        }
                    }
                    if let Some(discovered) = inspect_ble_device(&adapter, addr).await {
                        if tx.send(discovered).await.is_err() {
                            return;
                        }
                    }
                }
            }

            // 2. Process incoming discovery events from active LE scanning
            while let Some(event) = stream.next().await {
                if let AdapterEvent::DeviceAdded(addr) = event {
                    if addr == Address::any() {
                        continue;
                    }
                    if let Some(discovered) = inspect_ble_device(&adapter, addr).await {
                        if tx.send(discovered).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(rx)
    }

    /// Connects to a remote peer device over BLE GATT and retrieves Local-Only Hotspot credentials.
    pub async fn fetch_hotspot_credentials(&self, peer_addr: Address) -> Result<HotspotCredentials, WirelessError> {
        if peer_addr == Address::any() {
            return Err(WirelessError::DeviceNotFound("Refusing connection to 00:00:00:00:00:00".into()));
        }
        let dev = self
            .adapter
            .device(peer_addr)
            .map_err(|e| WirelessError::DeviceNotFound(format!("{peer_addr}: {e}")))?;

        if !dev.is_connected().await.unwrap_or(false) {
            tokio::time::timeout(std::time::Duration::from_secs(4), dev.connect())
                .await
                .map_err(|_| WirelessError::ConnectionFailed(peer_addr.to_string(), "Connection timed out after 4s".into()))?
                .map_err(|e| WirelessError::ConnectionFailed(peer_addr.to_string(), e.to_string()))?;
        }

        let services = dev
            .services()
            .await
            .map_err(|e| WirelessError::GattError(format!("Failed to list remote GATT services: {e}")))?;

        for service in services {
            if service.uuid().await.map_err(|e| WirelessError::GattError(e.to_string()))? == ARCHDROP_SERVICE_UUID {
                let characteristics = service
                    .characteristics()
                    .await
                    .map_err(|e| WirelessError::GattError(format!("Failed to list characteristics: {e}")))?;

                for char in characteristics {
                    if char.uuid().await.map_err(|e| WirelessError::GattError(e.to_string()))?
                        == CHAR_LOHS_CREDENTIALS_UUID
                    {
                        let bytes = char
                            .read()
                            .await
                            .map_err(|e| WirelessError::GattError(format!("Failed to read LOHS credentials: {e}")))?;
                        return decode_credentials(&bytes);
                    }
                }
            }
        }

        Err(WirelessError::GattError(format!(
            "Arch Drop GATT service ({ARCHDROP_SERVICE_UUID}) or credentials characteristic not found on {peer_addr}"
        )))
    }

    /// Sends a wakeup trigger directly over connected GATT to CHAR_WAKEUP_COMMAND.
    pub async fn send_wakeup_command_gatt(
        &self,
        peer_addr: Address,
        wakeup: &HotspotWakeup0x0D,
    ) -> Result<(), WirelessError> {
        if peer_addr == Address::any() {
            return Err(WirelessError::DeviceNotFound("Refusing connection to 00:00:00:00:00:00".into()));
        }
        let dev = self
            .adapter
            .device(peer_addr)
            .map_err(|e| WirelessError::DeviceNotFound(format!("{peer_addr}: {e}")))?;

        if !dev.is_connected().await.unwrap_or(false) {
            tokio::time::timeout(std::time::Duration::from_secs(4), dev.connect())
                .await
                .map_err(|_| WirelessError::ConnectionFailed(peer_addr.to_string(), "Connection timed out after 4s".into()))?
                .map_err(|e| WirelessError::ConnectionFailed(peer_addr.to_string(), e.to_string()))?;
        }

        let services = dev
            .services()
            .await
            .map_err(|e| WirelessError::GattError(format!("Failed to list remote GATT services: {e}")))?;

        for service in services {
            if service.uuid().await.map_err(|e| WirelessError::GattError(e.to_string()))? == ARCHDROP_SERVICE_UUID {
                let characteristics = service
                    .characteristics()
                    .await
                    .map_err(|e| WirelessError::GattError(format!("Failed to list characteristics: {e}")))?;

                for char in characteristics {
                    if char.uuid().await.map_err(|e| WirelessError::GattError(e.to_string()))?
                        == CHAR_WAKEUP_COMMAND_UUID
                    {
                        let bytes = encode_wakeup_command(wakeup);
                        char.write(&bytes)
                            .await
                            .map_err(|e| WirelessError::GattError(format!("Failed to write wakeup command: {e}")))?;
                        return Ok(());
                    }
                }
            }
        }

        Err(WirelessError::GattError(format!(
            "Arch Drop wakeup characteristic not found on {peer_addr}"
        )))
    }

    /// Writes a clipboard synchronization packet directly to a remote peer device over BLE GATT.
    pub async fn write_clipboard_sync_gatt(
        &self,
        peer_addr: Address,
        packet: &BleClipboardPacket,
    ) -> Result<(), WirelessError> {
        if peer_addr == Address::any() {
            return Err(WirelessError::DeviceNotFound("Refusing connection to 00:00:00:00:00:00".into()));
        }
        let dev = self
            .adapter
            .device(peer_addr)
            .map_err(|e| WirelessError::DeviceNotFound(format!("{peer_addr}: {e}")))?;

        if !dev.is_connected().await.unwrap_or(false) {
            tokio::time::timeout(std::time::Duration::from_secs(4), dev.connect())
                .await
                .map_err(|_| WirelessError::ConnectionFailed(peer_addr.to_string(), "Connection timed out after 4s".into()))?
                .map_err(|e| WirelessError::ConnectionFailed(peer_addr.to_string(), e.to_string()))?;
        }

        let services = dev
            .services()
            .await
            .map_err(|e| WirelessError::GattError(format!("Failed to list remote GATT services: {e}")))?;

        for service in services {
            if service.uuid().await.map_err(|e| WirelessError::GattError(e.to_string()))? == ARCHDROP_SERVICE_UUID {
                let characteristics = service
                    .characteristics()
                    .await
                    .map_err(|e| WirelessError::GattError(format!("Failed to list characteristics: {e}")))?;

                for char in characteristics {
                    if char.uuid().await.map_err(|e| WirelessError::GattError(e.to_string()))?
                        == CHAR_CLIPBOARD_SYNC_UUID
                    {
                        let bytes = packet.to_bytes();
                        char.write(&bytes)
                            .await
                            .map_err(|e| WirelessError::GattError(format!("Failed to write clipboard sync characteristic: {e}")))?;
                        return Ok(());
                    }
                }
            }
        }

        Err(WirelessError::GattError(format!(
            "Arch Drop clipboard sync characteristic ({CHAR_CLIPBOARD_SYNC_UUID}) not found on {peer_addr}"
        )))
    }

    /// Subscribes to clipboard synchronization notifications on the remote peer device.
    ///
    /// Forwards any incoming notification payloads to the provided channel.
    pub async fn subscribe_clipboard_notifications(
        &self,
        peer_addr: Address,
        tx: mpsc::Sender<Vec<u8>>,
    ) -> Result<tokio::task::JoinHandle<()>, WirelessError> {
        if peer_addr == Address::any() {
            return Err(WirelessError::DeviceNotFound("Refusing connection to 00:00:00:00:00:00".into()));
        }
        let dev = self
            .adapter
            .device(peer_addr)
            .map_err(|e| WirelessError::DeviceNotFound(format!("{peer_addr}: {e}")))?;

        if !dev.is_connected().await.unwrap_or(false) {
            tokio::time::timeout(std::time::Duration::from_secs(4), dev.connect())
                .await
                .map_err(|_| WirelessError::ConnectionFailed(peer_addr.to_string(), "Connection timed out after 4s".into()))?
                .map_err(|e| WirelessError::ConnectionFailed(peer_addr.to_string(), e.to_string()))?;
        }

        let services = dev
            .services()
            .await
            .map_err(|e| WirelessError::GattError(format!("Failed to list remote GATT services: {e}")))?;

        for service in services {
            if service.uuid().await.map_err(|e| WirelessError::GattError(e.to_string()))? == ARCHDROP_SERVICE_UUID {
                let characteristics = service
                    .characteristics()
                    .await
                    .map_err(|e| WirelessError::GattError(format!("Failed to list characteristics: {e}")))?;

                for char in characteristics {
                    if char.uuid().await.map_err(|e| WirelessError::GattError(e.to_string()))?
                        == CHAR_CLIPBOARD_SYNC_UUID
                    {
                        // 1. Read initial buffered clipboard packet if present on companion
                        if let Ok(initial_bytes) = char.read().await {
                            if !initial_bytes.is_empty() {
                                let _ = tx.send(initial_bytes).await;
                            }
                        }

                        // 2. Subscribe to live notifications for future clipboard changes
                        let mut notify_stream = Box::pin(char
                            .notify()
                            .await
                            .map_err(|e| WirelessError::GattError(format!("Failed to subscribe to clipboard notifications: {e}")))?);

                        let handle = tokio::spawn(async move {
                            while let Some(packet_bytes) = notify_stream.next().await {
                                if tx.send(packet_bytes).await.is_err() {
                                    break;
                                }
                            }
                        });
                        return Ok(handle);
                    }
                }
            }
        }

        Err(WirelessError::GattError(format!(
            "Arch Drop clipboard sync characteristic ({CHAR_CLIPBOARD_SYNC_UUID}) not found on {peer_addr}"
        )))
    }

    /// Discovers nearby companion device addresses from the local adapter cache.
    pub async fn discover_companion_addresses(&self) -> Vec<Address> {
        let mut result = Vec::new();
        if let Ok(addrs) = self.adapter.device_addresses().await {
            for addr in addrs {
                if addr == Address::any() {
                    continue;
                }
                if let Ok(dev) = self.adapter.device(addr) {
                    let is_conn = dev.is_connected().await.unwrap_or(false);
                    let has_rssi = dev.rssi().await.ok().flatten().is_some();
                    if !is_conn && !has_rssi {
                        continue;
                    }
                }
                if inspect_ble_device(&self.adapter, addr).await.is_some() {
                    result.push(addr);
                }
            }
        }
        result
    }

    /// Broadcasts a clipboard packet over BLE GATT to all discovered companion devices.
    pub async fn broadcast_clipboard_sync_gatt(
        &self,
        packet: &BleClipboardPacket,
    ) -> Result<usize, WirelessError> {
        let addrs = self.discover_companion_addresses().await;
        let mut count = 0;
        for addr in addrs {
            if let Ok(dev) = self.adapter.device(addr) {
                if !dev.is_connected().await.unwrap_or(false) {
                    continue;
                }
            }
            match self.write_clipboard_sync_gatt(addr, packet).await {
                Ok(_) => {
                    tracing::info!(peer = %addr, "Wrote clipboard sync packet to companion over BLE GATT");
                    count += 1;
                }
                Err(e) => {
                    tracing::debug!(peer = %addr, "Failed to write clipboard sync to {addr}: {e}");
                }
            }
        }
        Ok(count)
    }

    /// Registers the local Arch Drop GATT application.
    pub async fn serve_gatt(&mut self, state: GattServerState) -> Result<(), WirelessError> {
        let app = build_gatt_application(state);
        let handle = self
            .adapter
            .serve_gatt_application(app)
            .await
            .map_err(|e| WirelessError::GattError(format!("Failed to serve local GATT application: {e}")))?;
        self.active_app = Some(handle);
        Ok(())
    }
}

async fn inspect_ble_device(adapter: &Adapter, addr: Address) -> Option<DiscoveredDevice> {
    if addr == Address::any() {
        return None;
    }
    let dev = adapter.device(addr).ok()?;
    let rssi = dev.rssi().await.ok().flatten();
    let name = dev.name().await.ok().flatten();
    let mfg_data = dev.manufacturer_data().await.ok().flatten();

    let mut proximity_beacon = None;
    let mut hotspot_status = None;
    let mut hotspot_wakeup = None;

    if let Some(mfg) = mfg_data {
        if let Some(data) = mfg.get(&APPLE_COMPANY_ID) {
            if let Ok(beacon) = ProximityBeacon0x05::parse(data) {
                proximity_beacon = Some(beacon);
            } else if let Ok(status) = HotspotStatus0x0E::parse(data) {
                hotspot_status = Some(status);
            } else if let Ok(wakeup) = HotspotWakeup0x0D::parse(data) {
                hotspot_wakeup = Some(wakeup);
            }
        }
    }

    let is_archdrop_service = if let Ok(Some(uuids)) = dev.uuids().await {
        uuids.contains(&ARCHDROP_SERVICE_UUID)
    } else {
        false
    };

    let name_matches = name.as_deref().map(|n| {
        let lower = n.to_lowercase();
        lower.contains("archdrop") || lower.contains("moto") || lower.contains("android")
    }).unwrap_or(false);

    if proximity_beacon.is_some() || hotspot_status.is_some() || hotspot_wakeup.is_some() || is_archdrop_service || name_matches {
        Some(DiscoveredDevice {
            address: addr,
            rssi,
            name,
            proximity_beacon,
            hotspot_status,
            hotspot_wakeup,
            is_archdrop_service,
        })
    } else {
        None
    }
}

