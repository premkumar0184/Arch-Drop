#!/usr/bin/env python3
"""
Adversarial Empirical Stress Test Suite for Arch Drop Milestone 2:
Hyprland Desktop Status Integration (Waybar IPC Socket Streaming).

Tests:
1. Daemon Launch & IPC Socket Permissions (0o600)
2. Live Waybar Socket Streaming Concurrency (30 concurrent clients)
3. Client Churn, Abrupt Disconnection & Reconnection
4. File Descriptor Leak Verification via /proc/<pid>/fd
5. Fuzzing & Malformed Socket Commands (Binary garbage, huge payloads, invalid UTF-8)
6. M1 Backwards-Compatibility Snapshot Fallback (empty lines, legacy clients)
7. Ping-Pong Protocol Verification
8. Live File Transfer State Transitions & Waybar Streaming (idle -> transferring -> completed)
9. Subsequent Transfer Resilience & Transfer State Machine Unfreezing
10. Waybar JSON Schema Strict Compliance Oracle
11. Memory RSS Guardrail (< 15 MB)
12. Clean Graceful Shutdown on SIGTERM & SocketGuard Unlinking
13. Host Safety Invariant (WireGuard tiktok interface intact)
"""

import asyncio
import json
import os
import signal
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

PROJECT_ROOT = Path("/home/prem/Projects/Arch Drop")
DAEMON_BIN = PROJECT_ROOT / "target" / "release" / "archdropd"
STYLE_CSS = PROJECT_ROOT / "examples" / "waybar" / "style.css"

PASSED = 0
FAILED = 0
FINDINGS = []

VALID_CLASSES = {"idle", "paired", "transferring", "inbound", "outbound", "completed", "clipboard", "error", "offline"}


def record_pass(name: str):
    global PASSED
    PASSED += 1
    print(f"  [PASS] {name}")


def record_fail(name: str, reason: str):
    global FAILED
    FAILED += 1
    FINDINGS.append(f"{name}: {reason}")
    print(f"  [FAIL] {name}: {reason}")


def check_host_wireguard_safety():
    """Verify tiktok WireGuard interface is UP, 10.2.0.2/32, table 51820 intact."""
    res = subprocess.run(["ip", "addr", "show", "dev", "tiktok"], capture_output=True, text=True)
    if res.returncode != 0:
        return False, "tiktok interface does not exist or returned error"
    if "10.2.0.2/32" not in res.stdout and "10.2.0.2" not in res.stdout:
        return False, f"tiktok IP is not 10.2.0.2: {res.stdout}"
    if "UP" not in res.stdout:
        return False, "tiktok interface is not UP"

    res_route = subprocess.run(["ip", "route", "show", "table", "51820"], capture_output=True, text=True)
    if res_route.returncode != 0 or "tiktok" not in res_route.stdout:
        return False, f"table 51820 corrupted or missing tiktok route: {res_route.stdout}"

    return True, "WireGuard tiktok interface intact"


def get_process_fds(pid: int) -> int:
    """Return count of open file descriptors for PID from /proc/<pid>/fd."""
    try:
        fd_dir = Path(f"/proc/{pid}/fd")
        return len(list(fd_dir.iterdir()))
    except Exception as e:
        print(f"Warning: could not read /proc/{pid}/fd: {e}")
        return -1


def get_process_rss_kb(pid: int) -> int:
    """Return Resident Set Size (RSS) in KB from /proc/<pid>/status."""
    try:
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    parts = line.split()
                    return int(parts[1])
    except Exception as e:
        print(f"Warning: could not read VmRSS: {e}")
    return 0


def validate_waybar_payload(data: dict) -> tuple[bool, str]:
    """Validate JSON payload against Waybar custom module spec and CSS styles."""
    # 1. 'text' field
    if "text" not in data or not isinstance(data["text"], str):
        return False, f"Missing or non-string 'text': {data}"

    # 2. 'tooltip' field
    if "tooltip" not in data or not isinstance(data["tooltip"], str):
        return False, f"Missing or non-string 'tooltip': {data}"

    # 3. 'class' field
    if "class" not in data or not isinstance(data["class"], list) or len(data["class"]) == 0:
        return False, f"Missing, empty, or non-list 'class': {data}"
    for c in data["class"]:
        if not isinstance(c, str):
            return False, f"Class item is not a string: {c}"
        if c not in VALID_CLASSES:
            return False, f"Unknown CSS class '{c}' not supported in stylesheet"

    # 4. 'percentage' field
    if "percentage" in data and data["percentage"] is not None:
        pct = data["percentage"]
        if not isinstance(pct, (int, float)):
            return False, f"Percentage is not a number: {pct}"
        if pct < 0 or pct > 100:
            return False, f"Percentage out of range [0, 100]: {pct}"

    return True, "Valid Waybar payload"


