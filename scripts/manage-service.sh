#!/usr/bin/env bash
set -euo pipefail

usage() {
    local exit_code="${1:-0}"
    cat <<EOF
Usage: $(basename "$0") <COMMAND> [OPTIONS]

Management CLI for the archdropd systemd user service.

Commands:
    status       Display systemd service and IPC socket status
    start        Start the archdropd user service
    stop         Stop the archdropd user service
    restart      Restart the archdropd user service
    logs         Display journalctl service logs (use -f/--follow to stream)
    verify       Validate unit file syntax with systemd-analyze
    security     Run systemd security analysis and hardening rating
    -h, --help   Show this help message
EOF
    exit "${exit_code}"
}

if [[ $# -lt 1 ]]; then
    usage 1
fi

CMD="$1"
shift || true

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
UNIT_FILE="${PROJECT_ROOT}/systemd/user/archdropd.service"
USER_UNIT="${HOME}/.config/systemd/user/archdropd.service"

RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
SOCKET_PATH="${RUNTIME_DIR}/archdrop/archdropd.sock"
FALLBACK_SOCKET="/tmp/archdrop-$(id -u)/archdropd.sock"

case "${CMD}" in
    status)
        echo "=== Systemd Service Status ==="
        systemctl --user status archdropd.service --no-pager || true
        echo ""
        echo "=== IPC Socket Status ==="
        if [[ -S "${SOCKET_PATH}" ]]; then
            echo "Primary Socket : ${SOCKET_PATH} (PRESENT)"
            stat -c "Permissions    : %a (Owner: %U)" "${SOCKET_PATH}"
        elif [[ -S "${FALLBACK_SOCKET}" ]]; then
            echo "Fallback Socket: ${FALLBACK_SOCKET} (PRESENT)"
            stat -c "Permissions    : %a (Owner: %U)" "${FALLBACK_SOCKET}"
        else
            echo "IPC Socket     : NOT FOUND (Daemon not running or socket in non-standard path)"
        fi
        ;;

    start)
        echo "==> Starting archdropd.service..."
        systemctl --user start archdropd.service
        systemctl --user is-active --quiet archdropd.service && echo "Service is ACTIVE."
        ;;

    stop)
        echo "==> Stopping archdropd.service..."
        systemctl --user stop archdropd.service
        echo "Service STOPPED."
        ;;

    restart)
        echo "==> Restarting archdropd.service..."
        systemctl --user restart archdropd.service
        systemctl --user is-active --quiet archdropd.service && echo "Service restarted and ACTIVE."
        ;;

    logs)
        LINES=50
        FOLLOW=false
        while [[ $# -gt 0 ]]; do
            case "$1" in
                -f|--follow)
                    FOLLOW=true
                    shift
                    ;;
                --no-follow)
                    FOLLOW=false
                    shift
                    ;;
                -n|--lines)
                    if [[ $# -lt 2 || -z "$2" ]]; then
                        echo "Error: $1 requires an integer argument" >&2
                        exit 1
                    fi
                    LINES="$2"
                    shift 2
                    ;;
                -h|--help)
                    cat <<EOF
Usage: $(basename "$0") logs [LINES] [OPTIONS]

Display journalctl logs for archdropd.service.

Options:
    -n, --lines <N>  Number of journal entries to display (default: 50)
    -f, --follow     Follow live log stream
    --no-follow      Display recent logs and exit immediately (default)
    -h, --help       Show this help message
EOF
                    exit 0
                    ;;
                *)
                    if [[ "$1" =~ ^[0-9]+$ ]]; then
                        LINES="$1"
                        shift
                    else
                        echo "Unknown option for logs: $1" >&2
                        exit 1
                    fi
                    ;;
            esac
        done

        if [[ "${FOLLOW}" == "true" ]]; then
            journalctl --user -u archdropd.service -n "${LINES}" -f --output=cat
        else
            journalctl --user -u archdropd.service -n "${LINES}" --no-pager --output=cat
        fi
        ;;

    verify|check)
        TARGET_UNIT="${UNIT_FILE}"
        if [[ -f "${USER_UNIT}" ]]; then
            TARGET_UNIT="${USER_UNIT}"
        fi
        echo "==> Verifying unit file: ${TARGET_UNIT}..."
        systemd-analyze --user verify "${TARGET_UNIT}"
        echo "Verification passed cleanly with 0 errors!"
        ;;

    security)
        echo "==> Running systemd security audit on archdropd.service..."
        systemd-analyze --user security archdropd.service || true
        ;;

    -h|--help)
        usage 0
        ;;

    *)
        echo "Unknown command: ${CMD}" >&2
        usage 1
        ;;
esac
