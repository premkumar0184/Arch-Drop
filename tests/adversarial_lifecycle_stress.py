#!/usr/bin/env python3
"""
Empirical Adversarial Stress Harness for Arch Drop Milestone 1
Daemon Lifecycle, UNIX Domain Socket IPC, Signals & Host Safety Invariants.

This test suite executes empirical, black-box, and white-box stress testing
against the compiled `archdropd` binary and its IPC/systemd infrastructure.
"""

import os
import sys
import time
import socket
import signal
import subprocess
import tempfile
import stat
import json
import threading

PROJECT_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
BIN_RELEASE = os.path.join(PROJECT_ROOT, "target", "release", "archdropd")
BIN_DEBUG = os.path.join(PROJECT_ROOT, "target", "debug", "archdropd")

BIN_PATH = os.environ.get("ARCHDROPD_BIN", BIN_RELEASE if os.path.exists(BIN_RELEASE) else BIN_DEBUG)

print(f"[TEST HARNESS] Using binary: {BIN_PATH}")

test_failures = []
test_passes = []

def run_test(name, func):
    print(f"\n=======================================================")
    print(f"[RUNNING TEST] {name}")
    print(f"=======================================================")
    start = time.time()
    try:
        func()
        duration = time.time() - start
        print(f"[PASSED] {name} ({duration:.3f}s)")
        test_passes.append((name, duration))
    except Exception as e:
        duration = time.time() - start
        print(f"[FAILED] {name} ({duration:.3f}s): {e}")
        import traceback
        traceback.print_exc()
        test_failures.append((name, str(e)))

# Helper to find a free port
def get_free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]

