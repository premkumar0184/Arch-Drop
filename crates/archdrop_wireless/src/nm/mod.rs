//! NetworkManager D-Bus station controller and host safety invariant enforcement.
//!
//! Provides asynchronous NetworkManager interaction via zbus 5.0 for:
//! - Primary managed 802.11 device discovery
//! - Directed Access Point scanning without disruptive off-channel dwell time
//! - In-memory volatile connection profile creation and activation (AddAndActivateConnection2)
//! - Multi-tier dynamic Android LOHS gateway IP resolution
//! - Strict host safety preservation (WireGuard interface tiktok and routing table 51820)

pub mod client;
pub mod coordinator;
pub mod gateway;
pub mod profile;
pub mod types;

// Re-exports
pub use client::{ActiveLohsSession, NmStationController};
pub use coordinator::{LohsSessionCoordinator, DEFAULT_LOHS_IDLE_TIMEOUT};
pub use gateway::{
    calculate_subnet_gateway, parse_address_data, parse_dhcp_options, resolve_gateway_from_data,
    ResolvedGatewayInfo,
};
pub use profile::ConnectionProfileBuilder;
pub use types::{
    AccessPointInfo, ActiveConnectionState, LohsConnectionConfig, SecurityType,
    DEFAULT_ROUTE_METRIC, NM_ACTIVE_CONNECTION_STATE_ACTIVATED,
    NM_ACTIVE_CONNECTION_STATE_ACTIVATING, NM_ACTIVE_CONNECTION_STATE_DEACTIVATED,
    NM_ACTIVE_CONNECTION_STATE_DEACTIVATING, NM_DEVICE_TYPE_WIFI, NM_DEVICE_TYPE_WIFI_P2P,
};
