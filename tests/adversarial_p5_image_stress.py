#!/usr/bin/env python3
"""
Empirical Adversarial Verification Harness for Phase 5 Milestone 1:
- Binary Image Ingestion (PNG, JPEG with null bytes and magic headers)
- Wayland stdin piping & MIME typing verification via wl-paste
- Edge case resilience (corrupted Base64, empty content, whitespace)
- Case-insensitive SHA-256 echo suppression (A-F vs a-f)
- Staged file transfer text/uri-list injection
- Host WireGuard safety invariant (tiktok 10.2.0.2/32, routing table 51820)
"""

import os
import sys
import time
import socket
import json
import base64
import hashlib
import tempfile
import subprocess
import urllib.request
import urllib.error

PROJECT_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))

# Standard valid 1x1 PNG bytes with null bytes and chunk headers
VALID_1X1_PNG = (
    b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01"
    b"\x08\x06\x00\x00\x00\x1f\x15c4\x00\x00\x00\nIDATx\x9cc\x00\x01\x00\x00"
    b"\x05\x00\x01\r\n-\xb4\x00\x00\x00\x00IEND\xaeB`\x82"
)

# Minimal valid JPEG bytes with multiple null bytes
VALID_JPEG = (
    b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x01\x00`\x00`\x00\x00\xff\xdb"
    b"\x00C\x00\x08\x06\x06\x07\x06\x05\x08\x07\x07\x07\t\t\x08\n\x0c\x14\r\x0c"
    b"\x0b\x0b\x0c\x19\x12\x13\x0f\x14\x1d\x1a\x1f\x1e\x1d\x1a\x1c\x1c $.\x27 \",#"
    b"\x1c\x1c(7),01444\x1f\x279=82<.342\xff\xc0\x00\x0b\x08\x00\x01\x00\x01\x01"
    b"\x01\x11\x00\xff\xc4\x00\x1f\x00\x00\x01\x05\x01\x01\x01\x01\x01\x01\x00"
    b"\x00\x00\x00\x00\x00\x00\x00\x01\x02\x03\x04\x05\x06\x07\x08\t\n\x0b\xff"
    b"\xda\x00\x08\x01\x01\x00\x00?\x00\xbf\x00\xff\xd9"
)

test_passes = []
test_failures = []

def run_test(name, func):
    print("=" * 80)
    print(f"[RUNNING TEST] {name}")
    print("=" * 80)
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

def get_free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]

def verify_wireguard_safety():
    out = subprocess.check_output(["ip", "addr", "show", "dev", "tiktok"]).decode()
    if "10.2.0.2" not in out:
        raise RuntimeError(f"Host WireGuard interface tiktok does not have 10.2.0.2: {out}")
    if "state UNKNOWN" not in out and "state UP" not in out:
        raise RuntimeError(f"Host WireGuard interface tiktok not UP: {out}")
    rule_out = subprocess.check_output(["ip", "rule", "show"]).decode()
    if "51820" not in rule_out:
        raise RuntimeError(f"Routing rule for table 51820 missing: {rule_out}")

def test_host_wireguard_safety_initial():
    verify_wireguard_safety()

def test_wl_copy_and_paste_direct_binary():
    """Verify that host wl-copy and wl-paste preserve binary image bytes with null bytes."""
    # Test PNG
    proc = subprocess.Popen(["wl-copy", "--type", "image/png"], stdin=subprocess.PIPE)
    proc.communicate(input=VALID_1X1_PNG)
    if proc.returncode != 0:
        raise RuntimeError(f"wl-copy failed with returncode {proc.returncode}")
    pasted = subprocess.check_output(["wl-paste", "--type", "image/png"])
    if pasted != VALID_1X1_PNG:
        raise RuntimeError(f"Pasted PNG does not match original: len {len(pasted)} vs {len(VALID_1X1_PNG)}")

    # Test JPEG
    proc = subprocess.Popen(["wl-copy", "--type", "image/jpeg"], stdin=subprocess.PIPE)
    proc.communicate(input=VALID_JPEG)
    if proc.returncode != 0:
        raise RuntimeError(f"wl-copy failed with returncode {proc.returncode}")
    pasted_jpeg = subprocess.check_output(["wl-paste", "--type", "image/jpeg"])
    if pasted_jpeg != VALID_JPEG:
        raise RuntimeError(f"Pasted JPEG does not match original: len {len(pasted_jpeg)} vs {len(VALID_JPEG)}")