def wait_for_socket(sock_path, timeout=5.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if os.path.exists(sock_path):
            try:
                s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                s.connect(sock_path)
                s.close()
                return True
            except (socket.error, ConnectionRefusedError):
                pass
        time.sleep(0.05)
    return False

# ---------------------------------------------------------------------------
# Invariant: Host WireGuard Safety
# ---------------------------------------------------------------------------
def assert_wireguard_safety():
    out = subprocess.check_output(["ip", "addr", "show", "tiktok"]).decode()
    assert "10.2.0.2/32" in out, f"WireGuard IP 10.2.0.2/32 missing: {out}"
    assert "state UNKNOWN" in out or "UP" in out, f"tiktok interface not UP: {out}"

    rule_out = subprocess.check_output(["ip", "rule", "show"]).decode()
    assert "51820" in rule_out, f"Routing table rule 51820 missing: {rule_out}"

    route_out = subprocess.check_output(["ip", "route", "show", "table", "51820"]).decode()
    assert "tiktok" in route_out, f"Route in table 51820 missing tiktok: {route_out}"

def test_host_wireguard_safety():
    assert_wireguard_safety()
    print("Host WireGuard interface tiktok (10.2.0.2/32) and rule 51820 verified intact.")

# ---------------------------------------------------------------------------
# Test 1: Single SIGTERM Graceful Shutdown (< 5.0s, exit code 0, socket unlinked)
# ---------------------------------------------------------------------------
def test_sigterm_graceful_shutdown():
    with tempfile.TemporaryDirectory() as tmpdir:
        sock_path = os.path.join(tmpdir, "archdrop", "archdropd.sock")
        port = get_free_port()
        cmd = [BIN_PATH, "daemon", "--port", str(port), "--socket-path", sock_path, "--no-tls"]

        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            assert wait_for_socket(sock_path, timeout=5.0), "Socket did not appear within 5s"
            assert os.path.exists(sock_path), "Socket file missing on disk"

            start_t = time.time()
            proc.send_signal(signal.SIGTERM)
            ret = proc.wait(timeout=6.0)
            elapsed = time.time() - start_t

            assert ret == 0, f"Expected returncode 0 on SIGTERM, got {ret}"
            assert elapsed < 5.0, f"Shutdown took too long: {elapsed:.3f}s (must be < 5.0s drain window)"
            assert not os.path.exists(sock_path), "Socket file was not unlinked on graceful SIGTERM exit!"
            print(f"Graceful SIGTERM shutdown completed in {elapsed:.3f}s with exit code 0. Socket unlinked.")
        finally:
            if proc.poll() is None:
                proc.kill()

# ---------------------------------------------------------------------------
# Test 2: Single SIGINT (Ctrl+C) Graceful Shutdown
# ---------------------------------------------------------------------------
def test_sigint_graceful_shutdown():
    with tempfile.TemporaryDirectory() as tmpdir:
        sock_path = os.path.join(tmpdir, "archdrop", "archdropd.sock")
        port = get_free_port()
        cmd = [BIN_PATH, "daemon", "--port", str(port), "--socket-path", sock_path, "--no-tls"]

        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            assert wait_for_socket(sock_path, timeout=5.0), "Socket did not appear within 5s"

            start_t = time.time()
            proc.send_signal(signal.SIGINT)
            ret = proc.wait(timeout=6.0)
            elapsed = time.time() - start_t

            assert ret == 0, f"Expected returncode 0 on SIGINT, got {ret}"
            assert elapsed < 5.0, f"Shutdown took too long: {elapsed:.3f}s"
            assert not os.path.exists(sock_path), "Socket file was not unlinked on graceful SIGINT exit!"
            print(f"Graceful SIGINT shutdown completed in {elapsed:.3f}s with exit code 0. Socket unlinked.")
        finally:
            if proc.poll() is None:
                proc.kill()

# ---------------------------------------------------------------------------
# Test 3: Rapid Signal Bombardment (Multiple SIGINT / SIGTERM in bursts)
# ---------------------------------------------------------------------------
def test_rapid_signal_bombardment():
    with tempfile.TemporaryDirectory() as tmpdir:
        sock_path = os.path.join(tmpdir, "archdrop", "archdropd.sock")
        port = get_free_port()
        cmd = [BIN_PATH, "daemon", "--port", str(port), "--socket-path", sock_path, "--no-tls"]

        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            assert wait_for_socket(sock_path, timeout=5.0), "Socket did not appear within 5s"

            # Fire rapid alternating signals
            start_t = time.time()
            for sig in [signal.SIGINT, signal.SIGTERM, signal.SIGINT, signal.SIGTERM, signal.SIGINT]:
                try:
                    proc.send_signal(sig)
                except ProcessLookupError:
                    break
                time.sleep(0.01)

            ret = proc.wait(timeout=6.0)
            elapsed = time.time() - start_t

            assert ret == 0, f"Expected returncode 0 on signal burst, got {ret}"
            assert elapsed < 5.0, f"Shutdown took too long under signal barrage: {elapsed:.3f}s"
            assert not os.path.exists(sock_path), "Socket file unlinked under signal barrage"
            print(f"Burst signal test passed: exit code {ret} in {elapsed:.3f}s.")
        finally:
            if proc.poll() is None:
                proc.kill()

# ---------------------------------------------------------------------------
# Test 4: IPC Client Connected During SIGTERM (Drain & Clean Disconnect)
# ---------------------------------------------------------------------------
def test_shutdown_with_active_ipc_client():
    with tempfile.TemporaryDirectory() as tmpdir:
        sock_path = os.path.join(tmpdir, "archdrop", "archdropd.sock")
        port = get_free_port()
        cmd = [BIN_PATH, "daemon", "--port", str(port), "--socket-path", sock_path, "--no-tls"]

        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        client = None
        try:
            assert wait_for_socket(sock_path, timeout=5.0), "Socket did not appear"

            client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            client.connect(sock_path)
            client.settimeout(2.0)

            # Read initial status snapshot
            initial_line = client.recv(1024).decode()
            assert "idle" in initial_line or "timestamp" in initial_line, f"Unexpected snapshot: {initial_line}"

            # Send ping
            client.sendall(b"ping\n")
            pong = client.recv(1024).decode()
            assert "pong" in pong, f"Expected pong, got {pong}"

            # Now send SIGTERM to daemon while client is connected
            start_t = time.time()
            proc.send_signal(signal.SIGTERM)

            # Client should observe EOF
            eof_observed = False
            try:
                data = client.recv(1024)
                if not data:
                    eof_observed = True
            except socket.error:
                eof_observed = True

            ret = proc.wait(timeout=6.0)
            elapsed = time.time() - start_t

            assert ret == 0, f"Daemon exited with non-zero code {ret} with active client"
            assert elapsed < 5.0, f"Drain timeout exceeded with active client: {elapsed:.3f}s"
            assert not os.path.exists(sock_path), "Socket not unlinked after client disconnect"
            print(f"Active IPC client graceful drain passed in {elapsed:.3f}s.")
        finally:
            if client:
                try:
                    client.close()
                except Exception:
                    pass
            if proc.poll() is None:
                proc.kill()

# ---------------------------------------------------------------------------
# Test 5: Concurrent Socket Bind Attempt (Dual Instance Conflict Rejection)
# ---------------------------------------------------------------------------
def test_concurrent_socket_conflict_rejection():
    with tempfile.TemporaryDirectory() as tmpdir:
        sock_path = os.path.join(tmpdir, "archdrop", "archdropd.sock")
        port1 = get_free_port()
        port2 = get_free_port()

        # Instance 1
        cmd1 = [BIN_PATH, "daemon", "--port", str(port1), "--socket-path", sock_path, "--no-tls"]
        proc1 = subprocess.Popen(cmd1, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

        try:
            assert wait_for_socket(sock_path, timeout=5.0), "Instance 1 socket failed to bind"

            # Instance 2 attempts to bind to the SAME socket path
            cmd2 = [BIN_PATH, "daemon", "--port", str(port2), "--socket-path", sock_path, "--no-tls"]
            proc2 = subprocess.Popen(cmd2, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            out2, err2 = proc2.communicate(timeout=5.0)

            ret2 = proc2.returncode
            combined2 = (out2 + err2).decode()

            assert ret2 != 0, f"Instance 2 must fail to bind, but exited with code {ret2}"
            assert "already running" in combined2, f"Instance 2 error message must state daemon is already running: {combined2}"

            # Verify Instance 1 is still alive and serving
            assert proc1.poll() is None, "Instance 1 died after Instance 2 collision"
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.connect(sock_path)
            s.close()

            # Clean shutdown of Instance 1
            proc1.send_signal(signal.SIGTERM)
            assert proc1.wait(timeout=5.0) == 0, "Instance 1 failed to shut down cleanly"
            assert not os.path.exists(sock_path), "Socket not unlinked after Instance 1 exit"
            print("Concurrent socket conflict rejection test passed: second instance rejected cleanly.")
        finally:
            if proc1.poll() is None:
                proc1.kill()

# ---------------------------------------------------------------------------
# Test 6: Stale Socket File Recovery (Dead Socket / Ungraceful Crash Recovery)
# ---------------------------------------------------------------------------
def test_stale_socket_recovery():
    with tempfile.TemporaryDirectory() as tmpdir:
        sock_path = os.path.join(tmpdir, "archdrop", "archdropd.sock")
        port = get_free_port()

        # Scenario A: Stale regular file
        os.makedirs(os.path.dirname(sock_path), exist_ok=True)
        with open(sock_path, "w") as f:
            f.write("dead socket contents from previous crash")
        assert os.path.exists(sock_path)

        cmd = [BIN_PATH, "daemon", "--port", str(port), "--socket-path", sock_path, "--no-tls"]
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            assert wait_for_socket(sock_path, timeout=5.0), "Daemon failed to recover from stale regular file"
            proc.send_signal(signal.SIGTERM)
            assert proc.wait(timeout=5.0) == 0, "Failed graceful shutdown after stale file recovery"
            assert not os.path.exists(sock_path)
            print("Subcase A (stale regular file) passed.")
        finally:
            if proc.poll() is None:
                proc.kill()

        # Scenario B: Simulated SIGKILL crash leaving real UNIX socket file
        port = get_free_port()
        cmd = [BIN_PATH, "daemon", "--port", str(port), "--socket-path", sock_path, "--no-tls"]
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            assert wait_for_socket(sock_path, timeout=5.0)
            # Kill with SIGKILL (-9) so RAII drop does NOT run
            proc.kill()
            proc.wait()
            assert os.path.exists(sock_path), "Socket should remain after SIGKILL"

            # Now start new daemon: must detect dead socket and recover
            port2 = get_free_port()
            cmd2 = [BIN_PATH, "daemon", "--port", str(port2), "--socket-path", sock_path, "--no-tls"]
            proc2 = subprocess.Popen(cmd2, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                assert wait_for_socket(sock_path, timeout=5.0), "Daemon failed to recover from dead socket after SIGKILL"
                proc2.send_signal(signal.SIGTERM)
                assert proc2.wait(timeout=5.0) == 0
                assert not os.path.exists(sock_path)
                print("Subcase B (SIGKILL crash recovery) passed.")
            finally:
                if proc2.poll() is None:
                    proc2.kill()
        finally:
            if proc.poll() is None:
                proc.kill()

# ---------------------------------------------------------------------------
# Test 7: Strict Permission Verification (0o700 Parent Dir & 0o600 Socket)
# ---------------------------------------------------------------------------
def test_permissions_strictness():
    with tempfile.TemporaryDirectory() as tmpdir:
        parent_dir = os.path.join(tmpdir, "archdrop_test_perm")
        sock_path = os.path.join(parent_dir, "archdropd.sock")
        port = get_free_port()

        cmd = [BIN_PATH, "daemon", "--port", str(port), "--socket-path", sock_path, "--no-tls"]
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            assert wait_for_socket(sock_path, timeout=5.0)

            # Check parent directory permissions
            dir_st = os.stat(parent_dir)
            dir_mode = stat.S_IMODE(dir_st.st_mode)
            assert dir_mode == 0o700, f"Expected parent dir mode 0o700, got {oct(dir_mode)}"

            # Check socket permissions
            sock_st = os.stat(sock_path)
            sock_mode = stat.S_IMODE(sock_st.st_mode)
            assert sock_mode == 0o600, f"Expected socket mode 0o600, got {oct(sock_mode)}"

            print(f"Strict permissions verified: parent dir={oct(dir_mode)}, socket={oct(sock_mode)}.")
            proc.send_signal(signal.SIGTERM)
            assert proc.wait(timeout=5.0) == 0
        finally:
            if proc.poll() is None:
                proc.kill()

# ---------------------------------------------------------------------------
# Test 8: Systemd User Unit File Verification (systemd-analyze)
# ---------------------------------------------------------------------------
def test_systemd_analyze_verify():
    unit_path = os.path.join(PROJECT_ROOT, "systemd", "user", "archdropd.service")
    assert os.path.exists(unit_path), f"Unit file missing: {unit_path}"

    cmd = ["systemd-analyze", "--user", "verify", unit_path]
    res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

    assert res.returncode == 0, f"systemd-analyze failed with code {res.returncode}:\nstdout: {res.stdout}\nstderr: {res.stderr}"
    print(f"systemd-analyze --user verify passed with returncode 0.")

# ---------------------------------------------------------------------------
# Test 9: Scripts and Packaging Sanity
# ---------------------------------------------------------------------------
def test_scripts_and_packaging():
    scripts = [
        os.path.join(PROJECT_ROOT, "scripts", "install-service.sh"),
        os.path.join(PROJECT_ROOT, "scripts", "uninstall-service.sh"),
        os.path.join(PROJECT_ROOT, "scripts", "manage-service.sh"),
    ]
    for s in scripts:
        assert os.path.exists(s), f"Script missing: {s}"
        assert os.access(s, os.X_OK), f"Script not executable: {s}"
        res = subprocess.run(["bash", "-n", s])
        assert res.returncode == 0, f"Syntax error in script {s}"

    pkgbuild = os.path.join(PROJECT_ROOT, "packaging", "arch", "PKGBUILD")
    assert os.path.exists(pkgbuild), f"PKGBUILD missing: {pkgbuild}"
    with open(pkgbuild) as f:
        content = f.read()
    assert "pkgname=archdropd" in content
    assert "systemd/user/archdropd.service" in content
    print("All service management scripts and Arch PKGBUILD verified.")


# ---------------------------------------------------------------------------
# Test 10: Parent Directory Hardening (Pre-existing 0o755 corrected to 0o700)
# ---------------------------------------------------------------------------
def test_directory_hardening():
    with tempfile.TemporaryDirectory() as tmpdir:
        parent_dir = os.path.join(tmpdir, "permissive_dir")
        os.makedirs(parent_dir, mode=0o755, exist_ok=True)
        os.chmod(parent_dir, 0o755)
        assert stat.S_IMODE(os.stat(parent_dir).st_mode) == 0o755

        sock_path = os.path.join(parent_dir, "archdropd.sock")
        port = get_free_port()
        cmd = [BIN_PATH, "daemon", "--port", str(port), "--socket-path", sock_path, "--no-tls"]
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            assert wait_for_socket(sock_path, timeout=5.0)
            dir_mode = stat.S_IMODE(os.stat(parent_dir).st_mode)
            assert dir_mode == 0o700, f"Parent directory should be tightened from 0o755 to 0o700, got {oct(dir_mode)}"
            proc.send_signal(signal.SIGTERM)
            assert proc.wait(timeout=5.0) == 0
            print("Directory permission hardening test passed (0o755 tightened to 0o700).")
        finally:
            if proc.poll() is None:
                proc.kill()

# ---------------------------------------------------------------------------
# Test 11: SIGHUP Graceful Shutdown
# ---------------------------------------------------------------------------
def test_sighup_graceful_shutdown():
    with tempfile.TemporaryDirectory() as tmpdir:
        sock_path = os.path.join(tmpdir, "archdrop", "archdropd.sock")
        port = get_free_port()
        cmd = [BIN_PATH, "daemon", "--port", str(port), "--socket-path", sock_path, "--no-tls"]
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            assert wait_for_socket(sock_path, timeout=5.0)
            start_t = time.time()
            proc.send_signal(signal.SIGHUP)
            ret = proc.wait(timeout=6.0)
            elapsed = time.time() - start_t

            assert ret == 0, f"Expected returncode 0 on SIGHUP, got {ret}"
            assert elapsed < 5.0, f"Shutdown took too long: {elapsed:.3f}s"
            assert not os.path.exists(sock_path), "Socket not unlinked after SIGHUP exit"
            print(f"SIGHUP graceful shutdown passed in {elapsed:.3f}s with exit code 0.")
        finally:
            if proc.poll() is None:
                proc.kill()

# ---------------------------------------------------------------------------
# Test 12: Mass Concurrent IPC Clients (30 Connections) during SIGTERM
# ---------------------------------------------------------------------------
def test_mass_concurrent_ipc_clients():
    with tempfile.TemporaryDirectory() as tmpdir:
        sock_path = os.path.join(tmpdir, "archdrop", "archdropd.sock")
        port = get_free_port()
        cmd = [BIN_PATH, "daemon", "--port", str(port), "--socket-path", sock_path, "--no-tls"]
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        clients = []
        try:
            assert wait_for_socket(sock_path, timeout=5.0)
            for _ in range(30):
                c = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                c.connect(sock_path)
                c.settimeout(2.0)
                # Read snapshot
                _ = c.recv(1024)
                clients.append(c)

            # Send ping on all
            for c in clients:
                c.sendall(b"ping\n")

            start_t = time.time()
            proc.send_signal(signal.SIGTERM)

            ret = proc.wait(timeout=6.0)
            elapsed = time.time() - start_t

            assert ret == 0, f"Expected returncode 0 with 30 clients, got {ret}"
            assert elapsed < 5.0, f"Drain with 30 clients took too long: {elapsed:.3f}s"
            assert not os.path.exists(sock_path)
            print(f"Mass concurrent IPC clients test passed: 30 clients cleanly handled in {elapsed:.3f}s.")
        finally:
            for c in clients:
                try:
                    c.close()
                except Exception:
                    pass
            if proc.poll() is None:
                proc.kill()

# ---------------------------------------------------------------------------
# Test 13: Malformed & Fuzzed IPC Query Input
# ---------------------------------------------------------------------------
def test_fuzzed_ipc_queries():
    with tempfile.TemporaryDirectory() as tmpdir:
        sock_path = os.path.join(tmpdir, "archdrop", "archdropd.sock")
        port = get_free_port()
        cmd = [BIN_PATH, "daemon", "--port", str(port), "--socket-path", sock_path, "--no-tls"]
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        client = None
        try:
            assert wait_for_socket(sock_path, timeout=5.0)
            client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            client.connect(sock_path)
            client.settimeout(2.0)

            # Read initial snapshot
            _ = client.recv(1024)

            # Send malformed fuzzing payloads
            fuzz_payloads = [
                b"\n\n\n\r\n",
                b"UNKNOWN_COMMAND_FOOBAR_123\n",
                b"{\"malformed_json\": true\n",
                b"ping ping ping ping\n",
                b"\x00\x01\x02\x03\n",
                b"A" * 8192 + b"\n",
                b"status\n",
            ]
            for p in fuzz_payloads:
                client.sendall(p)
                time.sleep(0.01)

            # Ensure daemon did not crash and responds to valid status
            resp = client.recv(4096).decode()
            assert "idle" in resp or "timestamp" in resp, f"Expected valid response after fuzzing, got: {resp}"

            proc.send_signal(signal.SIGTERM)
            assert proc.wait(timeout=5.0) == 0
            assert not os.path.exists(sock_path)
            print("Fuzzed IPC queries test passed: daemon remained stable and responsive.")
        finally:
            if client:
                try:
                    client.close()
                except Exception:
                    pass
            if proc.poll() is None:
                proc.kill()

# ---------------------------------------------------------------------------
# Test 14: Immediate Signal On Launch (Startup Race Window & Recovery)
# ---------------------------------------------------------------------------
def test_immediate_signal_on_launch():
    """
    Adversarially probe the daemon's behavior when a signal arrives immediately
    after process spawn (before or after signal handler registration).
    If signal arrives before registration, kernel terminates with SIGTERM (-15).
    We verify that in both cases, the system survives without hanging or leaking
    unrecoverable socket state.
    """
    for sig in [signal.SIGTERM, signal.SIGINT]:
        with tempfile.TemporaryDirectory() as tmpdir:
            sock_path = os.path.join(tmpdir, "archdrop", "archdropd.sock")
            port = get_free_port()
            cmd = [BIN_PATH, "daemon", "--port", str(port), "--socket-path", sock_path, "--no-tls"]
            proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            # Immediate signal during launch
            time.sleep(0.005)
            proc.send_signal(sig)
            ret = proc.wait(timeout=6.0)
            
            # The process either caught the signal (exit code 0) or was terminated
            # by default signal disposition before handlers were registered (ret == -sig or 128+sig).
            expected_codes = [0, -sig, 128 + sig]
            assert ret in expected_codes, f"Unexpected returncode {ret} on immediate signal {sig}"

            # If a socket was created before termination, verify subsequent daemon run can recover
            if os.path.exists(sock_path):
                print(f"  [Note] Socket was left by pre-handler termination; verifying auto-recovery...")
                port2 = get_free_port()
                cmd2 = [BIN_PATH, "daemon", "--port", str(port2), "--socket-path", sock_path, "--no-tls"]
                proc2 = subprocess.Popen(cmd2, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                try:
                    assert wait_for_socket(sock_path, timeout=5.0), "Recovery after immediate signal failed"
                    proc2.send_signal(signal.SIGTERM)
                    assert proc2.wait(timeout=5.0) == 0
                    assert not os.path.exists(sock_path)
                finally:
                    if proc2.poll() is None:
                        proc2.kill()

    print("Immediate signal on launch test passed: handled safely with full stale-state recovery.")

def main():
    print("=" * 60)
    print("STARTING EMPIRICAL ADVERSARIAL STRESS TEST SUITE")
    print("=" * 60)

    # Initial WireGuard Invariant Check
    run_test("Host WireGuard Safety (Pre-check)", test_host_wireguard_safety)

    # Signal Handling Stress
    run_test("Signal Handling: Single SIGTERM Graceful Shutdown", test_sigterm_graceful_shutdown)
    run_test("Signal Handling: Single SIGINT Graceful Shutdown", test_sigint_graceful_shutdown)
    run_test("Signal Handling: SIGHUP Graceful Shutdown", test_sighup_graceful_shutdown)
    run_test("Signal Handling: Rapid Signal Bombardment (Burst)", test_rapid_signal_bombardment)
    run_test("Signal Handling: Immediate Signal on Launch (Race)", test_immediate_signal_on_launch)
    run_test("Signal Handling: Active IPC Client Drain on SIGTERM", test_shutdown_with_active_ipc_client)
    run_test("Signal Handling: Mass Concurrent IPC Clients (30 Conn)", test_mass_concurrent_ipc_clients)

    # IPC Lifecycle & Conflict Stress
    run_test("IPC Socket: Concurrent Dual Instance Conflict Rejection", test_concurrent_socket_conflict_rejection)
    run_test("IPC Socket: Stale File & Crash (SIGKILL) Recovery", test_stale_socket_recovery)
    run_test("IPC Socket: Strict File Permissions (0o700 / 0o600)", test_permissions_strictness)
    run_test("IPC Socket: Directory Permission Hardening (0o755 -> 0o700)", test_directory_hardening)
    run_test("IPC Socket: Fuzzed & Malformed IPC Queries", test_fuzzed_ipc_queries)

    # Systemd & Packaging Verification
    run_test("Systemd Unit: systemd-analyze --user verify", test_systemd_analyze_verify)
    run_test("Packaging: Scripts & PKGBUILD Verification", test_scripts_and_packaging)

    # Final WireGuard Invariant Check
    run_test("Host WireGuard Safety (Post-check)", test_host_wireguard_safety)

    print("\n" + "=" * 60)
    print("EMPIRICAL TEST SUMMARY")
    print(f"Total Tests Run: {len(test_passes) + len(test_failures)}")
    print(f"Passed: {len(test_passes)}")
    print(f"Failed: {len(test_failures)}")
    print("=" * 60)

    for name, dur in test_passes:
        print(f"  ✓ {name} ({dur:.3f}s)")
    for name, err in test_failures:
        print(f"  ✗ {name}: {err}")

    if test_failures:
        print("\nOVERALL VERDICT: FAILED")
        sys.exit(1)
    else:
        print("\nOVERALL VERDICT: CONFIRMED")
        sys.exit(0)

if __name__ == "__main__":
    main()
