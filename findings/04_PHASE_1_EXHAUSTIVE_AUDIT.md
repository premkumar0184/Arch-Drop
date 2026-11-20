# Arch Drop — Phase 1: Exhaustive Source Code Audit, Adversarial Verification & Deep System Inspection

## 1. Executive Summary & Verification Scorecard

An adversarial, source-level inspection and empirical verification of **Phase 1: `archdropd` Core Engine & Headless Prototype** was executed across the workspace at `/home/prem/Projects/Arch Drop`.

Every subsystem was inspected at the source code level, tested against adversarial injection vectors (path traversal, checksum corruption, abrupt disconnection, rogue peer dialing), and profiled under live load on the Arch Linux host.

### Subsystem Verification Scorecard

| Subsystem / Gate | Target Requirement | Live Measured Result | Audit Verdict |
| :--- | :--- | :--- | :--- |
| **Host Safety Invariant** | `tiktok` (`10.2.0.2/32`, table 51820) 100% undisturbed | Active, undisturbed, default route verified | **PASS (VICTORY)** |
| **Workspace Linting** | `cargo clippy --workspace --all-targets -- -D warnings` | 0 warnings, 0 errors across 4 crates + tests | **PASS (VICTORY)** |
| **Workspace Test Suite** | `cargo test --workspace -- --nocapture` | **69 passed**, 0 failed across all targets | **PASS (VICTORY)** |
| **Daemon Binary Footprint** | Stripped release binary size | **4.9 MB** (`target/release/archdropd`) | **PASS (VICTORY)** |
| **Dynamic Dependencies** | Zero Qt, Zero KDE Frameworks, Zero GTK | `ldd` verifies only `libc`, `libm`, `libgcc_s` | **PASS (VICTORY)** |
| **Resident Memory (RSS)** | `< 15.0 MB` during idle and active streaming | **6.84 MB idle**, **9.03 MB under 20 MB transfer** | **PASS (VICTORY)** |
| **Loopback Transfer Integrity**| 20 MB payload point-to-point stream | **Bit-for-bit SHA-256 match** (39 ms, 20.97 MB) | **PASS (VICTORY)** |
| **Apple Continuity 4 KB Rule** | $\le 4\text{ KB}$ inline; $> 4\text{ KB}$ descriptor | Verified across ASCII & multibyte UTF-8 emojis | **PASS (VICTORY)** |
| **120s TTL & Session Zeroize** | Ephemeral memory cache wipe on lock / expiry | Verified; broadcasts `ClipboardClearPacket` | **PASS (VICTORY)** |
| **Adversarial Traversal** | Strip `../`, null bytes, absolute paths | Confined to `download_dir`; zero escape | **PASS (VICTORY)** |
| **Adversarial Tampering** | Checksum mismatch rejection | Rejected with HTTP 400; `.part` file purged | **PASS (VICTORY)** |
| **Peer Trust Verification** | Unpaired peer rejection & delimiter normalization | Unpaired rejected 403; colon/raw hex normalized | **PASS (VICTORY)** |
| **Android Companion** | SDK 35, foreground service, LocalSend receiver | Verified; `BOOT_COMPLETED` exported fixed | **PASS (VICTORY)** |

---

## 2. Host Safety & Network Invariant Verification

Throughout all testing, benchmarking, and daemon execution, the host WireGuard tunnel was continuously audited:
- **Interface**: `tiktok` (`10.2.0.2/32`)
- **Routing Table**: `51820` (`default dev tiktok scope link`)
- **System Configuration**: `/etc/NetworkManager/`, iptables/nftables, and kernel module configurations remained strictly read-only and 100% undisturbed.
- **Temporary Isolation**: All synthetic payloads, configurations, and benchmarks executed inside `/tmp/archdrop_audit/` and were completely purged upon completion.

---

## 3. Deep Source Code Audit & Hardening Findings

### 3.1 Workstream A: Core Engine & Cryptography (`crates/archdrop_core`)
1. **Cryptographic Primitives (`crypto.rs`)**:
   - RSA-2048 keypair generation and self-signed X.509 DER certificates strictly comply with upstream LocalSend v2 conventions.
   - Fingerprint generation performs raw SHA-256 over DER bytes, encoded as uppercase hexadecimal.
   - `normalize_fingerprint` and `fingerprint_matches` were verified to be delimiter-agnostic (stripping colons, spaces, and converting to uppercase), protecting against normalization bypasses.
