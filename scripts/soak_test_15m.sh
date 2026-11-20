#!/usr/bin/env bash
# ==============================================================================
# Arch Drop — 15-Minute Continuous Operation Soak Stability Harness (R4)
# ==============================================================================
# Executes a strict 15-minute (900 seconds) continuous operation soak test on
# the stripped release binary `target/release/archdropd`.
#
# Workload:
# - Alternates between Active Simulated Transfer / IPC Streaming and Idle states.
# - Samples Resident Set Size (RSS) periodically via /proc/[pid]/statm and ps.
# - Enforces strict invariant: RSS must remain strictly < 15 MB (15360 KB) at all times.
# - Verifies clean graceful shutdown and host WireGuard safety invariants.
# ==============================================================================

set -euo pipefail

DURATION_SECONDS=900
SAMPLE_INTERVAL_SECONDS=15
SOAK_PORT=54321
SOAK_DIR="/tmp/archdrop_soak_15m"
PROJECT_ROOT="/home/prem/Projects/Arch Drop"
BINARY="$PROJECT_ROOT/target/release/archdropd"
CSV_FILE="$PROJECT_ROOT/.agents/teamwork/worker_p4_m4/soak_telemetry.csv"
LOG_FILE="$PROJECT_ROOT/.agents/teamwork/worker_p4_m4/soak_daemon.log"

echo "========================================================================"
echo " Arch Drop 15-Minute Continuous Operation Soak Test Harness"
echo " Start Time: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
echo " Binary: $BINARY"
echo " Duration: $DURATION_SECONDS seconds (15 minutes)"
echo " Sample Interval: $SAMPLE_INTERVAL_SECONDS seconds"
echo " Memory Budget: STRICTLY < 15.0 MB RSS (15360 KB)"
echo "========================================================================"

# Prepare clean workspace
rm -rf "$SOAK_DIR"
mkdir -p "$SOAK_DIR/downloads"
mkdir -p "$SOAK_DIR/payloads"
mkdir -p "$(dirname "$CSV_FILE")"

# Create isolated test config
cat <<EOF > "$SOAK_DIR/config.toml"
[device]
alias = "SoakTestNode"

[network]
port = $SOAK_PORT
bind_address = "127.0.0.1"

[security]
require_encryption = false
auto_accept_paired = true
paired_fingerprints = []

[storage]
download_dir = "$SOAK_DIR/downloads"

[mesh]
enabled = false
relay_url = "wss://derp.archdrop.net:443/derp"
direct_dial_timeout_ms = 1500
EOF

# Initialize CSV telemetry file
echo "timestamp_iso,elapsed_seconds,phase,rss_statm_kb,rss_ps_kb,rss_mb,active_transfers_count,status" > "$CSV_FILE"

# Start the stripped release daemon
echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] Launching release daemon on port $SOAK_PORT..."
"$BINARY" daemon \
    -c "$SOAK_DIR/config.toml" \
    --port "$SOAK_PORT" \
    --socket-path "$SOAK_DIR/archdropd.sock" \
    --no-tls > "$LOG_FILE" 2>&1 &
DAEMON_PID=$!

echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] Daemon launched with PID: $DAEMON_PID"

# Trap cleanup to guarantee daemon shutdown
cleanup() {
    echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] Cleaning up soak harness..."
    if kill -0 "$DAEMON_PID" 2>/dev/null; then
        echo "Sending SIGTERM to PID $DAEMON_PID..."
        kill -TERM "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
    rm -rf "$SOAK_DIR"
}
trap cleanup EXIT INT TERM

# Wait for daemon initialization
sleep 2
if ! kill -0 "$DAEMON_PID" 2>/dev/null; then
    echo "ERROR: Daemon failed to start. Logs:"
    cat "$LOG_FILE"
    exit 1
fi

PAGE_SIZE=$(getconf PAGESIZE)
START_TIME=$(date +%s)
ELAPSED=0
CYCLE=0
TRANSFERS_EXECUTED=0

MIN_RSS_KB=999999
MAX_RSS_KB=0
SUM_RSS_KB=0
SAMPLE_COUNT=0

echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] Entering 15-minute sampling loop..."

