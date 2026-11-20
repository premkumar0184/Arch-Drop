# Arch Drop: Phase Transition Workflow & State Machine

## 1. Overview & Verification Protocol

The development of Arch Drop follows a strict, verification-driven phase state machine. No phase may transition to `completed` without meeting its mandatory verification gates and preserving all host safety invariants.

```
+----------------------------------------------------------------------------------------------------+
|                                    ARCH DROP PHASE STATE MACHINE                                   |
+----------------------------------------------------------------------------------------------------+

  [Phase 0: Validation & Source Audit] ──► [Phase 1: archdropd Core & Wayland] ──► [Phase 2: Offline P2P & BLE]
               STATUS: COMPLETED                        NEXT MILESTONE                         PLANNED
               - LocalSend Rust Core                   - Cargo Workspace Core                - BLE Proximity
               - Hyprland ext-data-control             - wl-clipboard-rs Adapter             - SCC Frequency Pinning
               - WireGuard Userspace Mesh              - LocalSend v2 HTTP Server            - Android LOHS Trigger
               - Verified in ARCH_DROP_PHASE_0         - Headless RSS < 15 MB                - Hardware Safe
                                                                                                │
                                                                                                ▼
  [Phase 4: Production Hardening] ◄────── [Phase 3: WAN Reachability & Mesh] ◄──────────────────┘
              PLANNED                                      PLANNED
              - Systemd User Service                       - Headscale / Tailscale tsnet
              - Waybar / Dunst Integration                 - DERP HTTPS Port 443 Relay
              - APK Release & Packaging                    - Event Ledger & State Queue
```

---

## 2. Phase Definitions & Verification Gates

### Phase 0: Technical Validation, Source Audit & Benchmarking
* **Status**: **`COMPLETED & VERIFIED`** (Authoritative Report: `ARCH_DROP_PHASE_0_VALIDATION.md`).
* **Delivered Artifacts**:
  - Upstream LocalSend source audit (`localsend/localsend` at `92e133a`, Apache-2.0, 147 unit tests passing).
  - Live Hyprland Wayland protocol diagnostics (`ext_data_control_manager_v1` Global 54 confirmed).
  - Native Wayland clipboard test harness (`wl-clipboard-rs`, 3/3 synthetic tests passing, <4 MB RAM).
  - WireGuard userspace SOCKS5 architecture proof bypassing Android single-`VpnService` constraint.
  - Host safety audit: `tiktok` interface (`10.2.0.2/32`, table 51820) verified 100% untouched.

---

### Phase 1: `archdropd` Core Engine & Headless Prototype (Active Next Milestone)
* **Status**: **`IN_PROGRESS`**
* **Primary Objective**:
  Deliver the independent, headless Rust daemon (`archdropd`) on Arch Linux integrating LocalSend file transfers and native Wayland clipboard synchronization over WireGuard, accompanied by an Android test harness.
* **Workspace Crates to Author**:
  1. `crates/archdrop_core`: Shared data types, X.509 certificate generation, and SHA-256 fingerprinting.
  2. `crates/archdrop_transfer`: Headless LocalSend transfer engine (Hyper 1.11 HTTP/1.1 server + client with mTLS cert pinning).
  3. `crates/archdrop_clipboard`: Native Wayland clipboard adapter linking `wl-clipboard-rs` (`ext-data-control-v1`) with the 4 KB threshold rule.
  4. `crates/archdrop_mesh`: Userspace mesh bridge & SOCKS5 proxy dialer.
  5. `crates/archdropd`: Unified systemd user daemon binary (`clap` CLI + `tokio` event loop).
  6. `android/`: Android companion service stub with embedded HTTP receiver and clipboard listener.
* **Mandatory Verification Gates**:
  - **Gate 1 (Footprint)**: `cargo build --release` produces `archdropd` with RSS < 15 MB and zero Qt/GTK dynamic libraries.
  - **Gate 2 (Clipboard)**: Copying text on desktop is captured in `archdropd` within <10 ms without window focus.
  - **Gate 3 (Transfer)**: Direct point-to-point file transfer to target WireGuard IP succeeds at >100 MB/s without multicast.
  - **Gate 4 (Safety)**: Host `tiktok` WireGuard interface and routing table 51820 verified 100% undisturbed.

---

### Phase 2: Hardware-Safe Offline P2P & BLE Orchestration
* **Status**: **`PLANNED`**
* **Primary Objective**:
  Implement zero-infrastructure synchronization without triggering Realtek RTL8852BE (`rtw89`) Multi-Channel Concurrency (MCC) driver crashes.
* **Components to Author**:
  - `crates/archdrop_wireless`: BlueZ BLE GATT server (`bluer`), advertising rotating ephemeral hashes (Apple 0x05 pattern) and remote hotspot wakeups (Apple 0x0D pattern).
  - NetworkManager D-Bus client: Controls Wi-Fi association and dynamic DHCP gateway discovery.
  - Android `LOHSManager`: Starts `WifiManager.startLocalOnlyHotspot()` upon receiving BLE wake command.
* **Mandatory Verification Gates**:
  - Zero MCC triggering: Wi-Fi Direct frequency pinned via `WifiP2pConfig.Builder.setGroupOperatingFrequency` (SCC) or pure station connection via LOHS.
  - Devices disconnect from external Wi-Fi, negotiate over BLE, associate to SoftAP, and complete transfer in <5 seconds.

---

### Phase 3: WAN Reachability, Mesh Overlay & State Reconciliation
* **Status**: **`PLANNED`**
* **Primary Objective**:
  Enable global cross-network connectivity (cellular/NAT) and offline event queue reconciliation.
* **Components to Author**:
  - Userspace DERP HTTPS:443 bridge and WireGuard keepalive manager.
  - Local SQLite event ledger on Linux (`rusqlite`) and Room DB on Android.
  - Monotonic compound sequence keys `(origin_device_id, sequence_number)` with vector high-water marks.
  - Ephemeral 120-second clipboard TTL and screen-lock zeroization.
* **Mandatory Verification Gates**:
  - Laptop behind NAT transfers file and clipboard to phone on LTE cellular via DERP relay.
  - Android commercial VPN (e.g. AdGuard / Mullvad) remains connected on `tun0` during active mesh transfers.
  - Offline changes sync deterministically upon reconnection with zero data loss.

---

### Phase 4: Production Hardening, Systemd User Service & Android UI
* **Status**: **`PLANNED`**
* **Primary Objective**:
  Package `archdropd` into an automated systemd user service, provide Hyprland status integration (Waybar/SwayNC), and package the Android APK.
* **Components to Author**:
  - `systemd/user/archdropd.service` configuration.
  - Waybar module script (`/api/status` or D-Bus query).
  - Production Android APK with Jetpack Compose onboarding UI, QR code pairing, and `READ_LOGS` setup guide.
* **Mandatory Verification Gates**:
  - Clean `systemctl --user start archdropd` bootstrap on boot.
  - Memory consumption remains <15 MB after 24 hours of continuous operation.
