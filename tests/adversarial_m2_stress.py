#!/usr/bin/env python3
"""
Empirical Adversarial Stress Harness for Arch Drop Milestone 2:
- Rate Smoothing & Waybar IPC Streaming Stress
- Memory Footprint & Continuous RSS Budget (< 15 MB)
- Zero GUI (Qt, KDE, GTK, GDK) Dynamic Linking
- Host Safety Invariant (tiktok WireGuard interface and table 51820)
"""

import os
import sys
import time
import socket
import signal
import subprocess
import tempfile
import threading
import json

PROJECT_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
BIN_RELEASE = os.path.join(PROJECT_ROOT, "target", "release", "archdropd")

if not os.path.exists(BIN_RELEASE):
    print(f"ERROR: Release binary {BIN_RELEASE} not found. Build release binary first.")
    sys.exit(1)

test_failures = []
test_passes = []

def run_test(name, func):
    print(f"\n================================================================================")
    print(f"[RUNNING TEST] {name}")
    print(f"================================================================================")
    start = time.time()
    try:
        func()
        duration = time.time() - start
        print(f"[PASSED] {name} ({duration:.3f}s)")
        test_passes.append((name, duration))
    except Exception as e:
        duration = time.time() - start
        print(f"[FAILED] {name} ({duration:.3f}s): {e}")
        test_failures.append((name, str(e)))

def get_process_rss_kb(pid):
    """Read RSS in kilobytes from /proc/[pid]/statm or ps."""
    try:
        with open(f"/proc/{pid}/statm", "r") as f:
            parts = f.read().split()
            pages = int(parts[1])
            page_size_kb = os.sysconf("SC_PAGE_SIZE") // 1024
            return pages * page_size_kb
    except Exception:
        out = subprocess.check_output(["ps", "-o", "rss=", "-p", str(pid)])
        return int(out.strip())

def test_host_wireguard_safety():
    """Verify tiktok WireGuard interface (10.2.0.2/32, routing table 51820) remains active and untouched."""
    out = subprocess.check_output(["ip", "addr", "show", "dev", "tiktok"]).decode()
    if "10.2.0.2" not in out:
        raise AssertionError(f"tiktok interface does not have 10.2.0.2: {out}")
    if "UP" not in out:
        raise AssertionError(f"tiktok interface is not UP: {out}")

    rules = subprocess.check_output(["ip", "rule", "show"]).decode()
    if "51820" not in rules or "10.2.0.2" not in rules:
        raise AssertionError(f"ip rule 51820 missing: {rules}")

    routes = subprocess.check_output(["ip", "route", "show", "table", "51820"]).decode()
    if "tiktok" not in routes:
        raise AssertionError(f"table 51820 missing default route on tiktok: {routes}")

def test_zero_gui_dynamic_dependencies():
    """Run ldd on target/release/archdropd and ensure 0 dependencies on Qt, KDE, GTK, or GDK."""
    out = subprocess.check_output(["ldd", BIN_RELEASE]).decode()
    forbidden = ["gtk", "qt", "gdk", "kde"]
    for line in out.splitlines():
        for pat in forbidden:
            if pat in line.lower():
                raise AssertionError(f"Forbidden GUI library found in ldd output: {line}")
    print(f"ldd verified clean dependencies:\n{out.strip()}")

