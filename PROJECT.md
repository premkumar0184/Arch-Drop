# Project: Arch Drop (Phase 4 — Production Hardening, Systemd User Service & Android UI)

## Architecture Overview
Arch Drop Phase 2 implements hardware-safe offline peer-to-peer (P2P) wireless transfer and BLE GATT orchestration between Arch Linux (Hyprland Wayland) and Android without requiring prior shared Wi-Fi infrastructure.
1. `crates/archdrop_wireless`: Linux BLE advertising/discovery (`bluer 0.17`), Apple Continuity 0x05 / 0x0D / 0x0E wire frames, GATT credential negotiation, and NetworkManager D-Bus station association (`zbus 5.0`).
2. `android/app`: Android companion app with `LOHSManager` (`WifiManager.startLocalOnlyHotspot()`), `BlePeripheralManager` (GATT server & advertiser), and coroutine-based `HttpReceiver` (port 53317).
3. `crates/archdropd`: Headless daemon and CLI integrating `--p2p` automated discovery -> LOHS connection -> LocalSend v2 transfer dialing, preserving direct `--to <IP>` point-to-point transfers.
4. Host Safety Guarantees: Realtek RTL8852BE single RF synthesizer safety (operating as managed station to Android LOHS SoftAP, eliminating dual-channel MCC crashes) and preserving active `tiktok` WireGuard interface (`10.2.0.2/32`, routing table 51820) undisturbed.

```
+-----------------------------------------------------------------------------------+
|                        Arch Linux (Hyprland Wayland Desktop)                     |
|                                                                                   |
|  +------------------------+                     +------------------------------+  |
|  |       archdropd        |                     |      archdrop_transfer       |  |
|  | (CLI / Daemon Orchestr)|                     |     (LocalSend v2 Engine)    |  |
|  +-----------+------------+                     +--------------+---------------+  |
|              |                                                 |                  |
|              v                                                 |                  |
|  +-------------------------------------------------------------+---------------+  |
|  |                             crates/archdrop_wireless                         |  |
|  |  +-------------------------------------+   +-----------------------------+  |  |
|  |  |            src/ble/                 |   |           src/nm/           |  |  |
|  |  |  - BleController (bluer)            |   |  - NmStationController      |  |  |
|  |  |  - Apple 0x05 Proximity Beacon      |   |    (zbus 5.0 D-Bus)         |  |  |
|  |  |  - Apple 0x0D Hotspot Wakeup        |   |  - Volatile WPA2/WPA3 Conn  |  |  |
|  |  |  - Apple 0x0E Status Response       |   |  - Gateway IP Resolution    |  |  |
|  |  |  - GATT Credential Client           |   |  - Host Safety (never-def)  |  |  |
|  |  +-------------------------------------+   +-----------------------------+  |  |
|  +-----------------------------------------------------------------------------+  |
+--------------------------+-------------------------------------+------------------+
                           | BLE 0x0D Wake                       | Wi-Fi 802.11ax
                           | GATT Creds                          | LOHS Station Link
                           v                                     v
+-----------------------------------------------------------------------------------+
|                              Android Companion Device                             |
|                                                                                   |
|  +----------------------------------------+   +--------------------------------+  |
|  |          BlePeripheralManager          |   |          LOHSManager           |  |
|  |  - BluetoothLeAdvertiser               |   |  - startLocalOnlyHotspot()     |  |
|  |  - BluetoothGattServer                 |   |  - LocalOnlyHotspotReservation |  |
|  |  - Wake trigger listener               |   |  - WPA2/WPA3 SoftAP (SSID/PSK) |  |
|  |  - Credential exchange service         |   |  - Gateway IP (192.168.43.1)   |  |
|  +--------------------+-------------------+   +---------------+----------------+  |
|                       |                                       |                   |
|                       +-------------------+-------------------+                   |
|                                           |                                       |
|                                           v                                       |
|                       +---------------------------------------+                   |
|                       |        HttpReceiver (Port 53317)      |                   |
|                       | - Kotlin Coroutine ServerSocket       |                   |
|                       | - Bound to 0.0.0.0 (LOHS Interface)   |                   |
|                       | - LocalSend v2 REST & Binary Streams  |                   |
|                       +---------------------------------------+                   |
+-----------------------------------------------------------------------------------+
```