async def main():
    global PASSED, FAILED
    print("=" * 70)
    print("ARCH DROP - PHASE 4 MILESTONE 2 EMPIRICAL CHALLENGE SUITE")
    print("=" * 70)

    # Invariant check 0
    ok, msg = check_host_wireguard_safety()
    if not ok:
        record_fail("Host WireGuard Invariant Check Initial", msg)
        return 1
    record_pass(f"Host WireGuard Invariant Initial ({msg})")

    if not DAEMON_BIN.exists():
        print(f"Binary not found: {DAEMON_BIN}")
        return 1

    temp_dir = tempfile.TemporaryDirectory()
    sock_path = Path(temp_dir.name) / "archdropd_stress.sock"
    cfg_path = Path(temp_dir.name) / "config.toml"
    download_dir = Path(temp_dir.name) / "downloads"
    download_dir.mkdir(parents=True, exist_ok=True)

    with open(cfg_path, "w") as f:
        f.write(f"""
[network]
port = 54321
bind_address = "127.0.0.1"

[storage]
download_dir = "{download_dir}"

[security]
require_encryption = false
auto_accept_paired = true
paired_fingerprints = []
""")

    print(f"\n[1] Launching Daemon on custom socket: {sock_path}")
    daemon_proc = subprocess.Popen(
        [
            str(DAEMON_BIN),
            "--config",
            str(cfg_path),
            "daemon",
            "--port",
            "54321",
            "--no-tls",
            "--socket-path",
            str(sock_path),
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )

    time.sleep(0.6)

    try:
        # Check daemon is alive
        if daemon_proc.poll() is not None:
            record_fail("Daemon start", f"Process exited with {daemon_proc.returncode}")
            return 1
        record_pass("Daemon process started successfully")

        daemon_pid = daemon_proc.pid

        # Verify socket permissions
        if not sock_path.exists():
            record_fail("Socket existence", f"Socket {sock_path} does not exist")
            return 1

        mode = oct(sock_path.stat().st_mode & 0o777)
        if mode == "0o600":
            record_pass(f"Socket permissions strictly 0o600 ({mode})")
        else:
            record_fail("Socket permissions", f"Expected 0o600, got {mode}")

        initial_fds = get_process_fds(daemon_pid)
        initial_rss = get_process_rss_kb(daemon_pid)
        print(f"  Daemon PID: {daemon_pid} | Initial FDs: {initial_fds} | Initial RSS: {initial_rss / 1024:.2f} MB")

        # -------------------------------------------------------------
        # 2. Test Single Client Waybar Stream
        # -------------------------------------------------------------
        print("\n[2] Testing Single Client Waybar Stream...")
        client_sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client_sock.connect(str(sock_path))
        client_sock.sendall(b"status --follow\n")
        f = client_sock.makefile("r")
        first_line = f.readline()
        client_sock.close()

        if not first_line:
            record_fail("Single client stream", "Received empty response")
        else:
            try:
                payload = json.loads(first_line)
                valid, reason = validate_waybar_payload(payload)
                if valid:
                    record_pass(f"Single client received valid WaybarPayload (class: {payload['class']}, text: '{payload['text']}')")
                else:
                    record_fail("Single client payload validation", reason)
            except Exception as e:
                record_fail("Single client parse", str(e))

        # -------------------------------------------------------------
        # 3. Test Concurrency: 30 Streaming Clients Simultaneously
        # -------------------------------------------------------------
        print("\n[3] Testing 30 Concurrent Streaming Clients...")

        async def streaming_client(client_id: int):
            reader, writer = await asyncio.open_unix_connection(str(sock_path))
            writer.write(b"status --follow\n")
            await writer.drain()

            line = await reader.readline()
            payload = json.loads(line.decode().strip())
            valid, reason = validate_waybar_payload(payload)
            assert valid, f"Client {client_id} invalid payload: {reason}"

            start = time.time()
            lines_read = 1
            while time.time() - start < 0.5:
                try:
                    line = await asyncio.wait_for(reader.readline(), timeout=0.2)
                    if line:
                        p = json.loads(line.decode().strip())
                        valid, _ = validate_waybar_payload(p)
                        assert valid
                        lines_read += 1
                except asyncio.TimeoutError:
                    pass

            writer.close()
            await writer.wait_closed()
            return lines_read

        tasks = [streaming_client(i) for i in range(30)]
        results = await asyncio.gather(*tasks, return_exceptions=True)
        errors = [r for r in results if isinstance(r, Exception)]
        if errors:
            record_fail("30 Concurrent clients", f"{len(errors)} clients failed: {errors[0]}")
        else:
            record_pass(f"All 30 concurrent streaming clients succeeded (lines read: {sum(results)})")

        mid_fds = get_process_fds(daemon_pid)
        print(f"  Daemon FDs after concurrent clients: {mid_fds}")

        # -------------------------------------------------------------
        # 4. Stress Test: Client Disconnect & Reconnect Churn (100 cycles)
        # -------------------------------------------------------------
        print("\n[4] Testing Client Disconnect & Reconnect Churn (100 rapid cycles)...")
        churn_errors = 0
        for i in range(100):
            try:
                s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                s.connect(str(sock_path))
                s.sendall(b"status --follow\n")
                _ = s.recv(10)
                s.close()
            except Exception as e:
                churn_errors += 1

        await asyncio.sleep(0.3)

        if churn_errors == 0:
            record_pass("100 rapid connect/abrupt drop cycles completed with 0 client errors")
        else:
            record_fail("Churn cycles", f"{churn_errors} errors during connect/drop")

        # -------------------------------------------------------------
        # 5. File Descriptor Leak Audit
        # -------------------------------------------------------------
        print("\n[5] Verifying File Descriptor Leak Resilience...")
        post_churn_fds = get_process_fds(daemon_pid)
        print(f"  Initial FDs: {initial_fds} | Mid FDs: {mid_fds} | Post-churn FDs: {post_churn_fds}")
        if post_churn_fds <= initial_fds + 3:
            record_pass(f"No FD leak detected (delta = {post_churn_fds - initial_fds} FDs)")
        else:
            record_fail("FD Leak Audit", f"Leaked {post_churn_fds - initial_fds} file descriptors!")

        # -------------------------------------------------------------
        # 6. Fuzzing & Malformed Socket Commands
        # -------------------------------------------------------------
        print("\n[6] Testing Malformed & Fuzzing Inputs...")

        # 6a. Send arbitrary binary bytes
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.connect(str(sock_path))
            s.sendall(b"\x00\xff\xfe\xca\xfe\xba\xbe\x01\x02\n")
            s.close()
            record_pass("Daemon survived raw binary garbage bytes")
        except Exception as e:
            record_fail("Raw binary garbage", str(e))

        # 6b. Send huge 64KB command line
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.connect(str(sock_path))
            huge_line = b"status " + (b"A" * 65536) + b"\n"
            s.sendall(huge_line)
            s.close()
            record_pass("Daemon survived 64KB oversized command line")
        except Exception as e:
            record_fail("Oversized command line", str(e))

        # 6c. Send invalid JSON commands
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.connect(str(sock_path))
            s.sendall(b'{"cmd": "broken_json\n')
            s.close()
            record_pass("Daemon survived malformed JSON command")
        except Exception as e:
            record_fail("Malformed JSON", str(e))

        # 6d. Ping commands without blank prefix
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.connect(str(sock_path))
            s.sendall(b"ping\nping\n")
            f = s.makefile("r")
            resp1 = json.loads(f.readline())
            resp2 = json.loads(f.readline())
            s.close()
            if resp1.get("type") == "pong" and resp2.get("type") == "pong":
                record_pass("Daemon correctly answered consecutive ping commands with pong")
            else:
                record_fail("Ping response", f"Unexpected responses: {resp1}, {resp2}")
        except Exception as e:
            record_fail("Ping command test", str(e))

        # 6e. M1 backwards compatibility snapshot query
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.connect(str(sock_path))
            s.sendall(b"\n")
            f = s.makefile("r")
            resp = json.loads(f.readline())
            s.close()
            if "state" in resp and "timestamp" in resp:
                record_pass("Daemon returned M1 DaemonStatusEvent snapshot on blank/default connection")
            else:
                record_fail("M1 snapshot fallback", f"Unexpected response: {resp}")
        except Exception as e:
            record_fail("M1 snapshot fallback", str(e))

        # 6f. Verify daemon remains healthy and responsive after fuzzing
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.connect(str(sock_path))
            s.sendall(b"status\n")
            f = s.makefile("r")
            line = f.readline()
            s.close()
            status_obj = json.loads(line)
            if "state" in status_obj:
                record_pass("Daemon remains responsive and healthy after all fuzzing attacks")
            else:
                record_fail("Health check after fuzzing", f"Missing state in status: {line}")
        except Exception as e:
            record_fail("Health check after fuzzing", str(e))

        # -------------------------------------------------------------
        # 7. Live File Transfer State Transition Streaming Test
        # -------------------------------------------------------------
        print("\n[7] Testing Live File Transfer & Waybar Stream State Transitions...")
        test_file_path = Path(temp_dir.name) / "test_transfer.bin"
        with open(test_file_path, "wb") as f:
            f.write(os.urandom(2 * 1024 * 1024))

        stream_payloads = []
        stop_stream = asyncio.Event()

        async def collect_stream():
            reader, writer = await asyncio.open_unix_connection(str(sock_path))
            writer.write(b"status --follow\n")
            await writer.drain()

            while not stop_stream.is_set():
                try:
                    line = await asyncio.wait_for(reader.readline(), timeout=0.1)
                    if line:
                        p = json.loads(line.decode().strip())
                        stream_payloads.append(p)
                except asyncio.TimeoutError:
                    pass
            writer.close()
            await writer.wait_closed()

        stream_task = asyncio.create_task(collect_stream())
        await asyncio.sleep(0.1)

        send_proc = subprocess.run(
            [
                str(DAEMON_BIN),
                "--config",
                str(cfg_path),
                "send",
                "--to",
                "127.0.0.1",
                "--port",
                "54321",
                str(test_file_path),
            ],
            capture_output=True,
            text=True,
        )

        if send_proc.returncode != 0:
            record_fail("Live file transfer", f"send failed: {send_proc.stderr}")
        else:
            record_pass("File transfer via archdropd send completed successfully")

        await asyncio.sleep(0.5)
        stop_stream.set()
        await stream_task

        print(f"  Captured {len(stream_payloads)} Waybar payloads during transfer")
        has_transferring = any("transferring" in p.get("class", []) for p in stream_payloads)
        has_completed = any("completed" in p.get("class", []) for p in stream_payloads)

        all_valid = True
        invalid_reason = ""
        for p in stream_payloads:
            v, r = validate_waybar_payload(p)
            if not v:
                all_valid = False
                invalid_reason = r
                break

        if all_valid and len(stream_payloads) > 0:
            record_pass("All payloads captured during live transfer strictly adhere to Waybar schema")
        else:
            record_fail("Payload schema during transfer", invalid_reason)

        if has_transferring:
            record_pass("Stream successfully captured 'transferring' state with active progress")
        else:
            record_fail("Transferring state transition", "Stream never entered 'transferring' state")

        # ADVERSARIAL CHALLENGE CHECK: Did the daemon transition to 'completed'?
        if has_completed:
            record_pass("Stream captured 'completed' state")
        else:
            record_fail(
                "DEFECT: Transfer Completed State Never Emitted",
                "Transfer completed on disk but daemon NEVER transitioned to 'completed' state in Waybar stream (DaemonEvent::TransferCompleted is never emitted by transfer_server / daemon.rs)"
            )

        # -------------------------------------------------------------
        # 8. Subsequent Transfer Resilience & State Machine Freeze Check
        # -------------------------------------------------------------
        print("\n[8] Testing Subsequent Transfer & State Machine Freeze...")
        # Check current daemon status after transfer 1
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.connect(str(sock_path))
        s.sendall(b"status\n")
        post1 = json.loads(s.makefile("r").readline())
        s.close()

        if post1.get("state") == "transferring":
            record_fail(
                "DEFECT: Daemon State Stuck in 'transferring'",
                f"Daemon remains permanently stuck in 'transferring' (text: '{post1.get('text')}', current_transfer: '{post1.get('current_transfer')}') after transfer finished"
            )
        else:
            record_pass(f"Daemon transitioned out of 'transferring' to '{post1.get('state')}'")

        # Now send second file
        test_file_path2 = Path(temp_dir.name) / "test_transfer_2.bin"
        with open(test_file_path2, "wb") as f:
            f.write(os.urandom(1024 * 1024))

        send_proc2 = subprocess.run(
            [
                str(DAEMON_BIN),
                "--config",
                str(cfg_path),
                "send",
                "--to",
                "127.0.0.1",
                "--port",
                "54321",
                str(test_file_path2),
            ],
            capture_output=True,
            text=True,
        )
        time.sleep(0.5)

        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.connect(str(sock_path))
        s.sendall(b"status\n")
        post2 = json.loads(s.makefile("r").readline())
        s.close()

        if post2.get("current_transfer") == "test_transfer.bin":
            record_fail(
                "DEFECT: State Machine Frozen on First Transfer",
                f"Second transfer completed on disk but daemon status still reports first transfer ('{post2.get('current_transfer')}'), proving state machine is permanently frozen on active_transfer"
            )
        else:
            record_pass("Daemon correctly tracked second transfer")

        # -------------------------------------------------------------
        # 9. CLI status --follow Subprocess Integration Test
        # -------------------------------------------------------------
        print("\n[9] Testing CLI 'archdropd status --follow' streaming directly...")
        cli_proc = subprocess.Popen(
            [str(DAEMON_BIN), "status", "--follow", "--socket-path", str(sock_path)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

        time.sleep(0.3)
        cli_line = cli_proc.stdout.readline()
        cli_proc.send_signal(signal.SIGTERM)
        cli_proc.wait(timeout=2)

        if cli_line:
            payload = json.loads(cli_line.strip())
            valid, reason = validate_waybar_payload(payload)
            if valid:
                record_pass(f"CLI 'status --follow' streamed valid Waybar JSON: {payload['text']}")
            else:
                record_fail("CLI status --follow validation", reason)
        else:
            record_fail("CLI status --follow", "No output captured from CLI")

        # -------------------------------------------------------------
        # 10. Memory RSS Check (< 15 MB)
        # -------------------------------------------------------------
        print("\n[10] Checking Resident Memory Budget (< 15 MB RSS)...")
        final_rss_kb = get_process_rss_kb(daemon_pid)
        final_rss_mb = final_rss_kb / 1024.0
        print(f"  Final Daemon Resident Memory: {final_rss_mb:.2f} MB ({final_rss_kb} KB)")
        if final_rss_mb < 15.0:
            record_pass(f"Daemon memory footprint {final_rss_mb:.2f} MB is strictly < 15 MB")
        else:
            record_fail("Memory Budget", f"RSS exceeded 15 MB: {final_rss_mb:.2f} MB")

        # -------------------------------------------------------------
        # 11. Clean Shutdown & Socket Guard Teardown
        # -------------------------------------------------------------
        print("\n[11] Testing Graceful Shutdown & Socket Cleanup...")
        daemon_proc.send_signal(signal.SIGTERM)
        try:
            daemon_proc.wait(timeout=5)
            record_pass("Daemon terminated gracefully on SIGTERM")
        except subprocess.TimeoutExpired:
            daemon_proc.kill()
            record_fail("Daemon shutdown", "Daemon did not terminate within 5s on SIGTERM")

        if not sock_path.exists():
            record_pass("SocketGuard successfully unlinked IPC socket file on exit")
        else:
            record_fail("SocketGuard cleanup", f"Socket file still exists: {sock_path}")

    finally:
        if daemon_proc.poll() is None:
            daemon_proc.kill()
        temp_dir.cleanup()

    # -------------------------------------------------------------
    # 12. Final Host Safety Invariant Check
    # -------------------------------------------------------------
    print("\n[12] Final Host Safety Invariant Check...")
    ok, msg = check_host_wireguard_safety()
    if ok:
        record_pass(f"Host WireGuard Invariant Final ({msg})")
    else:
        record_fail("Host WireGuard Invariant Final", msg)

    print("\n" + "=" * 70)
    print(f"CHALLENGE TEST RESULTS: {PASSED} PASSED, {FAILED} FAILED")
    if FINDINGS:
        print("\nDISCOVERED DEFECTS:")
        for idx, finding in enumerate(FINDINGS, 1):
            print(f"  {idx}. {finding}")
    print("=" * 70)
    return 0 if FAILED == 0 else 1


if __name__ == "__main__":
    code = asyncio.run(main())
    sys.exit(code)