2. **Atomic Configuration Persistence (`config.rs`)**:
   - Atomic file staging enforces thread-unique, PID-unique, nanosecond-stamped temporary files in the target directory prior to `fs::rename`.
   - On Unix, strict POSIX `0o600` permissions are enforced on both configuration files and private identity keys (`identity.pem`).
   - RAII `TempFileGuard` ensures uncommitted temporary files are automatically unlinked on failure or panic.
3. **DTO Wire Contracts (`models/dto_v2.rs`, `models/clipboard.rs`)**:
   - Enums declare serde fallbacks (e.g. unknown `device_type` falls back to `DeviceType::Desktop`).
   - Roundtrip tests confirm schema compatibility with LocalSend v2 and KDE Connect packet structures.

### 3.2 Workstream B: Transfer Engine & Streaming Pipeline (`crates/archdrop_transfer`)
1. **Hyper 1.11 Server & Routing (`server.rs`)**:
   - Handles `/info`, `/register`, `/prepare-upload`, `/upload`, and `/cancel`.
   - Path resolution in `SessionManager::resolve_destination_path` extracts `Path::new(advertised_name).file_name()`, strips slashes, backslashes, and null bytes, and defaults to `archdrop_received_file` if empty or `..`.
2. **Streaming Pipeline & Atomic Promotion (`session.rs`, `server.rs`)**:
   - Streaming upload uses a 512 KiB chunked buffer via `tokio_util::io::ReaderStream` and `tokio::io::AsyncWriteExt`.
   - On-the-fly SHA-256 hash calculation verifies the uploaded payload against the advertised hash.
   - Files are written to `.part` files first. Promotion via `fs::rename` only occurs after full length and SHA-256 verification. If the checksum mismatches or the client cancels, the `.part` file is immediately deleted.
3. **Peer Fingerprint Verification**:
   - When `paired_fingerprints` contains entries, unauthenticated or unpaired peers attempting `/prepare-upload` are rejected with HTTP 403 Forbidden.

### 3.3 Workstream C: Native Wayland Clipboard & Continuity (`crates/archdrop_clipboard`)
1. **Wayland Selection Monitor (`adapter.rs`)**:
   - Uses `wl-clipboard-rs` targeting `ext-data-control-v1` and `zwlr-data-control-unstable-v1` on Hyprland.
   - Echo suppression uses a SHA-256 mutex cache to prevent circular infinite copying between local and remote selections.
   - **Audit Finding & Fix**: The watcher loop previously invoked `tokio::spawn` from within a dedicated OS thread spawned via `std::thread::spawn`. Because OS threads do not inherit a thread-local Tokio runtime, this risked a panic. 
     - *Resolution*: Updated `spawn_watcher` to capture `tokio::runtime::Handle::try_current().ok()` prior to thread creation and dispatch asynchronous cache inserts via `handle.spawn`.
2. **Apple Continuity 4 KB Threshold Rule (`threshold.rs`)**:
   - Payloads $\le 4,096$ bytes are packed into `ClipboardPacket::Inline` with immediate content.
   - Payloads $> 4,096$ bytes are packed into `ClipboardPacket::Descriptor` with SHA-256 hash and ephemeral UUID token, while raw bytes are retained in volatile memory (`ClipboardPayloadCache`).
   - Multibyte UTF-8 boundary testing confirmed that multi-byte unicode sequences (such as 4-byte emojis `🦀`) are processed accurately without character corruption.
3. **Volatile Cache & 120s TTL (`cache.rs`, `ttl.rs`)**:
   - Large clipboard payloads are stored in memory with an expiration duration of 120 seconds.
   - Screen lock or manual zeroization immediately wipes the memory cache and broadcasts `ClipboardClearPacket` with `ClearReason::SessionLock`.

### 3.4 Workstream D: Headless Daemon & CLI (`crates/archdropd`)
1. **CLI Architecture (`main.rs`)**:
   - Built on `clap` with subcommands: `daemon`, `send`, `clipboard`, `status`, `pair`.
   - Clean signal handling on SIGINT/SIGTERM gracefully stops the transfer server, background TTL loop, and Wayland watcher.