## Feature Inventory
| # | Feature | Description | Milestone | Source |
|---|---------|-------------|-----------|--------|
| 1 | F01: Cargo Workspace Foundation | Root Cargo.toml workspace configuration with shared dependency specifications | Phase 1 M1 | Survey 1 |
| 2 | F02: Configuration Loader | Self-documenting config loader for `~/.config/archdrop/config.toml` with defaults | Phase 1 M1 | Survey 1 |
| 3 | F03: Shared DTO Data Models | Serde models for LocalSend v2 wire protocol and clipboard packets | Phase 1 M1 | Survey 1 |
| 4 | F04: X.509 Cert & Fingerprint | X.509 self-signed cert generation (`rcgen` + `ring`) & uppercase hex DER SHA-256 fingerprinting | Phase 1 M1 | Survey 1 |
| 5 | F05: Hyper 1.11 HTTP Server | Dual-stack HTTP/1.1 server on `:53317` with `/info`, `/prepare-upload`, `/upload`, `/cancel` | Phase 1 M2 | Survey 1 |
| 6 | F06: mTLS Certificate Pinning | `CustomClientCertVerifier` and `PinnedServerCertVerifier` with auto-acceptance for paired peers | Phase 1 M2 | Survey 1 |
| 7 | F07: Point-to-Point IP Dialing | Unicast HTTP/mTLS client dialing `--to <IP>` without multicast discovery requirements | Phase 1 M2 | Survey 1 |
| 8 | F08: Streaming File Pipeline | 512 KiB buffered streaming transfer with concurrent SHA-256 digest computation | Phase 1 M2 | Survey 1 |
| 9 | F09: Wayland Clipboard Watcher | Unfocused background selection capture via `ext_data_control_manager_v1` (fallback `zwlr`) | Phase 1 M3 | Survey 2 |
| 10 | F10: Apple 4 KB Threshold Rule | Inlining text $\le 4\text{ KB}$; advertising lazy metadata descriptor for payloads $> 4\text{ KB}$ | Phase 1 M3 | Survey 2 |
| 11 | F11: 120-Second Ephemeral TTL | Automatic in-memory clipboard eviction and Wayland `copy::clear()` after 120s | Phase 1 M3 | Survey 2 |
| 12 | F12: Session Lock Zeroization | Wiping clipboard buffers and clearing selection via systemd-logind D-Bus signals (`zbus`) | Phase 1 M3 | Survey 2 |
| 13 | F13: Secret / Password Filtering | Dropping offers with `x-kde-passwordManagerHint`, `application/x-secret`, etc. | Phase 1 M3 | Survey 2 |
| 14 | F14: Headless Daemon & CLI | `clap` derive CLI (`daemon`, `send`, `clipboard`, `status`) and IPC server | Phase 1 M4 | Survey 2 |
| 15 | F15: Memory & Dependency Budget | Resident memory < 15 MB RSS and zero Qt/KDE/GTK dynamic dependencies (`ldd` check) | Phase 1 M4 | Survey 2 |
| 16 | F16: Android Project Scaffold | Modern Kotlin Gradle project (compileSdk 35, minSdk 29, Java 21, Gradle 9.8.1) | Phase 1 M5 | Survey 3 |
| 17 | F17: Connected Device Service | Foreground service declaring `FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE` | Phase 1 M5 | Survey 3 |
| 18 | F18: Android HTTP Receiver | Embedded LocalSend v2 compatible receiver accepting binary file streams on port 53317 | Phase 1 M5 | Survey 3 |
| 19 | F19: 3-Tier Clipboard Sync | Android clipboard delivery architecture with 4 KB rule and background fallbacks | Phase 1 M5 | Survey 3 |
| 20 | F20: Workspace Unit Tests | Comprehensive automated tests across all crates (`cargo test --workspace`) | Phase 1 M6 | Survey 3 |
| 21 | F21: Loopback Transfer Test | Automated file streaming with SHA-256 checksum verification over localhost | Phase 1 M6 | Survey 3 |
| 22 | F22: Synthetic Wayland Test | Unfocused selection capture verification on live Hyprland compositor | Phase 1 M6 | Survey 3 |
| 23 | F23: Network Safety Invariants | Host WireGuard interface `tiktok` and routing table 51820 preserved undisturbed | Phase 1 M6 | Survey 3 |
| 24 | F24: Progress Metrics Update | Update `progress.json` with Phase 1 verification metrics and acceptance data | Phase 1 M6 | Survey 3 |
| 25 | F25: Wireless Crate Foundation | `crates/archdrop_wireless` Cargo workspace member, `bluer 0.17`, `zbus 5.0`, `tokio`, models | Phase 2 M1 | Survey 1 |
| 26 | F26: Apple 0x05 Proximity Frame | 22-byte Manufacturer Data under Company ID 0x004C (AirDrop beacon, rolling HMAC salt) | Phase 2 M1 | Survey 1 |
| 27 | F27: Apple 0x0D Hotspot Wakeup | 12-byte 0x0D remote hotspot wakeup trigger payload (HMAC token + port 53317) | Phase 2 M1 | Survey 1 |
| 28 | F28: Apple 0x0E Hotspot Status | 10-byte 0x0E status response parsing (battery %, cell RAT, signal bars, frequency) | Phase 2 M1 | Survey 1 |
| 29 | F29: Linux BLE GATT Controller | `BleController` via `bluer`: BlueZ peripheral advertising, central discovery stream, GATT | Phase 2 M1 | Survey 1 |
| 30 | F30: NM D-Bus Station Client | `NmStationController` via `zbus 5`: scan for target LOHS SoftAP SSID | Phase 2 M2 | Survey 2 |
| 31 | F31: Volatile WPA2/WPA3 Association | In-memory transient connection via `AddAndActivateConnection2` (`persist = "volatile"`) | Phase 2 M2 | Survey 2 |
| 32 | F32: Android Gateway Resolution | Asynchronous DHCP lease & IP4Config resolution to determine Android gateway IP | Phase 2 M2 | Survey 2 |
| 33 | F33: Host Network Safety Invariants | `ipv4.never-default = true`, table 51820 & `tiktok` WireGuard preserved 100% | Phase 2 M2 | Survey 2 |
| 34 | F34: Android LOHS Reservation | `LOHSManager` managing `WifiManager.startLocalOnlyHotspot()` across API 29-35 | Phase 2 M3 | Survey 3 |
| 35 | F35: Android BLE GATT Peripheral | `BlePeripheralManager` advertising, wake trigger handling, credential delivery | Phase 2 M3 | Survey 3 |
| 36 | F36: Coroutine HTTP Receiver | Zero-external-dependency Kotlin coroutine `ServerSocket` listening on port 53317 | Phase 2 M3 | Survey 3 |
| 37 | F37: Android Build & Permissions | Clean `./gradlew assembleDebug` build, `local.properties`, permissions | Phase 2 M3 | Survey 3 |
| 38 | F38: Daemon P2P CLI Integration | `archdropd send` wireless flow: BLE wakeup -> NM connect -> LocalSend v2 dialing | Phase 2 M4 | Survey 1, 2 |
| 39 | F39: Automated Tests & Verification | `cargo check --workspace`, `cargo test --workspace`, WireGuard safety audit | Phase 2 M4 | Survey 1, 2, 3 |
| 40 | F40: Canonical DERP Codec | 10-frame length-prefixed binary DERP frame serializer/deserializer (`crates/archdrop_mesh`) | Phase 3 M1 | Survey 1 |
| 41 | F41: Ed25519 Mutual Auth | Ring 0.17 mutual public-key authentication with 60-second timestamp freshness | Phase 3 M1 | Survey 1 |
| 42 | F42: Virtual Duplex DerpStream | `AsyncRead + AsyncWrite` adapter enabling Hyper mTLS over DERP relay packet channels | Phase 3 M1 | Survey 1 |
| 43 | F43: Transparent Fallback Dialer | Tiered connectivity bounding direct TCP to 1500 ms SLA and falling back to DERP in <1.55s | Phase 3 M1 | Survey 1 |
| 44 | F44: Deterministic SQLite Ledger | Local WAL mode ledger in `archdrop_core` with monotonic `(origin, seq)` compound keys | Phase 3 M2 | Survey 2 |
| 45 | F45: Android Room DB Ledger | AndroidX Room 2.6.1 + KSP event ledger mirroring SQLite schema and vector clock state | Phase 3 M2 | Survey 2 |
| 46 | F46: Vector Clock Reconciliation | 5-step causal synchronization protocol with Last-Write-Wins (LWW) conflict tie-breaking | Phase 3 M2 | Survey 2 |
| 47 | F47: Android Userspace Mesh | Pure-Kotlin loopback networking (`127.0.0.1`) with 0 `VpnService` claims and VPN coexistence | Phase 3 M3 | Survey 3 |
| 48 | F48: E2E Mesh Transfer & Invariants | 224 workspace tests, 51 Android tests, 8.22 MB daemon RSS, WireGuard tiktok untouched | Phase 3 M4 | Survey 1-3 |
| 49 | F49: Hardened Systemd Service | `systemd/user/archdropd.service` with `ProtectSystem=strict`, `MemoryDenyWriteExecute`, sandboxing | Phase 4 M1 | Survey 1 |
| 50 | F50: Signal Handling & Socket Cleanup | Dual SIGTERM/SIGINT graceful 5s drain in `daemon.rs`, RAII `SocketGuard` in `ipc.rs` | Phase 4 M1 | Survey 1 |
| 51 | F51: Lifecycle Scripts & Arch Package | `scripts/manage-service.sh`, `install-service.sh`, `uninstall-service.sh`, `packaging/arch/PKGBUILD` | Phase 4 M1 | Survey 1 |
| 52 | F52: Waybar NDJSON IPC Streaming | Real-time `archdropd status --follow` streaming with EMA speed smoothing ($\alpha=0.35$) & classes | Phase 4 M2 | Survey 2 |
| 53 | F53: Freedesktop D-Bus Notifications | Native `zbus 5.0` notification engine for Dunst/SwayNC with interactive actions & `xdg-open` | Phase 4 M2 | Survey 2 |
| 54 | F54: Android Material 3 Compose UI | Complete Compose M3 UI with dynamic theming, 4-tab Navigation, hero cards, Room flows | Phase 4 M3 | Survey 3 |
| 55 | F55: Offline Cryptographic QR Pairing | ZXing QR code generator & CameraX scanner decoding `archdrop://pair` URIs into Room DB | Phase 4 M3 | Survey 3 |
| 56 | F56: Production Soak & Resource Bounds | 15m soak stability (RSS 8.57-11.07 MB < 15 MB), 0 Qt/GTK dynamic deps, signed release APK | Phase 4 M4 | Survey 1-3 |

