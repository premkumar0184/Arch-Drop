# Arch Drop: Apple Ecosystem Reference Architecture & Continuity Patterns

**Document Identifier**: `FINDINGS-02-APPLE-ECOSYSTEM-PATTERNS`  
**Reference Source**: Forensic Dissection of Apple iOS 27.2 (`~/Projects/iOS 27/findings/09_APPLE_ECOSYSTEM_DEEP_DIVE.md` & `artifacts/ecosystem/continuity_ipc_matrix.json`)  
**Target Platform Integration**: Arch Drop (Android $\longleftrightarrow$ Arch Linux Hyprland)  

---

## 1. Executive Summary: What Makes Apple Continuity Seamless?

Apple's cross-device ecosystem (AirDrop, Universal Clipboard, Handoff, Instant Hotspot, Auto-Unlock) operates across five discrete planes:

1. **The Signaling Plane (BLE 0x004C)**: Low-power continuous presence discovery and state notification without powering on Wi-Fi radios.
2. **The Identity & Trust Plane (IDS / Alloy)**: Zero-click mutual trust derived from shared Apple Account keys synced through hardware-backed Keychain (Protected Cloud Storage).
3. **The Data Plane (AWDL / Wi-Fi Direct)**: High-speed, ad-hoc, peer-to-peer Wi-Fi activated strictly on-demand for large payloads.
4. **The Caching & Presentation Plane (pasted / useractivityd)**: Ephemeral, two-tier state caches with automatic time-to-live expiration (120s) and lock-state zeroization.
5. **The Compositor Plane (FrontBoard / SpringBoard)**: Deep IPC integration with window managers to stage alerts, Dynamic Island live activities, and banner notifications.

---

## 2. Reverse-Engineered Core Continuity Daemons

| Daemon | Binary Path | Primary Role | Arch Drop Counterpart |
| :--- | :--- | :--- | :--- |
| **`sharingd`** | `/usr/libexec/sharingd` (10.48 MB) | Central coordinator for AirDrop, Handoff, proximity pairing | `archdropd` daemon core |
| **`wifip2pd`** | `/usr/libexec/wifip2pd` (8.57 MB) | AWDL and Wi-Fi Direct peer-to-peer transport broker | NetworkManager D-Bus client + Android `WifiManager` |
| **`pasted`** | `/usr/libexec/pasted` (316 KB) | Universal Clipboard server & pasteboard sync | `crates/archdrop_clipboard` (`wl-clipboard-rs`) |
| **`useractivityd`** | `/System/Library/PrivateFrameworks/.../useractivityd` (998 KB)| Handoff activity streamer & URL coordinator | `archdropd` state manager |
| **`identityservicesd`**| `/System/Library/PrivateFrameworks/.../identityservicesd` (16.3 MB)| Identity resolution, 112 IDS Alloy services, E2E encryption | WireGuard Cryptokey Routing (Noise_IK) + Headscale |

---

## 3. The 5 Core Apple Patterns Adopted into Arch Drop

### Pattern 1: The Two-Tier Clipboard Protocol (4 KB Threshold Rule)
* **Apple Ground Truth**:
  - Payloads $\le 4\text{ KB}$ are inlined into compressed notification streams.
  - Payloads $> 4\text{ KB}$ advertise only an authenticated SHA-256 hash over BLE (`0x0C`); full payload is fetched over direct TLS socket **only when the user actually invokes Paste**.
* **Arch Drop Adoption**:
  - `archdrop_clipboard` will inline text $\le 4\text{ KB}$ over the WireGuard tunnel.
  - Payloads $> 4\text{ KB}$ (images, long documents) advertise a metadata descriptor. Android places a proxy item in `ClipboardManager`; full streaming occurs over local HTTP only on read.

### Pattern 2: Ephemeral 120-Second TTL & Lock-State Zeroization
* **Apple Ground Truth**:
  - Universal Clipboard entries expire after 120 seconds of inactivity or upon device lock (`kAKSKeyBagLockStateLocked`).
* **Arch Drop Adoption**:
  - Remote clipboard buffers have a default TTL of 120 seconds.
  - Listening to Hyprland lock events (`ext-session-lock-v1` / `hypridle`) immediately flushes remote clipboard memory on Linux.
  - Android screen-off / keyguard-lock flushes remote clipboard memory on mobile.

### Pattern 3: Zero-Touch Hotspot Awakening (Apple 0x0D Beacon)
* **Apple Ground Truth**:
  - iPhones broadcast BLE `0x0D` packets containing cellular signal bars, RAT (5G/LTE), battery level, and cloud identity tokens.
  - Paired Macs can silently wake up the iPhone's Personal Hotspot via BLE without the user unlocking the phone.
* **Arch Drop Adoption**:
  - Android companion app advertises BLE GATT service.
  - Laptop sends a wake command over BLE.
  - Android calls `WifiManager.startLocalOnlyHotspot()`, spins up SoftAP, and sends SSID/passphrase over BLE to the laptop.
  - Laptop connects via NetworkManager D-Bus (`wlan0`). Zero interaction needed on the phone!

### Pattern 4: Salted Truncated Hash Presence (Apple 0x05 Beacon)
* **Apple Ground Truth**:
  - AirDrop broadcasts 2-byte truncated SHA-256 hashes of contact identifiers salted with an ephemeral salt rotating every 15 minutes.
* **Arch Drop Adoption**:
  - Arch Drop broadcasts rolling HMAC tokens instead of cleartext device names or IPs, preventing public network surveillance.

### Pattern 5: Physical Radio Reality (Why Arch Drop Avoids Direct AWDL Emulation)
* **Apple Silicon Reality**:
  - Apple uses custom Broadcom/Murata Wi-Fi firmware capable of microsecond hardware Time-Division Multiplexing (TDM) between the home AP and AWDL social channels (6, 44, 149).
* **Linux Realtek RTL8852BE Reality**:
  - PC Wi-Fi chips with single synthesizers suffer a 60–75% throughput collapse and PCIe command queue deadlocks (`c2h packet timeout`) under Multi-Channel Concurrency (MCC).
* **Arch Drop Translation**:
  - Enforce **Same-Channel Concurrency (SCC)** frequency pinning when an external AP is connected.
  - Use **Android Local-Only Hotspot (LOHS)** in pure station mode when offline, guaranteeing 100% stability at full 802.11ax link speeds.