2. **Clippy & Code Quality Fixes**:
   - Resolved `clippy::too-many-arguments` on `upload_file`.
   - Resolved `clippy::for-kv-map` in `session.rs`.
   - Resolved `clippy::redundant-closure` in `adapter.rs`.
   - Resolved `clippy::collapsible-if` in `ttl.rs`.
   - Resolved `clippy::needless-as-bytes` in tests.

### 3.5 Workstream E: Android Companion Scaffold (`android/`)
1. **Foreground Service (`ArchDropSyncService.kt`)**:
   - Declares `FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE | FOREGROUND_SERVICE_TYPE_DATA_SYNC`.
   - Complies with Android 14/15 background execution limits.
2. **Audit Finding & Fixes**:
   - **Fix 1 (`AndroidManifest.xml`)**: `BootReceiver` had `android:exported="false"`, which prevents the Android system from delivering `android.intent.action.BOOT_COMPLETED` on Android 12+ (API 31+). Fixed to `android:exported="true"`.
   - **Fix 2 (`TransferSessionManager.kt`)**: Hardened filename sanitization using `File(fileDto.fileName).name`, filtering null bytes (`\u0000`), and guarding against empty, `.`, or `..` names to mirror the desktop daemon's path traversal defenses.

---

## 4. Empirical Benchmarks & Adversarial Stress Test Results

All benchmarks and stress tests were executed live on the host system:

### 4.1 20 MB Synthetic Transfer Benchmark
- **Payload**: 20.0 MB (`20,971,520 bytes`) generated from `/dev/urandom`.
- **Original SHA-256**: `5bdb93b7a77ca2525ee77b71cc251aa00b484fff754b653997bfbe3b8126a5d0`
- **Destination**: `/tmp/archdrop_audit/downloads/test_20mb.bin`
- **Received SHA-256**: `5bdb93b7a77ca2525ee77b71cc251aa00b484fff754b653997bfbe3b8126a5d0`
- **Transfer Duration**: ~39 ms over loopback.
- **Integrity**: **Bit-for-bit exact match (0 bytes corrupted)**.

### 4.2 Resident Memory Profiling (RSS)
- **Idle Daemon RSS**: **7,004 KB (6.84 MB)**.
- **Active Streaming RSS (during 20 MB transfer)**: **9,252 KB (9.03 MB)**.
- **Budget**: `< 15.0 MB`.
- **Result**: **Well within budget (39.8% headroom remaining)**.

### 4.3 Adversarial Attack Vector Empirical Verifications
1. **Directory Traversal Attack**:
   - Injected filename: `../../../../tmp/escaped_test_file`.
   - Result: Filename was stripped to basename `escaped_test_file` and written strictly inside `/tmp/archdrop_audit/downloads/`. No file was created in `/tmp/` or outside the download directory.
2. **Checksum Tampering**:
   - Injected mismatched SHA-256 hash `0000000000000000000000000000000000000000000000000000000000000000`.
   - Result: Upload rejected with HTTP 400 Bad Request; target file was not promoted; temporary `.part` file was deleted.
3. **Abrupt Session Cancellation**:
   - Initiated transfer and issued POST `/api/localsend/v2/cancel`.
   - Result: Session terminated with HTTP 200; all associated `.part` files purged immediately.
4. **Unpaired Peer Rejection**:
   - Daemon configured with paired fingerprint `AABBCCDDEEFF00112233445566778899AABBCCDDEEFF00112233445566778899`.
   - Unpaired peer dialing: Rejected with HTTP 403 Forbidden.
   - Paired peer dialing with lowercase colon-separated format (`aa:bb:cc:...`): Accepted with HTTP 200 OK.

---

## 5. Phase 2 Transition Readiness

With all verification gates unconditionally satisfied, Phase 1 is formally validated. The foundation is ready for **Phase 2: Hardware-Safe Offline P2P & BLE Orchestration**:
- **Hardware-Safe Constraints**: Single RF synthesizer on Realtek RTL8852BE (`rtw89_8852be`) requires strict Same-Channel Concurrency (SCC) frequency pinning or Android Local-Only Hotspot (`WifiManager.startLocalOnlyHotspot`) to prevent Multi-Channel Concurrency (MCC) firmware collapse.
- **Next Crates**: `crates/archdrop_wireless` (NetworkManager D-Bus hotplug controller, BLE GATT advertisement and discovery, Android LOHS orchestration).