## Milestones
| # | Name | Scope | Dependencies | Status |
|---|------|-------|-------------|--------|
| P1.1 | P1 M1: Core & Identity Engine | Workspace root, `crates/archdrop_core` (DTOs, config, X.509 Ring crypto, fingerprints) | None | DONE |
| P1.2 | P1 M2: LocalSend Transfer Engine | `crates/archdrop_transfer` (Hyper 1.11, mTLS cert pinning, P2P IP dialing, streaming) | P1 M1 | DONE |
| P1.3 | P1 M3: Wayland Clipboard Adapter | `crates/archdrop_clipboard` (`wl-clipboard-rs`, 4 KB rule, 120s TTL, session lock, secrets) | P1 M1 | DONE |
| P1.4 | P1 M4: Headless Daemon & CLI | `crates/archdropd` (`clap` subcommands, event loop, IPC socket, RSS < 15 MB) | P1 M1-3 | DONE |
| P1.5 | P1 M5: Android Companion Scaffold | `android/` project scaffold (Gradle, foreground service, LocalSend HTTP receiver, clipboard) | P1 M1 | DONE |
| P1.6 | P1 M6: E2E Acceptance & Invariants | E2E test suite execution, loopback test, Wayland test, WireGuard audit | P1 M1-5 | DONE |
| P2.1 | P2 M1: Linux BLE & Continuity Engine | `crates/archdrop_wireless` (Cargo.toml, `src/lib.rs`, `src/ble/`, 0x05/0x0D/0x0E frames) | None | DONE |
| P2.2 | P2 M2: NetworkManager D-Bus Controller | `crates/archdrop_wireless` (`src/nm/`, D-Bus station client, volatile profiles, safety) | P2 M1 | DONE |
| P2.3 | P2 M3: Android LOHS & BLE Peripheral | `android/app` (`LOHSManager`, `BlePeripheralManager`, `HttpReceiver`) | None | DONE |
| P2.4 | P2 M4: Integration, E2E Verification & Safety Audit | `crates/archdropd`, workspace tests, `./gradlew assembleDebug`, audit | P2 M1-3 | DONE |
| P3.1 | P3 M1: Userspace DERP Relay & WAN NAT Traversal | `crates/archdrop_mesh` (DERP codec, Ed25519 auth, DerpStream, FallbackDialer) | None | DONE |
| P3.2 | P3 M2: Deterministic State Ledger & Vector Sync | SQLite WAL in `archdrop_core`, Room in `android/app`, 5-step vector sync protocol | P3 M1 | DONE |
| P3.3 | P3 M3: Android Userspace Mesh & VPN Coexistence | Pure-Kotlin loopback `127.0.0.1`, 0 VpnService, FIB Rule 0 coexistence, SunEC Ed25519 | P3 M1, P3 M2 | DONE |
| P3.4 | P3 M4: E2E WAN Integration & Invariant Audit | CLI `--derp-relay`, 224 Rust tests, 51 Android tests, 8.22 MB RSS, WireGuard safety | P3 M1-3 | DONE |
| P4.1 | P4 M1: Desktop Systemd Service & Packaging | `systemd/user/archdropd.service`, lifecycle scripts, PKGBUILD, RAII socket | None | DONE |
| P4.2 | P4 M2: Hyprland Desktop Status Integration | Waybar NDJSON IPC streaming, EMA speed smoothing, D-Bus notifications, xdg-open | P4 M1 | DONE |
| P4.3 | P4 M3: Android Production UI & QR Pairing | Material 3 Compose UI, CameraX+ZXing QR pairing, 71 tests, signed release APK | None | DONE |
| P4.4 | P4 M4: Production Hardening, Soak & Invariants | 15m soak stability (RSS 11.07 MB < 15 MB), 0 Qt/GTK deps, 260 Rust tests, WireGuard | P4 M1-3 | DONE |

