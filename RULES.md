# Arch Drop Engineering Rules & Operational Guidelines

This repository enforces strict engineering directives, safety boundaries, and architectural patterns. All AI agents, contributors, and development sessions operating in `/home/prem/Projects/Arch Drop` must comply with these guidelines.

---

## 1. Quick Reference: Absolute Rules

1. **Host Safety First**:
   - **DO NOT TOUCH** the active `tiktok` WireGuard connection (`10.2.0.2/32`, routing table 51820).
   - Zero disruptive system changes: no editing `/etc/NetworkManager`, no altering kernel modules (`/etc/modprobe.d/rtw89.conf`), no flushing firewall rules.
2. **Android Non-Root Constraint**:
   - Zero root requirements. Never unlock the bootloader or flash custom ROMs.
   - Do NOT take over Android's global `VpnService` slot; use userspace networking via embedded SOCKS5 (`tsnet` / `wireproxy` on `127.0.0.1`).
3. **No Placeholders (Zero-Tolerance Anti-Cheating)**:
   - Never commit `// TODO`, `unimplemented!()`, or mock templates.
   - Every file must compile cleanly, execute real logic, and handle errors.
4. **Hardware-Safe Wi-Fi (Realtek RTL8852BE)**:
   - Single RF synthesizer: Never trigger Multi-Channel Concurrency (MCC).
   - Enforce Same-Channel Concurrency (SCC) frequency pinning or use Android Local-Only Hotspot (`WifiManager.startLocalOnlyHotspot`).
5. **Lightweight Footprint**:
   - Desktop daemon (`archdropd`): **Resident memory < 15 MB**. Pure Rust (Tokio, Hyper 1.11, Rustls, `wl-clipboard-rs`). Zero Qt/GTK runtime bloat.
6. **Apple Continuity Best Practices**:
   - 4 KB clipboard threshold rule: $\le 4\text{ KB}$ inline; $> 4\text{ KB}$ metadata descriptor + on-demand fetch.
   - 120-second ephemeral clipboard TTL; wipe remote clipboard memory on lock screen.
   - BLE GATT remote hotspot wakeup (Apple 0x0D pattern).

---

## 2. Directory & Documentation Index

- **`.agent_rules/CORE_RULES.md`**: Detailed execution directives, safety policies, coding standards.
- **`.agent_rules/PHASE_WORKFLOW.md`**: The 5-phase transition state machine and verification gates.
- **`progress.json`**: Authoritative execution tracker.
- **`findings/00_PROJECT_OVERVIEW.md`**: Architectural topology, subsystem breakdown, resource budgets.
- **`findings/01_PHASE_0_EMPIRICAL_VALIDATION.md`**: Empirical test results (LocalSend 12.4 Gbps, Hyprland `ext-data-control-v1`, WireGuard SOCKS5).
- **`findings/02_APPLE_ECOSYSTEM_REFERENCE_PATTERNS.md`**: Reverse-engineered Apple iOS 27.2 Continuity stack and translation into open source.
- **`ARCH_DROP_PHASE_0_VALIDATION.md`**: The master 1,110-line Phase 0 validation report.
- **`ECOSYSTEM_RESEARCH_REPORT.md`**: The foundational 1,792-line ecosystem landscape report.
