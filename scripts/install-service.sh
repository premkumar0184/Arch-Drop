#!/usr/bin/env bash
set -euo pipefail

MODE="user"
AUTO_START=true
CUSTOM_BIN=""

usage() {
    local exit_code="${1:-0}"
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Install and configure the archdropd systemd user service.

Options:
    --user         Install to ~/.config/systemd/user (default, rootless)
    --system       Install to /usr/lib/systemd/user (requires root)
    --bin <PATH>   Path to archdropd binary (defaults to target/release or target/debug)
    --no-start     Enable unit without starting immediately
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
        --bin)
            if [[ $# -lt 2 || -z "$2" ]]; then
                echo "Error: --bin requires a path argument" >&2
                usage 1
            fi
            CUSTOM_BIN="$2"
            shift 2
            ;;
        --no-start)
            AUTO_START=false
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

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
UNIT_SRC="${PROJECT_ROOT}/systemd/user/archdropd.service"

if [[ ! -f "${UNIT_SRC}" ]]; then
    echo "Error: Service unit template not found at ${UNIT_SRC}" >&2
    exit 1
fi

# Locate or build binary
ARCHDROPD_BIN=""
if [[ -n "${CUSTOM_BIN}" ]]; then
    if [[ ! -x "${CUSTOM_BIN}" ]]; then
        echo "Error: Specified binary ${CUSTOM_BIN} is not executable" >&2
        exit 1
    fi
    ARCHDROPD_BIN="${CUSTOM_BIN}"
elif [[ -x "${PROJECT_ROOT}/target/release/archdropd" ]]; then
    ARCHDROPD_BIN="${PROJECT_ROOT}/target/release/archdropd"
elif [[ -x "${PROJECT_ROOT}/target/debug/archdropd" ]]; then
    ARCHDROPD_BIN="${PROJECT_ROOT}/target/debug/archdropd"
else
    echo "Binary not found in target/. Building release binary..."
    (cd "${PROJECT_ROOT}" && cargo build --release --bin archdropd)
    ARCHDROPD_BIN="${PROJECT_ROOT}/target/release/archdropd"
fi

if [[ "${MODE}" == "user" ]]; then
    if systemctl --user is-active --quiet archdropd.service 2>/dev/null; then
        echo "==> Stopping active archdropd service before upgrade..."
        systemctl --user stop archdropd.service 2>/dev/null || true
    fi

    REAL_HOME=$(getent passwd "$(id -u)" 2>/dev/null | cut -d: -f6 || true)
    TARGET_HOME="${REAL_HOME:-$HOME}"

    BIN_DEST_DIR="${TARGET_HOME}/.cargo/bin"
    UNIT_DEST_DIR="${TARGET_HOME}/.config/systemd/user"
    mkdir -p "${BIN_DEST_DIR}" "${UNIT_DEST_DIR}"

    echo "==> Installing archdropd binary to ${BIN_DEST_DIR}/archdropd..."
    install -Dm755 "${ARCHDROPD_BIN}" "${BIN_DEST_DIR}/archdropd"

    # Also mirror into $HOME if running inside an IDE sandbox/subshell
    if [[ "${TARGET_HOME}" != "${HOME}" && -d "${HOME}" ]]; then
        mkdir -p "${HOME}/.cargo/bin" "${HOME}/.config/systemd/user"
        install -Dm755 "${ARCHDROPD_BIN}" "${HOME}/.cargo/bin/archdropd" 2>/dev/null || true
    fi

    echo "==> Installing systemd user unit to ${UNIT_DEST_DIR}/archdropd.service..."
    sed "s|ExecStart=.*|ExecStart=%h/.cargo/bin/archdropd daemon|" "${UNIT_SRC}" > "${UNIT_DEST_DIR}/archdropd.service"
    chmod 644 "${UNIT_DEST_DIR}/archdropd.service"

    if [[ "${TARGET_HOME}" != "${HOME}" && -d "${HOME}/.config/systemd/user" ]]; then
        cp "${UNIT_DEST_DIR}/archdropd.service" "${HOME}/.config/systemd/user/archdropd.service" 2>/dev/null || true
    fi

    echo "==> Reloading systemd user daemon..."
    systemctl --user daemon-reload

    if [[ "${AUTO_START}" == "true" ]]; then
        echo "==> Enabling and starting archdropd.service..."
        systemctl --user enable --now archdropd.service
        sleep 0.5
        if systemctl --user is-active --quiet archdropd.service; then
            echo "==> Service successfully started and active!"
        else
            echo "Warning: Service installed but not currently active. Check: systemctl --user status archdropd.service"
        fi
    else
        echo "==> Enabling archdropd.service..."
        systemctl --user enable archdropd.service
    fi

elif [[ "${MODE}" == "system" ]]; then
    if [[ $EUID -ne 0 ]]; then
        echo "Error: System mode installation requires root privileges (run with sudo)" >&2
        exit 1
    fi

    BIN_DEST_DIR="/usr/bin"
    UNIT_DEST_DIR="/usr/lib/systemd/user"
    mkdir -p "${BIN_DEST_DIR}" "${UNIT_DEST_DIR}"

    echo "==> Installing archdropd binary to ${BIN_DEST_DIR}/archdropd..."
    install -Dm755 "${ARCHDROPD_BIN}" "${BIN_DEST_DIR}/archdropd"

    echo "==> Installing systemd user unit to ${UNIT_DEST_DIR}/archdropd.service..."
    sed "s|ExecStart=.*|ExecStart=/usr/bin/archdropd daemon|" "${UNIT_SRC}" > "${UNIT_DEST_DIR}/archdropd.service"
    chmod 644 "${UNIT_DEST_DIR}/archdropd.service"

    echo "==> Reloading systemd global unit definitions..."
    systemctl --global daemon-reload 2>/dev/null || true
    echo "==> System-wide unit installed to ${UNIT_DEST_DIR}/archdropd.service"
fi

echo "Arch Drop user service installation complete."