## Interface Contracts

### archdrop_wireless::ble ↔ archdropd
- Controller: `BleController::init(BleConfig)` connects to system BlueZ via `bluer 0.17`.
- Advertising: `send_hotspot_wakeup(HotspotWakeup0x0D)` broadcasts Apple 0x0D wakeup frame.
- Discovery: `scan_for_peers()` returns a stream of `DiscoveredDevice` containing proximity and status metadata.
- Negotiation: `fetch_hotspot_credentials(Address)` reads characteristic `CHAR_LOHS_CREDENTIALS_UUID` (`a4c80003-...`) to obtain SSID and passphrase.

### archdrop_wireless::nm ↔ archdropd
- Controller: `NmStationController::new()` connects to NetworkManager system D-Bus via `zbus 5.0`.
- Device Enumeration: `find_wifi_device(Option<&str>)` filters for managed 802.11 station interface (`DeviceType == 2`).
- Scanning: `scan_access_points(device, Option<&str>, Duration)` performs directed SSID discovery with local cache fast-path.
- Association: `connect_lohs(device, &LohsConnectionConfig, Duration)` invokes `AddAndActivateConnection2` with `persist = "volatile"`.
- Resolution: Multi-tier gateway resolution resolving Android SoftAP gateway IP (e.g. `192.168.43.1`).
- Lifecycle: `ActiveLohsSession` guard automatically deactivates transient profile upon completion or drop.

