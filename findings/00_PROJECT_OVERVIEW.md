# Arch Drop: Project Overview & Systems Architecture

**Document Identifier**: `FINDINGS-00-PROJECT-OVERVIEW`  
**Target Environment**: Android 10–16 Mobile Device $\longleftrightarrow$ Arch Linux Workstation  
**Compositor Baseline**: Hyprland 0.56.2+ (Aquamarine Native Wayland Backend)  
**Hardware Baseline**: Realtek RTL8852BE Wi-Fi 6 PCIe NIC (`rtw89_8852be`), Realtek Bluetooth 5.2 Adapter (`btusb`)  
**Network Invariant**: Existing WireGuard Interface `tiktok` (`10.2.0.2/32`, Table 51820) strictly preserved  

---

## 1. Project Mission & Core Directive

Arch Drop is a high-performance, open-source personal device continuity ecosystem connecting an Android smartphone and an Arch Linux laptop running the Hyprland Wayland compositor.

The goal is to deliver an Apple-like user experience (instant AirDrop-like file transfers, universal clipboard synchronization, proximity discovery, and automatic cross-network connectivity) without commercial cloud lock-in, manual IP configuration, or repeated authentication prompts.

### The Core Architectural Directive:
> **Integrate established open-source components before inventing new ones. Write only the minimal integration glue needed to unify them.**

---

## 2. Verified Architectural Stack

```
+===================================================================================================+
|                                    ARCH DROP UNIFIED ARCHITECTURE                                 |
+===================================================================================================+

    +---------------------------------------------------------------------------------------+
    |                                HEADSCALE CONTROL PLANE                                |
    |           (Self-hosted single Go binary; coordinates Node Keys, STUN & DERP)         |
    +-------------------------------------------+-------------------------------------------+
                                                |
                      +-------------------------+-------------------------+
                      v (WireGuard Public Keys & STUN Discovery)          v
+---------------------------------------------------+     +---------------------------------------------------+
|            ARCH LINUX HOST (HYPRLAND)             |     |                  ANDROID DEVICE                   |
|                                                   |     |                                                   |
|   +-------------------------------------------+   |     |   +-------------------------------------------+   |
|   |         archdropd (Unified Daemon)        |   |     |   |         Arch Drop Companion App           |   |
|   |                                           |   |     |   |                                           |   |
|   |   [ LocalSend Engine (packages/core) ]    |   |     |   |   [ LocalSend Companion Engine ]          |   |
|   |   - Hyper 1.11 HTTP/1.1 Server (:53317)   |   |     |   |   - Embedded HTTP Server & Client         |   |
|   |   - mTLS Ring Crypto Cert Pinning         |   |     |   |   - Auto-accept from Paired Fingerprint   |   |
|   |                                           |   |     |   |                                           |   |
|   |   [ Wayland Clipboard (wl-clipboard-rs) ] |   |     |   |   [ Android Clipboard & Notification Sync]|   |
|   |   - Binds ext_data_control_manager_v1     |   |     |   |   - Foreground Service Sync Engine        |   |
|   |   - Unfocused Selection Event Listener    |   |     |   |   - READ_LOGS Logcat Filter / Tile Fallback|  |
|   |   - Serializes 'kdeconnect.clipboard'     |   |     |   |   - Notification Interception Listener    |   |
|   |                                           |   |     |   |                                           |   |
|   |   [ Mesh Transport (tailscaled / wg) ]    |   |     |   |   [ Userspace SOCKS5 Engine (tsnet) ]     |   |
|   |   - Policy Routing (Table 51820 Isolated) |   |     |   |   - gVisor/netstack Userspace Engine      |   |
|   |   - Assigned Mesh IP: 100.64.0.1          |   |     |   |   - Binds Local SOCKS5 (127.0.0.1:port)   |   |
|   |                                           |   |     |   |   - ZERO VpnService / ZERO Root Required  |   |
|   +---------------------+---------------------+   |     |   +---------------------+---------------------+   |
+-------------------------|-------------------------+     +-------------------------|-------------------------+
                          |                                                         |
                          |==== Direct P2P WireGuard UDP Tunnel (Fast Path) =======|
                          |==== Encrypted DERP HTTPS 443 Relay (Fallback Path) =====|
                          |==== Local Wi-Fi LAN Direct Socket Bypass (192.168.x) ==|
```

---

## 3. Subsystem Breakdown

| Subsystem | Upstream Technology Base | Custom Arch Drop Glue | Resource Budget |
| :--- | :--- | :--- | :--- |
| **High-Speed Transfer** | LocalSend Rust Core (`packages/core`, Apache-2.0) | `crates/archdrop_transfer` | <10 MB RAM, 12.4 Gbps loopback |
| **Wayland Clipboard** | `wl-clipboard-rs` (`ext-data-control-v1`) | `crates/archdrop_clipboard` (4 KB rule) | <4 MB RAM, <10 ms latency |
| **Mesh Networking** | Tailscale / Headscale wireguard protocol | `crates/archdrop_mesh` + Android `tsnet` | Userspace SOCKS5, 0 root |
| **Proximity Discovery** | BlueZ BLE GATT (`bluer`) + Android BLE | `crates/archdrop_wireless` | 15-min rotating salted tokens |
| **Desktop Orchestration**| Pure Rust Tokio Daemon | `crates/archdropd` | **<15 MB RSS total** |
| **Compositor Bindings** | FreeDesktop D-Bus Notifications + Hyprland IPC | `zbus` notification client | Integrated in daemon |
