# Arch Drop: Core Agent & Engineering Rules (Hardened)

## 1. Fundamental Execution Directives

These directives are absolute and apply to all agents, subagents, and development sessions working within the **Arch Drop** workspace:

1. **Sequential Phase Execution**:
   - Execute exactly **one major milestone/phase at a time** as defined in `.agent_rules/PHASE_WORKFLOW.md` and tracked in `progress.json`.
   - Never skip forward or mark a phase complete without verifiable disk artifacts and passing automated test suites.
   - Ground all implementation in the empirical findings established in `ARCH_DROP_PHASE_0_VALIDATION.md`.

2. **Integration Before Reinvention**:
   - **Never rewrite infrastructure that already exists and works.**
   - Reuse verified upstream modules:
     - File Transfer: LocalSend Rust core (`packages/core`, Apache-2.0).
     - Clipboard: Native Wayland data-control via `wl-clipboard-rs` (`ext-data-control-v1`).
     - Mesh & WAN Transport: Headscale / Tailscale userspace WireGuard (`tsnet` / embedded SOCKS5).
     - Wireless Control: NetworkManager D-Bus API (`org.freedesktop.NetworkManager`) and Android `WifiManager`.
   - Custom code is strictly reserved for the thin integration glue (`archdropd` on Linux and `Arch Drop Companion` on Android).

3. **Depth Over Breadth & Zero Placeholders (Anti-Cheating Policy)**:
   - **Never commit placeholder code or documentation** (e.g., `// TODO: implement later`, `unimplemented!()`, `/* omitted for brevity */`, or synthetic mock data).
   - Every file created must compile cleanly, execute real logic, and handle actual error states.
   - Any document or artifact marked as an output of a phase must contain real, verified empirical data, code listings, or benchmark metrics.

4. **Proportional Effort & Lightweight Architecture**:
   - Maintain an ultra-lightweight memory and CPU footprint.
   - Desktop daemon (`archdropd`): **Resident memory (RSS) must stay < 15 MB**. Zero Qt, zero KDE Frameworks, zero GTK runtime dependencies in the daemon core.
   - Android Companion: Must not drain battery or trigger OEM process killing. Use `FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE` with a low-priority status notification.

5. **Persistent On-Disk State**:
   - Preserve all architectural decisions, reverse-engineering analyses, protocol schemas, and benchmark records on disk under `findings/`.
   - Never keep critical knowledge solely in LLM context.

6. **Authoritative Execution Tracker**:
   - `progress.json` is the **sole authoritative execution tracker** for milestone progress and verification states.
   - Always read `progress.json` before starting work; update `progress.json` only after empirical verification passes.

---

## 2. Experimental Safety & Host Integrity Rules

The host machine is an Arch Linux workstation with an active WireGuard connection. Strict safety boundaries must be maintained at all times:

1. **Preserve Existing WireGuard Tunnel (`tiktok`)**:
   - **STRICT SAFETY INVARIANT**: The existing WireGuard interface `tiktok` (`10.2.0.2/32`, routing table 51820, fwmark `0xca6c`) must remain 100% active and undisturbed.
   - Never run `wg-quick down`, never flush routing table 51820, and never add default routes (`0.0.0.0/0`) that intercept existing VPN traffic.
2. **Read-Only System Configuration**:
   - Never modify NetworkManager configuration files in `/etc/NetworkManager/`.
   - Never alter host firewall (`iptables`, `nftables`) or sysctl network settings without explicit user consent.
   - Never change kernel module options (`/etc/modprobe.d/rtw89.conf`).
3. **Android Device Safety**:
   - Zero root requirements. Never attempt or recommend unlocking the bootloader or installing a custom ROM.
   - Never take over Android's system-wide `VpnService` slot; mesh networking on Android must operate in userspace via SOCKS5 (`tsnet` / `wireproxy` on `127.0.0.1`).
4. **Credential & Privacy Protection**:
   - Never log, display, or commit private keys, preshared keys, authentication tokens, or personal clipboard data.
   - All tests and benchmarks must use harmless synthetic strings or test files in temporary directories (`/tmp/archdrop_tests/`).

---

## 3. Hardware & Protocol Constraints

All code authored for Arch Drop must respect verified hardware and OS boundaries:

1. **Realtek RTL8852BE Wi-Fi Concurrency (`rtw89`)**:
   - The host Wi-Fi adapter has a single physical MAC and single RF synthesizer.
   - **Never trigger Multi-Channel Concurrency (MCC)**: Running Wi-Fi Direct on a different channel than the primary infrastructure station causes a 60–75% throughput collapse and PCIe command queue deadlocks (`c2h packet timeout`).
   - **Enforce Same-Channel Concurrency (SCC)**: When an external AP is connected, pin Android Wi-Fi Direct to the laptop's exact station frequency (`WifiP2pConfig.Builder.setGroupOperatingFrequency`).
   - **Offline Transfers**: Prefer **Android Local-Only Hotspot (`WifiManager.startLocalOnlyHotspot`)**, letting the Linux laptop connect as a pure 802.11 station on `wlan0`.
2. **Hyprland / Wayland Security Policies**:
   - Wayland compositors prohibit unfocused background windows from reading clipboard selections via standard window protocols (`wl_data_device`).
   - The desktop daemon must bind compositor-level data-control protocols:
     - Primary: `ext_data_control_manager_v1` (Global 54 in Hyprland 0.56.2).
     - Fallback: `zwlr_data_control_manager_v1` (Global 38 in Hyprland).
   - Use `wl-clipboard-rs` asynchronous event streams.
3. **Android 10–16 Background Clipboard & Lifecycle Policies**:
   - AOSP restricts background apps from reading `ClipboardManager` without active window focus.
   - Documented workaround: Grant `android.permission.READ_LOGS` via ADB or use a Quick Settings tile / floating overlay trigger for manual pull.
   - Android 14+ 6-hour execution cap on `dataSync` services is mitigated by separating persistent sync into `FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE` and transient file transfers into `DATA_SYNC`.

---

## 4. Continuity & Apple Reference Patterns

Derived from the reverse-engineered Apple Ecosystem stack in `~/Projects/iOS 27/` (`findings/09_APPLE_ECOSYSTEM_DEEP_DIVE.md`), Arch Drop incorporates these proven patterns:

1. **The 4 KB Clipboard Rule**:
   - Small payloads ($\le 4\text{ KB}$): Inlined in compressed sync packets for instantaneous (<25 ms) pasting.
   - Large payloads ($> 4\text{ KB}$ text, images, files): Advertised via lightweight metadata descriptor (`hash`, `mime`, `size`). Binary stream is fetched on-demand over a direct socket only when the recipient issues a "Paste" command.
2. **120-Second Ephemeral TTL & Screen Lock Flush**:
   - Remote clipboard buffers expire automatically after 120 seconds of inactivity.
   - When Hyprland locks (via `hypridle` / `ext-session-lock-v1`) or Android screen turns off, remote clipboard data is wiped from volatile memory.
3. **Zero-Touch Hotspot Awakening (Apple 0x0D Pattern)**:
   - Laptop detects Android phone via BLE GATT advertisement.
   - Laptop sends a wake signal over BLE; phone starts `LocalOnlyHotspot` invisibly in the background and returns SSID/passphrase over BLE.
   - Laptop connects via NetworkManager D-Bus without touching the phone.
4. **Salted Rolling Hash Presence (Apple 0x05 Pattern)**:
   - Proximity beacons broadcast an ephemeral rolling HMAC (`HMAC-SHA256(timestamp / 900)`) instead of cleartext device names, preventing third-party tracking in public spaces.

---

## 5. Coding & Toolchain Standards

1. **Rust (`archdropd` & crates)**:
   - Rust 2021/2024 edition, strictly formatted with `rustfmt`, zero warnings under `cargo clippy --all-targets`.
   - Async runtime: `tokio 1.x` (multi-threaded).
   - HTTP/TLS: `hyper 1.11`, `rustls 0.23` (ring crypto).
   - D-Bus: `zbus 4.x` for NetworkManager and desktop notifications.
   - Workspace layout: `crates/archdrop_core`, `crates/archdrop_transfer`, `crates/archdrop_clipboard`, `crates/archdrop_mesh`, `crates/archdropd`.
2. **Android Companion App**:
   - Modern Kotlin (targeting SDK 34/35/36, min SDK 29 / Android 10).
   - UI: Jetpack Compose with Material 3.
   - Userspace Mesh: Embedded `tsnet` / SOCKS5 proxy via `gVisor/netstack` (zero root, zero `VpnService` permission).
   - Data Storage: Room Database for state synchronization queues.