### Desktop BLE ↔ Android BlePeripheralManager
- Protocol: Apple Continuity 0x05 / 0x0D / 0x0E frames under Manufacturer Data Company ID `0x004C`.
- GATT Service UUID: `a4c80001-7d1a-4c91-9e28-3e2840000001`.
- Characteristics:
  - Wakeup Command: `a4c80002-7d1a-4c91-9e28-3e2840000001` (Write).
  - LOHS Credentials: `a4c80003-7d1a-4c91-9e28-3e2840000001` (Read, Notify).
  - Telemetry: `a4c80004-7d1a-4c91-9e28-3e2840000001` (Read, Notify).

### Desktop NM Station ↔ Android LOHSManager
- SoftAP: WPA2-Personal (`wpa-psk`) or WPA3-Personal (`sae`) 802.11ax/ac link.
- Gateway IP: `192.168.43.1` (standard Android LOHS subnet `192.168.43.0/24`).
- Safety Metric: Host route metric 2000, `never-default = true`, `ignore-auto-dns = true`, `disable-ipv6 = true`.

### archdrop_transfer ↔ Android HttpReceiver
- Protocol: LocalSend v2 over HTTP/1.1 (port 53317).
- Server: Android coroutine-based `ServerSocket` bound to `0.0.0.0:53317` on LOHS network interface.
- Pipeline: `/api/localsend/v2/info` -> `/prepare-upload` -> `/upload` streaming binary chunks with SHA-256 verification.