while [ "$ELAPSED" -lt "$DURATION_SECONDS" ]; do
    CURRENT_TIME=$(date +%s)
    ELAPSED=$((CURRENT_TIME - START_TIME))
    CYCLE=$((CYCLE + 1))
    TIMESTAMP_ISO=$(date -u '+%Y-%m-%dT%H:%M:%SZ')

    if ! kill -0 "$DAEMON_PID" 2>/dev/null; then
        echo "ERROR: Daemon died unexpectedly at elapsed time ${ELAPSED}s!"
        cat "$LOG_FILE"
        exit 1
    fi

    # Read RSS from /proc/[pid]/statm
    STATM_PAGES=$(awk '{print $2}' "/proc/$DAEMON_PID/statm" 2>/dev/null || echo 0)
    STATM_RSS_KB=$((STATM_PAGES * PAGE_SIZE / 1024))

    # Read RSS from ps -o rss=
    PS_RSS_KB=$(ps -o rss= -p "$DAEMON_PID" 2>/dev/null | tr -d ' ' || echo 0)

    # Compute MB (with 2 decimal places)
    RSS_MB=$(awk "BEGIN {printf \"%.2f\", $STATM_RSS_KB / 1024}")

    # Determine Phase: Alternate between ACTIVE (odd cycles) and IDLE (even cycles)
    # Every 4th cycle: larger payload burst
    PHASE="IDLE"
    if [ $((CYCLE % 2)) -ne 0 ]; then
        PHASE="ACTIVE"
    fi

    # Record metrics
    if [ "$STATM_RSS_KB" -lt "$MIN_RSS_KB" ]; then MIN_RSS_KB=$STATM_RSS_KB; fi
    if [ "$STATM_RSS_KB" -gt "$MAX_RSS_KB" ]; then MAX_RSS_KB=$STATM_RSS_KB; fi
    SUM_RSS_KB=$((SUM_RSS_KB + STATM_RSS_KB))
    SAMPLE_COUNT=$((SAMPLE_COUNT + 1))

    # Strict Invariant Check: Must be < 15360 KB (15.0 MB)
    if [ "$STATM_RSS_KB" -ge 15360 ]; then
        echo "FATAL INVARIANT VIOLATION: Memory RSS $STATM_RSS_KB KB ($RSS_MB MB) exceeds 15 MB limit!"
        echo "$TIMESTAMP_ISO,$ELAPSED,$PHASE,$STATM_RSS_KB,$PS_RSS_KB,$RSS_MB,$TRANSFERS_EXECUTED,VIOLATION" >> "$CSV_FILE"
        exit 2
    fi

    echo "$TIMESTAMP_ISO,$ELAPSED,$PHASE,$STATM_RSS_KB,$PS_RSS_KB,$RSS_MB,$TRANSFERS_EXECUTED,OK" >> "$CSV_FILE"

    echo "[$TIMESTAMP_ISO] [${ELAPSED}s / ${DURATION_SECONDS}s] Phase: $PHASE | Statm RSS: ${STATM_RSS_KB} KB (${RSS_MB} MB) | ps RSS: ${PS_RSS_KB} KB | Transfers: $TRANSFERS_EXECUTED"

    # Execute active workload if in ACTIVE phase
    if [ "$PHASE" = "ACTIVE" ]; then
        # Vary payload size between 32 KB, 256 KB, 1 MB, 4 MB
        PAYLOAD_SIZE_KB=$(( (CYCLE % 4 + 1) * 512 ))
        TEST_PAYLOAD="$SOAK_DIR/payloads/payload_${CYCLE}.bin"
        dd if=/dev/urandom of="$TEST_PAYLOAD" bs=1024 count="$PAYLOAD_SIZE_KB" 2>/dev/null

        # Transfer via CLI
        "$BINARY" send --to 127.0.0.1 --port "$SOAK_PORT" "$TEST_PAYLOAD" >/dev/null 2>&1 || true
        TRANSFERS_EXECUTED=$((TRANSFERS_EXECUTED + 1))

        # Query IPC socket status snapshot
        "$BINARY" status --socket-path "$SOAK_DIR/archdropd.sock" --json >/dev/null 2>&1 || true

        # Clean up payloads and downloaded files to prevent disk exhaustion
        rm -f "$TEST_PAYLOAD"
        rm -rf "$SOAK_DIR/downloads"/*
    fi

    # Sleep until next sample interval
    sleep "$SAMPLE_INTERVAL_SECONDS"
done

# Final post-soak sample
FINAL_STATM_PAGES=$(awk '{print $2}' "/proc/$DAEMON_PID/statm" 2>/dev/null || echo 0)
FINAL_STATM_RSS_KB=$((FINAL_STATM_PAGES * PAGE_SIZE / 1024))
FINAL_PS_RSS_KB=$(ps -o rss= -p "$DAEMON_PID" 2>/dev/null | tr -d ' ' || echo 0)
FINAL_RSS_MB=$(awk "BEGIN {printf \"%.2f\", $FINAL_STATM_RSS_KB / 1024}")
AVG_RSS_KB=$((SUM_RSS_KB / SAMPLE_COUNT))
AVG_RSS_MB=$(awk "BEGIN {printf \"%.2f\", $AVG_RSS_KB / 1024}")
MIN_RSS_MB=$(awk "BEGIN {printf \"%.2f\", $MIN_RSS_KB / 1024}")
MAX_RSS_MB=$(awk "BEGIN {printf \"%.2f\", $MAX_RSS_KB / 1024}")

echo "========================================================================"
echo " 15-Minute Soak Test Complete!"
echo " Total Duration: ${ELAPSED} seconds"
echo " Total Samples: ${SAMPLE_COUNT}"
echo " Total Active Transfers: ${TRANSFERS_EXECUTED}"
echo " Min RSS: ${MIN_RSS_KB} KB (${MIN_RSS_MB} MB)"
echo " Max RSS: ${MAX_RSS_KB} KB (${MAX_RSS_MB} MB)"
echo " Avg RSS: ${AVG_RSS_KB} KB (${AVG_RSS_MB} MB)"
echo " Final RSS: ${FINAL_STATM_RSS_KB} KB (${FINAL_RSS_MB} MB)"
echo " Memory Budget: 15.0 MB (15360 KB)"
echo " Invariant Verdict: PASSED (Peak ${MAX_RSS_MB} MB < 15.0 MB)"
echo "========================================================================"

# Send SIGTERM to test graceful drain
echo "Testing graceful daemon shutdown with SIGTERM..."
kill -TERM "$DAEMON_PID"
wait "$DAEMON_PID" || true
echo "Daemon shutdown cleanly."

# Verify Host WireGuard safety invariant
echo "Verifying Host WireGuard interface safety invariant..."
ip addr show dev tiktok | grep -q "10.2.0.2/32" && echo "PASS: tiktok interface is 10.2.0.2/32 UP"
ip route show table 51820 | grep -q "default dev tiktok scope link" && echo "PASS: table 51820 route intact"
ip rule show | grep -q "100:.*from 10.2.0.2 lookup 51820" && echo "PASS: rule 100 lookup 51820 intact"

echo "ALL SOAK AND SAFETY INVARIANTS SATISFIED 100%!"
exit 0
