# E2E Test Infra: Arch Drop (Phase 1)

## Test Philosophy
- Opaque-box, requirement-driven, deterministic verification.
- Validates the complete Arch Drop continuity system: Rust daemon workspace (`archdropd`), LocalSend v2 streaming transfer engine, native Wayland clipboard adapter with Apple Continuity patterns, Android companion scaffold, and host network safety invariants.
- Methodology: Category-Partition + Boundary Value Analysis (BVA) + Pairwise Combinatorial Testing + Real-World Workload Testing across Tiers 1–4.

## Feature Inventory & Test Mapping
| # | Feature | Source | Tier 1 (Coverage) | Tier 2 (Boundary) | Tier 3 (Pairwise) | Tier 4 (Scenario) |
|---|---------|--------|:-----------------:|:-----------------:|:-----------------:|:-----------------:|
| F01 | Cargo Workspace Foundation | ORIGINAL_REQUEST R1 | 5 | 5 | ✓ | ✓ |
| F02 | Configuration Loader (`config.toml`) | ORIGINAL_REQUEST R1 | 5 | 5 | ✓ | ✓ |
| F03 | Shared DTO Data Models (LocalSend v2) | ORIGINAL_REQUEST R1 | 5 | 5 | ✓ | ✓ |
| F04 | X.509 Cert & SHA-256 Fingerprint | ORIGINAL_REQUEST R1 | 5 | 5 | ✓ | ✓ |
| F05 | Hyper 1.11 HTTP/1.1 Server (:53317) | ORIGINAL_REQUEST R1 | 5 | 5 | ✓ | ✓ |
| F06 | mTLS & Cert Pinning Verifier | ORIGINAL_REQUEST R1 | 5 | 5 | ✓ | ✓ |
| F07 | Point-to-Point IP Dialing Client | ORIGINAL_REQUEST R1 | 5 | 5 | ✓ | ✓ |
| F08 | Streaming File Pipeline & SHA-256 | ORIGINAL_REQUEST R1 | 5 | 5 | ✓ | ✓ |
| F09 | Wayland Unfocused Selection Capture | ORIGINAL_REQUEST R1 | 5 | 5 | ✓ | ✓ |
| F10 | Apple 4 KB Threshold Rule | ORIGINAL_REQUEST R1 | 5 | 5 | ✓ | ✓ |
| F11 | 120-Second Ephemeral TTL Eviction | ORIGINAL_REQUEST R1 | 5 | 5 | ✓ | ✓ |
| F12 | Session Lock Zeroization (zbus) | ORIGINAL_REQUEST R1 | 5 | 5 | ✓ | ✓ |
| F13 | Password Manager Hint Filtering | ORIGINAL_REQUEST R1 | 5 | 5 | ✓ | ✓ |
| F14 | Headless Daemon CLI Subcommands | ORIGINAL_REQUEST R1 | 5 | 5 | ✓ | ✓ |
| F15 | Memory Footprint (<15 MB) & Zero Qt/GTK | ORIGINAL_REQUEST R1 | 5 | 5 | ✓ | ✓ |
| F16 | Android Kotlin Gradle Project Scaffold | ORIGINAL_REQUEST R2 | 5 | 5 | ✓ | ✓ |
| F17 | Android Connected Device Service | ORIGINAL_REQUEST R2 | 5 | 5 | ✓ | ✓ |
| F18 | Embedded LocalSend HTTP Receiver | ORIGINAL_REQUEST R2 | 5 | 5 | ✓ | ✓ |
| F19 | Android 3-Tier Clipboard Sync | ORIGINAL_REQUEST R2 | 5 | 5 | ✓ | ✓ |
| F20 | Workspace Unit Tests | ORIGINAL_REQUEST R4 | 5 | 5 | ✓ | ✓ |
| F21 | Loopback File Transfer with SHA-256 | ORIGINAL_REQUEST R4 | 5 | 5 | ✓ | ✓ |
| F22 | Synthetic Wayland Clipboard Test | ORIGINAL_REQUEST R4 | 5 | 5 | ✓ | ✓ |
| F23 | WireGuard Host Invariant (`tiktok`) | ORIGINAL_REQUEST R3 | 5 | 5 | ✓ | ✓ |
| F24 | Progress Metrics Recording | ORIGINAL_REQUEST R4 | 5 | 5 | ✓ | ✓ |

## Test Architecture
- **Test Runner**:
  - Rust workspace: `cargo test --workspace -- --nocapture`
  - Integration harness: `cargo test --test loopback_transfer_test` and `cargo test --test wayland_clipboard_test`
  - Android build: `cd android && ./gradlew assembleDebug`
  - Host safety audit: `ip addr show dev tiktok` and `ip route show table 51820`
  - Footprint audit: `cat /proc/<pid>/status | grep VmRSS` and `ldd target/release/archdropd`
- **Output & Pass/Fail Semantics**:
  - Exit code `0` on all commands.
  - Zero warnings or errors under `cargo build --release`.
  - Zero dynamic dependencies on Qt, KDE Frameworks, or GTK.
  - WireGuard interface `tiktok` remains active and table 51820 intact.

## Real-World Application Scenarios (Tier 4)
| # | Scenario | Features Exercised | Complexity |
|---|----------|--------------------|------------|
| 1 | Headless Local File Burst: Direct IP dial send of 10 MB payload over localhost with concurrent SHA-256 verification | F01, F03, F05, F06, F07, F08, F14, F21 | High |
| 2 | Wayland Background Continuity: Unfocused selection copy under Hyprland, 4 KB rule partitioning, and TTL expiration | F01, F09, F10, F11, F13, F14, F22 | High |
| 3 | Privacy Hardening & Screen Lock: Secret MIME suppression and session lock zeroization via systemd-logind D-Bus | F09, F11, F12, F13 | Medium |
| 4 | Daemon Low-Resource Footprint: Long-lived daemon execution verified < 15 MB RSS with zero Qt/GTK dynamic libraries | F01, F05, F09, F14, F15 | Medium |
| 5 | Android Companion Scaffold Build: Clean Gradle assemble of foreground service and embedded LocalSend HTTP receiver | F16, F17, F18, F19 | High |
| 6 | Host Network Invariant Preservation: Full suite execution with 100% preservation of `tiktok` WireGuard interface & table 51820 | F23, F24 | Critical |

## Coverage Thresholds
- Tier 1: Feature Coverage (≥5 tests per feature across 24 features = 120 tests)
- Tier 2: Boundary & Corner Cases (≥5 tests per feature = 120 tests)
- Tier 3: Cross-Feature Combinations (Pairwise coverage across major feature interactions = 24 tests)
- Tier 4: Real-World Application Scenarios (6 comprehensive application-level scenarios)
- **Total Threshold: ≥ 270 test assertions / cases**