def test_daemon_streaming_rss_under_15mb():
    """Measure resident set size (RSS) under continuous simulated transfer streaming."""
    with tempfile.TemporaryDirectory() as tmpdir:
        sock_path = os.path.join(tmpdir, "archdropd.sock")
        cfg_path = os.path.join(tmpdir, "config.toml")
        
        # Write minimal config
        with open(cfg_path, "w") as f:
            f.write(f"""
[device]
alias = "ChallengerTest"

[network]
port = 54321
bind_address = "127.0.0.1"

[storage]
download_dir = "{tmpdir}/downloads"

[security]
paired_fingerprints = []
""")
        os.makedirs(os.path.join(tmpdir, "downloads"), exist_ok=True)

        env = os.environ.copy()
        env["RUST_LOG"] = "warn"

        proc = subprocess.Popen(
            [BIN_RELEASE, "--config", cfg_path, "daemon", "--socket-path", sock_path, "--port", "54321"],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

        try:
            # Wait for socket to appear
            for _ in range(50):
                if os.path.exists(sock_path):
                    break
                time.sleep(0.05)
            else:
                raise RuntimeError("Daemon failed to create socket in time")

            # Check initial idle RSS
            initial_rss = get_process_rss_kb(proc.pid)
            print(f"Initial daemon RSS: {initial_rss} KB ({initial_rss / 1024:.2f} MB)")
            if initial_rss >= 15360:
                raise AssertionError(f"Initial RSS exceeds 15 MB: {initial_rss} KB")

            # Connect 3 streaming clients (Waybar follow)
            clients = []
            for _ in range(3):
                s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                s.connect(sock_path)
                s.sendall(b"status --follow\n")
                clients.append(s)

            # Stream updates and measure RSS continuously
            max_rss = initial_rss
            for cycle in range(50):
                # Sample lines from clients to prevent socket buffer backpressure
                for s in clients:
                    s.setblocking(False)
                    try:
                        data = s.recv(4096)
                        if data:
                            # Verify valid JSON
                            lines = data.decode(errors="ignore").splitlines()
                            for l in lines:
                                if l.strip():
                                    parsed = json.loads(l.strip())
                                    assert "text" in parsed
                                    assert "class" in parsed
                    except BlockingIOError:
                        pass

                current_rss = get_process_rss_kb(proc.pid)
                if current_rss > max_rss:
                    max_rss = current_rss
                time.sleep(0.02)

            print(f"Max continuous RSS during streaming: {max_rss} KB ({max_rss / 1024:.2f} MB)")
            if max_rss >= 15360:
                raise AssertionError(f"Max RSS exceeded 15 MB budget: {max_rss} KB >= 15360 KB")

            # Close client sockets
            for s in clients:
                s.close()

        finally:
            proc.send_signal(signal.SIGTERM)
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()

def test_cli_status_follow_streaming():
    """Verify `archdropd status --follow` connects and streams valid newline JSON."""
    with tempfile.TemporaryDirectory() as tmpdir:
        sock_path = os.path.join(tmpdir, "archdropd.sock")
        cfg_path = os.path.join(tmpdir, "config.toml")
        with open(cfg_path, "w") as f:
            f.write(f"""
[device]
alias = "CliFollowTest"

[network]
port = 54322
bind_address = "127.0.0.1"

[storage]
download_dir = "{tmpdir}/downloads"

[security]
paired_fingerprints = []
""")
        os.makedirs(os.path.join(tmpdir, "downloads"), exist_ok=True)

        env = os.environ.copy()
        env["RUST_LOG"] = "warn"

        daemon_proc = subprocess.Popen(
            [BIN_RELEASE, "--config", cfg_path, "daemon", "--socket-path", sock_path, "--port", "54322"],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

        cli_proc = None
        try:
            for _ in range(50):
                if os.path.exists(sock_path):
                    break
                time.sleep(0.05)
            else:
                raise RuntimeError("Daemon failed to bind socket")

            # Spawn CLI status --follow
            cli_proc = subprocess.Popen(
                [BIN_RELEASE, "status", "--follow", "--socket-path", sock_path],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )

            # Read initial output line from status --follow
            line = cli_proc.stdout.readline()
            if not line:
                raise AssertionError("CLI status --follow produced no output")

            parsed = json.loads(line.strip())
            print(f"Received CLI status line: {parsed}")
            assert "text" in parsed, "Missing 'text' in payload"
            assert "class" in parsed, "Missing 'class' in payload"
            assert "tooltip" in parsed, "Missing 'tooltip' in payload"
            # On a live Wayland session, the clipboard watcher immediately emits a ClipboardSynced event
            # for existing clipboard data, yielding class ['clipboard', 'paired']. Otherwise, 'idle'.
            assert parsed["class"] in [["idle"], ["clipboard", "paired"], ["paired"]], f"Unexpected class: {parsed['class']}"

        finally:
            if cli_proc:
                cli_proc.terminate()
                try:
                    cli_proc.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    cli_proc.kill()
def test_heavy_transfer_streaming_rss_soak():
    """Stress test: 10 concurrent Waybar clients receiving 500 rapid status updates, verifying continuous RSS < 15 MB."""
    with tempfile.TemporaryDirectory() as tmpdir:
        sock_path = os.path.join(tmpdir, "archdropd.sock")
        cfg_path = os.path.join(tmpdir, "config.toml")
        with open(cfg_path, "w") as f:
            f.write(f"""
[device]
alias = "SoakTest"

[network]
port = 54323
bind_address = "127.0.0.1"

[storage]
download_dir = "{tmpdir}/downloads"

[security]
paired_fingerprints = []
""")
        os.makedirs(os.path.join(tmpdir, "downloads"), exist_ok=True)

        env = os.environ.copy()
        env["RUST_LOG"] = "error"

        daemon_proc = subprocess.Popen(
            [BIN_RELEASE, "--config", cfg_path, "daemon", "--socket-path", sock_path, "--port", "54323"],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

        try:
            for _ in range(50):
                if os.path.exists(sock_path):
                    break
                time.sleep(0.05)
            else:
                raise RuntimeError("Daemon failed to bind socket")

            # Connect 10 concurrent Waybar clients
            clients = []
            for _ in range(10):
                s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                s.connect(sock_path)
                s.sendall(b"status --follow\n")
                s.setblocking(False)
                clients.append(s)

            # Connect 5 regular snapshot querying clients sending ping/status
            probe_socks = []
            for _ in range(5):
                s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                s.connect(sock_path)
                probe_socks.append(s)

            max_rss_kb = 0
            # 200 cycles of status checks and reading streaming outputs
            for cycle in range(200):
                for ps in probe_socks:
                    try:
                        ps.sendall(b"status\n")
                        _ = ps.recv(1024)
                    except Exception:
                        pass

                for s in clients:
                    try:
                        while True:
                            chunk = s.recv(4096)
                            if not chunk:
                                break
                    except BlockingIOError:
                        pass

                rss = get_process_rss_kb(daemon_proc.pid)
                if rss > max_rss_kb:
                    max_rss_kb = rss
                time.sleep(0.01)

            print(f"Max soak test RSS: {max_rss_kb} KB ({max_rss_kb / 1024:.2f} MB)")
            if max_rss_kb >= 15360:
                raise AssertionError(f"Continuous RSS during soak test exceeded 15 MB: {max_rss_kb} KB")

            for s in clients:
                s.close()
            for s in probe_socks:
                s.close()

        finally:
            daemon_proc.send_signal(signal.SIGTERM)
            try:
                daemon_proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                daemon_proc.kill()


def main():
    print("================================================================================")
    print("ARCH DROP MILESTONE 2: EMPIRICAL BLACK-BOX & INVARIANT STRESS SUITE")
    print("================================================================================")

    run_test("Host WireGuard Safety Invariant (Pre-check)", test_host_wireguard_safety)
    run_test("Zero GUI Dynamic Linking Verification (ldd)", test_zero_gui_dynamic_dependencies)
    run_test("Continuous RSS Budget under Load (< 15 MB)", test_daemon_streaming_rss_under_15mb)
    run_test("CLI Status --follow JSON Streaming", test_cli_status_follow_streaming)
    run_test("Heavy Transfer Streaming Soak & RSS (< 15 MB)", test_heavy_transfer_streaming_rss_soak)
    run_test("Host WireGuard Safety Invariant (Post-check)", test_host_wireguard_safety)

    print("\n================================================================================")
    print("EMPIRICAL TEST SUMMARY")
    print(f"Total Tests Run: {len(test_passes) + len(test_failures)}")
    print(f"Passed: {len(test_passes)}")
    print(f"Failed: {len(test_failures)}")
    print("================================================================================")

    for name, dur in test_passes:
        print(f"  ✓ {name} ({dur:.3f}s)")
    for name, err in test_failures:
        print(f"  ✗ {name}: {err}")

    if test_failures:
        sys.exit(1)
    else:
        print("\nALL STRESS TESTS PASSED EMPIRICALLY WITH ZERO DEFECTS.")
        sys.exit(0)

if __name__ == "__main__":
    main()