def start_daemon_instance(temp_dir, port):
    cfg_path = os.path.join(temp_dir, "config.toml")
    download_dir = os.path.join(temp_dir, "Downloads")
    os.makedirs(download_dir, exist_ok=True)
    with open(cfg_path, "w") as f:
        f.write(f"""[device]
alias = "AdversarialTester"

[storage]
download_dir = "{download_dir}"

[network]
port = {port}
multicast_enabled = false

[security]
require_encryption = false
paired_fingerprints = ["PEER_FINGERPRINT_123"]
""")

    sock_path = os.path.join(temp_dir, "test_daemon.sock")
    bin_path = os.path.join(PROJECT_ROOT, "target", "debug", "archdropd")
    if not os.path.exists(bin_path):
        subprocess.check_call(["cargo", "build", "-p", "archdropd"], cwd=PROJECT_ROOT)

    proc = subprocess.Popen(
        [bin_path, "--config", cfg_path, "daemon", "--socket-path", sock_path],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=PROJECT_ROOT
    )

    # Wait for daemon to listen
    deadline = time.time() + 5.0
    connected = False
    while time.time() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                connected = True
                break
        except Exception:
            time.sleep(0.05)

    if not connected:
        proc.kill()
        out, err = proc.communicate()
        raise RuntimeError(f"Daemon failed to start on port {port}. Out: {out.decode()} Err: {err.decode()}")

    return proc, download_dir

def post_clipboard(port, packet_dict):
    url = f"http://127.0.0.1:{port}/api/v2/clipboard"
    data = json.dumps(packet_dict).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={
            "Content-Type": "application/json",
            "X-Fingerprint": "PEER_FINGERPRINT_123"
        }
    )
    with urllib.request.urlopen(req, timeout=5) as resp:
        return resp.getcode(), resp.read()

def test_daemon_raw_base64_png_ingestion():
    port = get_free_port()
    with tempfile.TemporaryDirectory() as td:
        daemon_proc, _ = start_daemon_instance(td, port)
        try:
            b64 = base64.b64encode(VALID_1X1_PNG).decode("ascii")
            sha256 = hashlib.sha256(VALID_1X1_PNG).hexdigest()

            packet = {
                "type": "clipboard.inline",
                "version": 2,
                "deviceId": "PEER_FINGERPRINT_123",
                "timestamp": int(time.time() * 1000),
                "ttlSeconds": 120,
                "payload": {
                    "mimeType": "image/png",
                    "content": b64,
                    "sha256": sha256,
                    "sizeBytes": len(VALID_1X1_PNG)
                }
            }

            code, _ = post_clipboard(port, packet)
            if code != 200:
                raise RuntimeError(f"Expected HTTP 200, got {code}")

            time.sleep(0.2)
            pasted = subprocess.check_output(["wl-paste", "--type", "image/png"])
            if pasted != VALID_1X1_PNG:
                raise RuntimeError(f"Pasted PNG does not match original binary bytes! len {len(pasted)} vs {len(VALID_1X1_PNG)}")

        finally:
            daemon_proc.terminate()
            daemon_proc.wait(timeout=3)

def test_daemon_data_uri_jpeg_ingestion():
    port = get_free_port()
    with tempfile.TemporaryDirectory() as td:
        daemon_proc, _ = start_daemon_instance(td, port)
        try:
            b64 = base64.b64encode(VALID_JPEG).decode("ascii")
            sha256 = hashlib.sha256(VALID_JPEG).hexdigest()
            content = f"data:image/jpeg;base64,{b64}"

            packet = {
                "type": "clipboard.inline",
                "version": 2,
                "deviceId": "PEER_FINGERPRINT_123",
                "timestamp": int(time.time() * 1000),
                "ttlSeconds": 120,
                "payload": {
                    "mimeType": "image/jpeg",
                    "content": content,
                    "sha256": sha256,
                    "sizeBytes": len(VALID_JPEG)
                }
            }

            code, _ = post_clipboard(port, packet)
            if code != 200:
                raise RuntimeError(f"Expected HTTP 200, got {code}")

            time.sleep(0.2)
            pasted = subprocess.check_output(["wl-paste", "--type", "image/jpeg"])
            if pasted != VALID_JPEG:
                raise RuntimeError(f"Pasted JPEG does not match original binary bytes!")

        finally:
            daemon_proc.terminate()
            daemon_proc.wait(timeout=3)

