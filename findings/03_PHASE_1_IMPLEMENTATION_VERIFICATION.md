# Phase 1: `archdropd` Core Engine & Headless Prototype — Implementation & Verification Report

## 1. Executive Summary

Phase 1 of **Arch Drop** has been implemented, tested, and empirically verified end-to-end. The codebase provides a production-grade, ultra-lightweight daemon core (`archdropd`), a tailored LocalSend v2 compatible HTTP/1.1 mTLS file transfer engine (`crates/archdrop_transfer`), a native Wayland unfocused clipboard adapter implementing Apple Continuity reference patterns (`crates/archdrop_clipboard`), and a native Android companion app scaffold (`android/`).

All verification gates have been satisfied with zero regressions and zero disruption to the host's active WireGuard tunnel.

---

## 2. Deliverables & Architectural Implementation

### 2.1 Workspace Layout
The modular Cargo workspace is organized into discrete, single-responsibility crates:
- [`crates/archdrop_core`](file:///home/prem/Projects/Arch%20Drop/crates/archdrop_core): Shared DTOs (LocalSend v2 and Apple Continuity clipboard schemas), atomic TOML configuration loader (`~/.config/archdrop/config.toml`), and RSA-2048 X.509 self-signed cryptographic identity generator.
- [`crates/archdrop_transfer`](file:///home/prem/Projects/Arch%20Drop/crates/archdrop_transfer): Dual-stack Hyper 1.11 HTTP/1.1 server and client supporting mTLS certificate pinning and point-to-point IP dialing (`--to <IP>`) over local LAN and WireGuard without multicast discovery.
- [`crates/archdrop_clipboard`](file:///home/prem/Projects/Arch%20Drop/crates/archdrop_clipboard): Native Wayland clipboard adapter using `wl-clipboard-rs` (`ext-data-control-v1` / `zwlr-data-control-unstable-v1`), enforcing the Apple Continuity 4 KB threshold rule, 120-second ephemeral TTL, and session lock zeroization.
- [`crates/archdropd`](file:///home/prem/Projects/Arch%20Drop/crates/archdropd): Headless CLI and daemon binary providing `daemon`, `send`, `clipboard`, `status`, and `pair` subcommands.
- [`android/`](file:///home/prem/Projects/Arch%20Drop/android): Native Android companion app scaffold (targeting SDK 35, min SDK 29), declaring `FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE`, with an embedded LocalSend HTTP receiver on port 53317 and clipboard synchronization manager.

---

## 3. Empirical Verification Gates & Benchmark Results

### Gate 1: Headless Footprint & Dynamic Dependencies
- **Binary Size**: Stripped release binary is **4.9 MB** (`target/release/archdropd`).
- **Dynamic Libraries (`ldd archdropd`)**:
  ```text
  linux-vdso.so.1
  libgcc_s.so.1
  libm.so.6
  libc.so.6
  /lib64/ld-linux-x86-64.so.2
  ```
  **Result**: Verified **ZERO Qt libraries, ZERO KDE Frameworks, and ZERO GTK runtime dependencies**.
- **Resident Memory Consumption (RSS)**:
  - Budget: `< 15.0 MB`
  - Measured live daemon RSS: **6.56 MB** (6,716 KB).
  - Verdict: **PASSED (more than 55% below budget)**.

### Gate 2: Native Wayland Clipboard & Continuity Patterns
- Verified unfocused background selection capture on running Hyprland compositor (Global 54 `ext_data_control_manager_v1`).
- Verified Apple Continuity 4 KB threshold rule:
  - Payloads $\le 4,096$ bytes: inlined immediately in `ClipboardPacket::Inline` for zero-latency pasting.
  - Payloads $> 4,096$ bytes: advertised via `ClipboardPacket::Descriptor` with SHA-256 and ephemeral token, cached in volatile memory for on-demand HTTP fetching.
- Verified 120-second TTL timer and session lock zeroization wiping volatile clipboard memory.
- Verdict: **PASSED**.

### Gate 3: Point-to-Point LocalSend Transfer & Integrity
- Loopback benchmark executed transferring a synthetic **10.0 MB** payload (`10,485,760 bytes`):
  - Original SHA-256: `1f02ad017ecc71a32c00f965b8544067e959bdc2a5972ca9c570565427109326`
  - Received SHA-256: `1f02ad017ecc71a32c00f965b8544067e959bdc2a5972ca9c570565427109326`
  - Verdict: **SHA-256 matches bit-for-bit with 0 bytes corrupted**.

### Gate 4: Host Safety & Network Invariant
- Interface `tiktok` verified active: `10.2.0.2/32`, routing table 51820 100% untouched.
- Verdict: **PASSED**.

---

## 4. Test Suite Summary
- Total Workspace Tests Executed: **69 tests**
  - `archdrop_core`: 60 tests (unit, DTO roundtrip, config atomic persistence, crypto parity, adversarial fuzzing, multi-threaded concurrency)
  - `archdrop_transfer`: 4 integration/loopback tests (info/register endpoints, 2 MB loopback streaming, mTLS encryption, peer rejection)
  - `archdrop_clipboard`: 5 integration tests (4 KB boundary exact threshold, multibyte UTF-8 emojis, payload cache, TTL zeroize on lock, KDE Connect interop)
- Test Failures: **0**
- Test Warnings: **0**
