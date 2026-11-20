#!/usr/bin/env bash
set -euo pipefail

MODE="user"
PURGE=false

usage() {
    local exit_code="${1:-0}"
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Stop, disable, and uninstall the archdropd systemd service.

Options:
    --user         Uninstall user service from ~/.config/systemd/user (default)
    --system       Uninstall system-wide service from /usr/lib/systemd/user (requires root)
    --purge        Also remove installed binary and socket/runtime files
    -h, --help     Show this help message
EOF
    exit "${exit_code}"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --user)
            MODE="user"
            shift
            ;;
        --system)
            MODE="system"
            shift
            ;;
        --purge)
            PURGE=true
            shift
            ;;
        -h|--help)
            usage 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage 1
            ;;
    esac
done

if [[ "${MODE}" == "user" ]]; then
    echo "==> Stopping archdropd.service..."
    systemctl --user stop archdropd.service 2>/dev/null || true

    echo "==> Disabling archdropd.service..."
    systemctl --user disable archdropd.service 2>/dev/null || true

    REAL_HOME=$(getent passwd "$(id -u)" 2>/dev/null | cut -d: -f6 || true)
    TARGET_HOME="${REAL_HOME:-$HOME}"

    for H in "${TARGET_HOME}" "${HOME}"; do
        USER_UNIT="${H}/.config/systemd/user/archdropd.service"
        if [[ -f "${USER_UNIT}" ]]; then
            echo "==> Removing unit file ${USER_UNIT}..."
            rm -f "${USER_UNIT}"
        fi
    done

    echo "==> Reloading systemd user daemon..."
    systemctl --user daemon-reload
    systemctl --user reset-failed 2>/dev/null || true

    # Clean up socket file if left behind
    RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
    SOCKET_PATH="${RUNTIME_DIR}/archdrop/archdropd.sock"
    FALLBACK_SOCKET="/tmp/archdrop-$(id -u)/archdropd.sock"

    if [[ -e "${SOCKET_PATH}" ]]; then
        echo "==> Cleaning up runtime socket ${SOCKET_PATH}..."
        rm -f "${SOCKET_PATH}"
    fi
    if [[ -e "${FALLBACK_SOCKET}" ]]; then
        echo "==> Cleaning up fallback socket ${FALLBACK_SOCKET}..."
        rm -f "${FALLBACK_SOCKET}"
    fi

    if [[ "${PURGE}" == "true" ]]; then
        for H in "${TARGET_HOME}" "${HOME}"; do
            USER_BIN="${H}/.cargo/bin/archdropd"
            if [[ -f "${USER_BIN}" ]]; then
                echo "==> Purging binary ${USER_BIN}..."
                rm -f "${USER_BIN}"
            fi
        done
        echo "==> Purge complete."
    fi

elif [[ "${MODE}" == "system" ]]; then
    if [[ $EUID -ne 0 ]]; then
        echo "Error: System mode uninstallation requires root privileges (run with sudo)" >&2
        exit 1
    fi

    echo "==> Removing system unit /usr/lib/systemd/user/archdropd.service..."
    rm -f "/usr/lib/systemd/user/archdropd.service"
    systemctl --global daemon-reload 2>/dev/null || true

    if [[ "${PURGE}" == "true" ]]; then
        echo "==> Purging /usr/bin/archdropd..."
        rm -f "/usr/bin/archdropd"
    fi
fi

echo "Arch Drop service uninstallation complete."