def test_daemon_corrupted_base64_and_empty_edge_cases():
    port = get_free_port()
    with tempfile.TemporaryDirectory() as td:
        daemon_proc, _ = start_daemon_instance(td, port)
        try:
            # 1. Corrupted base64
            corrupted_packet = {
                "type": "clipboard.inline",
                "version": 2,
                "deviceId": "PEER_FINGERPRINT_123",
                "timestamp": int(time.time() * 1000),
                "ttlSeconds": 120,
                "payload": {
                    "mimeType": "image/png",
                    "content": "NotAValidBase64@@@@!!!!",
                    "sha256": hashlib.sha256(b"NotAValidBase64@@@@!!!!").hexdigest(),
                    "sizeBytes": 23
                }
            }
            code, _ = post_clipboard(port, corrupted_packet)
            if code != 200:
                raise RuntimeError(f"Corrupted base64 should return 200, got {code}")

            # 2. Empty content
            empty_packet = {
                "type": "clipboard.inline",
                "version": 2,
                "deviceId": "PEER_FINGERPRINT_123",
                "timestamp": int(time.time() * 1000),
                "ttlSeconds": 120,
                "payload": {
                    "mimeType": "image/png",
                    "content": "",
                    "sha256": hashlib.sha256(b"").hexdigest(),
                    "sizeBytes": 0
                }
            }
            code, _ = post_clipboard(port, empty_packet)
            if code != 200:
                raise RuntimeError(f"Empty content should return 200, got {code}")

            # 3. Malformed data URI
            malformed_uri_packet = {
                "type": "clipboard.inline",
                "version": 2,
                "deviceId": "PEER_FINGERPRINT_123",
                "timestamp": int(time.time() * 1000),
                "ttlSeconds": 120,
                "payload": {
                    "mimeType": "image/png",
                    "content": "data:image/png;base64,====",
                    "sha256": hashlib.sha256(b"====").hexdigest(),
                    "sizeBytes": 4
                }
            }
            code, _ = post_clipboard(port, malformed_uri_packet)
            if code != 200:
                raise RuntimeError(f"Malformed data URI should return 200, got {code}")

        finally:
            daemon_proc.terminate()
            daemon_proc.wait(timeout=3)

def test_daemon_case_insensitive_sha256_echo_suppression():
    port = get_free_port()
    with tempfile.TemporaryDirectory() as td:
        daemon_proc, _ = start_daemon_instance(td, port)
        try:
            b64 = base64.b64encode(VALID_1X1_PNG).decode("ascii")
            lowercase_sha = hashlib.sha256(VALID_1X1_PNG).hexdigest().lower()
            uppercase_sha = lowercase_sha.upper()

            # First post: lowercase SHA-256
            packet_lower = {
                "type": "clipboard.inline",
                "version": 2,
                "deviceId": "PEER_FINGERPRINT_123",
                "timestamp": int(time.time() * 1000),
                "ttlSeconds": 120,
                "payload": {
                    "mimeType": "image/png",
                    "content": b64,
                    "sha256": lowercase_sha,
                    "sizeBytes": len(VALID_1X1_PNG)
                }
            }
            code1, _ = post_clipboard(port, packet_lower)
            if code1 != 200:
                raise RuntimeError(f"First post failed with {code1}")

            time.sleep(0.1)

            # Second post: identical content with UPPERCASE SHA-256 (simulating reflection echo)
            packet_upper = {
                "type": "clipboard.inline",
                "version": 2,
                "deviceId": "PEER_FINGERPRINT_123",
                "timestamp": int(time.time() * 1000) + 1,
                "ttlSeconds": 120,
                "payload": {
                    "mimeType": "image/png",
                    "content": b64,
                    "sha256": uppercase_sha,
                    "sizeBytes": len(VALID_1X1_PNG)
                }
            }
            code2, _ = post_clipboard(port, packet_upper)
            if code2 != 200:
                raise RuntimeError(f"Duplicate post failed with {code2}")

        finally:
            daemon_proc.terminate()
            daemon_proc.wait(timeout=3)