## Code Layout
```
/home/prem/Projects/Arch Drop/
├── Cargo.toml                              # Workspace root Cargo.toml (5 crates)
├── Cargo.lock
├── crates/
│   ├── archdrop_core/                      # Shared models, crypto, config
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config.rs                   # ~/.config/archdrop/config.toml loader
│   │       ├── crypto.rs                   # Ring + rcgen X.509 cert & SHA-256 fingerprint
│   │       └── models/                     # DTOs: dto_v2.rs, clipboard.rs, device.rs
│   ├── archdrop_transfer/                  # LocalSend v2 engine
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── server.rs                   # Hyper 1.11 HTTP/1.1 dual-stack server (:53317)
│   │       ├── client.rs                   # P2P IP dialing client (--to <IP>)
│   │       ├── tls.rs                      # tokio-rustls mTLS & custom cert verifiers
│   │       └── stream.rs                   # 512 KiB streaming with concurrent SHA-256
│   ├── archdrop_clipboard/                 # Native Wayland clipboard adapter
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── watcher.rs                  # wl-clipboard-rs ext-data-control watcher
│   │       ├── continuity.rs               # 4 KB threshold rule & 120s ephemeral TTL
│   │       ├── lock.rs                     # Session lock zeroization via zbus D-Bus
│   │       └── filter.rs                   # Password manager hint filtering
│   ├── archdrop_wireless/                  # Linux BLE & NetworkManager P2P engine
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                      # Crate entrypoint & exports
│   │       ├── error.rs                    # WirelessError enum
│   │       ├── ble/                        # BLE & Continuity Engine
│   │       │   ├── mod.rs
│   │       │   ├── controller.rs           # bluer Adapter, central discovery & advertising
│   │       │   ├── gatt.rs                 # GATT service & characteristic client/server
│   │       │   ├── types.rs                # HotspotCredentials, BleConfig, DiscoveredDevice
│   │       │   └── continuity/             # Apple Continuity wire frames
│   │       │       ├── mod.rs
│   │       │       ├── proximity_0x05.rs   # Apple 0x05 AirDrop proximity beacon
│   │       │       └── hotspot_0x0d.rs     # Apple 0x0D wakeup & 0x0E status frames
│   │       └── nm/                         # NetworkManager D-Bus Controller
│   │           ├── mod.rs
│   │           ├── client.rs               # zbus 5.0 NM D-Bus client & ActiveLohsSession
│   │           ├── profile.rs              # Volatile connection profile dictionary builder
│   │           ├── gateway.rs              # Dynamic multi-tier gateway IP resolution
│   │           └── types.rs                # SecurityType, LohsConnectionConfig, AP info
│   └── archdropd/                          # Headless daemon & CLI
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs                     # CLI entrypoint (daemon, send --to/--p2p, wireless, clipboard)
│           ├── daemon.rs                   # Tokio event loop orchestrating server & watcher
│           └── ipc.rs                      # UNIX domain socket IPC server/client
├── android/                                # Android companion application
│   ├── build.gradle.kts                    # Root build script
│   ├── settings.gradle.kts                 # Project settings
│   ├── gradle.properties
│   ├── local.properties                    # sdk.dir=/home/prem/Android/Sdk
│   ├── gradlew                             # Gradle wrapper script
│   └── app/
│       ├── build.gradle.kts                # App module build configuration
│       └── src/main/
│           ├── AndroidManifest.xml         # Declares Wi-Fi & BLE permissions, foreground service
│           └── kotlin/org/archdrop/
│               ├── ArchDropApp.kt
│               ├── service/
│               │   ├── ArchDropSyncService.kt  # Dual foreground service coordinating LOHS & BLE
│               │   ├── LOHSManager.kt          # startLocalOnlyHotspot manager across API 29-35
│               │   └── BlePeripheralManager.kt # GATT server & advertiser (UUID a4c80001-...)
│               └── transfer/
│                   └── HttpReceiver.kt         # Coroutine ServerSocket LocalSend v2 receiver (:53317)
└── tests/                                  # Integration & E2E verification
    ├── p2p_wireless_e2e_test.rs            # E2E test (BLE frames, NM profile, gateway, LocalSend, WireGuard)
    └── ...
```