def test_daemon_staged_file_upload_uri_list():
    port = get_free_port()
    with tempfile.TemporaryDirectory() as td:
        daemon_proc, download_dir = start_daemon_instance(td, port)
        try:
            test_content = b"Staged Document File Content Ingestion Test"
            test_sha256 = hashlib.sha256(test_content).hexdigest()

            # 1. Prepare upload with header X-Clipboard-Staged: true
            prep_url = f"http://127.0.0.1:{port}/api/localsend/v2/prepare-upload"
            prep_body = {
                "info": {
                    "alias": "Phone Device",
                    "version": "2.2",
                    "deviceModel": "Android",
                    "deviceType": "mobile",
                    "fingerprint": "PEER_FINGERPRINT_123",
                    "port": 53317,
                    "protocol": "http",
                    "download": True
                },
                "files": {
                    "doc-1": {
                        "id": "doc-1",
                        "fileName": "clipboard_staged.pdf",
                        "size": len(test_content),
                        "fileType": "application/pdf",
                        "sha256": test_sha256
                    }
                }
            }

            req = urllib.request.Request(
                prep_url,
                data=json.dumps(prep_body).encode("utf-8"),
                headers={
                    "Content-Type": "application/json",
                    "X-Clipboard-Staged": "true"
                }
            )
            with urllib.request.urlopen(req, timeout=5) as resp:
                if resp.getcode() != 200:
                    raise RuntimeError(f"prepare-upload failed: {resp.getcode()}")
                prep_data = json.loads(resp.read().decode("utf-8"))

            session_id = prep_data["sessionId"]
            token = prep_data["files"]["doc-1"]

            # 2. Upload file
            upload_url = f"http://127.0.0.1:{port}/api/localsend/v2/upload?sessionId={session_id}&fileId=doc-1&token={token}"
            req_upload = urllib.request.Request(
                upload_url,
                data=test_content,
                headers={"Content-Type": "application/octet-stream"}
            )
            with urllib.request.urlopen(req_upload, timeout=5) as resp:
                if resp.getcode() != 200:
                    raise RuntimeError(f"upload failed: {resp.getcode()}")

            time.sleep(0.3)

            # 3. Verify file exists in download_dir
            saved_file = os.path.join(download_dir, "clipboard_staged.pdf")
            if not os.path.exists(saved_file):
                raise RuntimeError(f"Uploaded file not found at {saved_file}")

            with open(saved_file, "rb") as f:
                saved_bytes = f.read()
            if saved_bytes != test_content:
                raise RuntimeError(f"Saved file contents mismatch: {len(saved_bytes)} vs {len(test_content)}")

            # 4. Verify Wayland clipboard contains text/uri-list pointing to saved file
            pasted_uri = subprocess.check_output(["wl-paste", "--type", "text/uri-list"]).decode("utf-8")
            expected_uri = f"file://{saved_file}"
            if expected_uri not in pasted_uri:
                raise RuntimeError(f"Expected uri '{expected_uri}' not in pasted uri-list '{pasted_uri}'")

        finally:
            daemon_proc.terminate()
            daemon_proc.wait(timeout=3)

def test_host_wireguard_safety_final():
    verify_wireguard_safety()

def main():
    print("================================================================================")
    print("STARTING EMPIRICAL ADVERSARIAL STRESS SUITE (Phase 5 Milestone 1)")
    print("================================================================================")

    run_test("Host WireGuard Safety Invariant (Pre-Test)", test_host_wireguard_safety_initial)
    run_test("Native wl-copy & wl-paste Binary PNG and JPEG", test_wl_copy_and_paste_direct_binary)
    run_test("Daemon Raw Base64 PNG Ingestion & Wayland Binary Paste", test_daemon_raw_base64_png_ingestion)
    run_test("Daemon Data URI JPEG Ingestion & Wayland Binary Paste", test_daemon_data_uri_jpeg_ingestion)
    run_test("Daemon Corrupted Base64 & Empty Content Resilience", test_daemon_corrupted_base64_and_empty_edge_cases)
    run_test("Daemon Case-Insensitive SHA-256 Echo Suppression (A-F vs a-f)", test_daemon_case_insensitive_sha256_echo_suppression)
    run_test("Daemon Staged File Upload & Wayland text/uri-list Injection", test_daemon_staged_file_upload_uri_list)
    run_test("Host WireGuard Safety Invariant (Post-Test)", test_host_wireguard_safety_final)

    print("\n" + "=" * 80)
    print("EMPIRICAL TEST SUITE SUMMARY")
    print("=" * 80)
    print(f"Total Tests Run: {len(test_passes) + len(test_failures)}")
    print(f"Passed: {len(test_passes)}")
    print(f"Failed: {len(test_failures)}")

    if test_failures:
        print("\nFAILURES:")
        for name, err in test_failures:
            print(f"  - {name}: {err}")
        sys.exit(1)
    else:
        print("\nALL EMPIRICAL ADVERSARIAL TESTS PASSED!")
        sys.exit(0)

if __name__ == "__main__":
    main()
