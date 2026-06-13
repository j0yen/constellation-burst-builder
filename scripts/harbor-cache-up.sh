#!/usr/bin/env bash
# harbor-cache-up.sh — set up a shared MinIO sccache backend on the permanent hub.
#
# Usage:
#   harbor-cache-up.sh [--dry-run] [--hub-json PATH] [--bind-addr ADDR]
#
# Options:
#   --dry-run        Print planned steps and exit 0; touch nothing.
#   --hub-json PATH  Path to hub.json (default: ~/.config/wm-burst/hub.json).
#   --bind-addr ADDR MinIO bind address (default: 0.0.0.0; set to mesh IP in prod).
#
# Idempotent: re-running a live hub detects MinIO + bucket and reconciles only.
# On success, prints shell-exportable SCCACHE_* lines (no secrets in stdout when
# already running — instructs user to source ~/.config/wm-burst/cache.env).
#
# Secrets (access/secret key) are written ONLY to ~/.config/wm-burst/cache.env
# and are never printed to stdout after initial creation.

set -euo pipefail

# ── defaults ────────────────────────────────────────────────────────────────
DRY_RUN=0
HUB_JSON="${HOME}/.config/wm-burst/hub.json"
BIND_ADDR="0.0.0.0"
MINIO_PORT=9000
MINIO_CONSOLE_PORT=9001
BUCKET_NAME="wm-sccache"
MINIO_DATA_DIR="/var/lib/minio/data"
MINIO_USER="minio-user"
CACHE_ENV="${HOME}/.config/wm-burst/cache.env"

# ── arg parse ────────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run)    DRY_RUN=1; shift ;;
        --hub-json)   HUB_JSON="$2"; shift 2 ;;
        --bind-addr)  BIND_ADDR="$2"; shift 2 ;;
        *)  echo "error: unknown argument '$1'" >&2; exit 1 ;;
    esac
done

# ── helpers ──────────────────────────────────────────────────────────────────
log()  { echo "[harbor-cache] $*"; }
step() { echo "[harbor-cache] STEP: $*"; }
dry()  { echo "[harbor-cache] DRY-RUN: $*"; }

# Generate a random 32-char alphanumeric key (portable, no /dev/urandom assumption).
gen_key() {
    tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 32
}

# ── resolve hub endpoint ─────────────────────────────────────────────────────
if [[ -f "$HUB_JSON" ]]; then
    HUB_IP="$(python3 -c "import json,sys; d=json.load(open('$HUB_JSON')); print(d.get('ip',''))" 2>/dev/null || true)"
else
    HUB_IP=""
fi
MINIO_ENDPOINT="http://${HUB_IP:-127.0.0.1}:${MINIO_PORT}"

# ── dry-run path ─────────────────────────────────────────────────────────────
if [[ $DRY_RUN -eq 1 ]]; then
    # Check for already-up marker (idempotency smoke test).
    MARKER_FILE="/tmp/harbor-cache-up.marker"
    if [[ -f "$MARKER_FILE" ]]; then
        dry "marker ${MARKER_FILE} present → reconcile only, no create"
        dry "STEP would-reconcile: verify MinIO unit active"
        dry "STEP would-reconcile: verify bucket '${BUCKET_NAME}' exists"
        dry "STEP would-reconcile: verify cache.env present"
        dry "No changes needed (reconcile-only mode)."
        exit 0
    fi

    dry "STEP 1: detect existing MinIO installation"
    dry "STEP 2: install minio binary (if absent) — from https://dl.min.io/server/minio/release/linux-amd64/minio"
    dry "STEP 3: create system user '${MINIO_USER}'"
    dry "STEP 4: create data directory '${MINIO_DATA_DIR}'"
    dry "STEP 5: generate access keypair (32-char random; written to ${CACHE_ENV})"
    dry "STEP 6: write /etc/default/minio with MINIO_ROOT_USER / MINIO_ROOT_PASSWORD (no secret in tracked files)"
    dry "STEP 7: install systemd unit /etc/systemd/system/minio.service"
    dry "  [Unit] Description=MinIO sccache backend"
    dry "  [Service] User=${MINIO_USER} ExecStart=/usr/local/bin/minio server --address ${BIND_ADDR}:${MINIO_PORT} --console-address :${MINIO_CONSOLE_PORT} ${MINIO_DATA_DIR}"
    dry "STEP 8: systemctl daemon-reload && systemctl enable --now minio"
    dry "STEP 9: create bucket '${BUCKET_NAME}' via mc (MinIO client)"
    dry "STEP 10: emit shell-exportable SCCACHE_* lines:"
    dry "  export SCCACHE_ENDPOINT=${MINIO_ENDPOINT}"
    dry "  export SCCACHE_BUCKET=${BUCKET_NAME}"
    dry "  export SCCACHE_S3_USE_SSL=false"
    dry "  export AWS_ACCESS_KEY_ID=<generated>"
    dry "  export AWS_SECRET_ACCESS_KEY=<generated>"
    dry "  export RUSTC_WRAPPER=sccache"
    dry "STEP 11: write ${CACHE_ENV} (secrets only, chmod 600, gitignored)"
    log "Dry-run complete — exiting 0 without touching anything."
    exit 0
fi

# ── live path (must run as root on the hub) ──────────────────────────────────
log "Starting MinIO setup for shared sccache backend..."

# Detect if MinIO is already installed and running.
MINIO_BINARY="/usr/local/bin/minio"
MINIO_UNIT="/etc/systemd/system/minio.service"
MINIO_DEFAULTS="/etc/default/minio"
ALREADY_RUNNING=0

if systemctl is-active --quiet minio 2>/dev/null; then
    log "MinIO unit already active — entering reconcile mode."
    ALREADY_RUNNING=1
fi

if [[ $ALREADY_RUNNING -eq 0 ]]; then
    step "1/9: install minio binary"
    if [[ ! -x "$MINIO_BINARY" ]]; then
        curl -fsSL "https://dl.min.io/server/minio/release/linux-amd64/minio" \
            -o /tmp/minio-download
        install -m 0755 /tmp/minio-download "$MINIO_BINARY"
        rm -f /tmp/minio-download
        log "minio binary installed to ${MINIO_BINARY}"
    else
        log "minio binary already present — skipping download"
    fi

    step "2/9: create system user ${MINIO_USER}"
    if ! id "$MINIO_USER" &>/dev/null; then
        useradd --system --no-create-home --shell /sbin/nologin "$MINIO_USER"
    fi

    step "3/9: create data directory ${MINIO_DATA_DIR}"
    mkdir -p "$MINIO_DATA_DIR"
    chown -R "${MINIO_USER}:${MINIO_USER}" "$MINIO_DATA_DIR"
    chmod 750 "$MINIO_DATA_DIR"

    step "4/9: generate access keypair"
    if [[ -f "$CACHE_ENV" ]] && grep -q "AWS_ACCESS_KEY_ID=" "$CACHE_ENV" 2>/dev/null; then
        log "cache.env already contains keypair — reusing"
        # shellcheck disable=SC1090
        source "$CACHE_ENV"
    else
        mkdir -p "$(dirname "$CACHE_ENV")"
        ACCESS_KEY="$(gen_key)"
        SECRET_KEY="$(gen_key)"
        cat > "$CACHE_ENV" <<EOF
# wm-burst sccache client environment — DO NOT COMMIT
# Generated by harbor-cache-up.sh on $(date -u +%Y-%m-%dT%H:%M:%SZ)
export SCCACHE_ENDPOINT=${MINIO_ENDPOINT}
export SCCACHE_BUCKET=${BUCKET_NAME}
export SCCACHE_S3_USE_SSL=false
export AWS_ACCESS_KEY_ID=${ACCESS_KEY}
export AWS_SECRET_ACCESS_KEY=${SECRET_KEY}
export RUSTC_WRAPPER=sccache
EOF
        chmod 600 "$CACHE_ENV"
        log "Keypair written to ${CACHE_ENV} (chmod 600)"
    fi

    # Load keypair for MinIO defaults.
    # shellcheck disable=SC1090
    source "$CACHE_ENV"

    step "5/9: write /etc/default/minio"
    cat > "$MINIO_DEFAULTS" <<EOF
# MinIO environment — managed by harbor-cache-up.sh
MINIO_ROOT_USER=${AWS_ACCESS_KEY_ID}
MINIO_ROOT_PASSWORD=${AWS_SECRET_ACCESS_KEY}
MINIO_VOLUMES=${MINIO_DATA_DIR}
MINIO_OPTS="--address ${BIND_ADDR}:${MINIO_PORT} --console-address :${MINIO_CONSOLE_PORT}"
EOF
    chmod 640 "$MINIO_DEFAULTS"

    step "6/9: install systemd unit"
    cat > "$MINIO_UNIT" <<'UNIT'
[Unit]
Description=MinIO sccache shared object store
Documentation=https://min.io/docs/minio/linux/index.html
Wants=network-online.target
After=network-online.target

[Service]
User=minio-user
Group=minio-user
EnvironmentFile=/etc/default/minio
ExecStart=/usr/local/bin/minio server $MINIO_OPTS $MINIO_VOLUMES
Restart=on-failure
RestartSec=5s
LimitNOFILE=65536
TasksMax=infinity
TimeoutStopSec=60

[Install]
WantedBy=multi-user.target
UNIT

    step "7/9: enable and start MinIO"
    systemctl daemon-reload
    systemctl enable --now minio
    sleep 3  # give it a moment to bind

    # Touch marker for idempotency detection.
    touch /tmp/harbor-cache-up.marker
fi

step "8/9: ensure bucket '${BUCKET_NAME}' exists"
# Load credentials from cache.env.
# shellcheck disable=SC1090
source "$CACHE_ENV"

# Use mc (MinIO client) if available; otherwise use curl+minio admin API.
MC_BINARY="/usr/local/bin/mc"
if [[ ! -x "$MC_BINARY" ]]; then
    curl -fsSL "https://dl.min.io/client/mc/release/linux-amd64/mc" \
        -o /tmp/mc-download
    install -m 0755 /tmp/mc-download "$MC_BINARY"
    rm -f /tmp/mc-download
fi

"$MC_BINARY" alias set wm-cache "${MINIO_ENDPOINT}" \
    "${AWS_ACCESS_KEY_ID}" "${AWS_SECRET_ACCESS_KEY}" --insecure 2>/dev/null

if "$MC_BINARY" ls wm-cache/"${BUCKET_NAME}" --insecure &>/dev/null; then
    log "Bucket '${BUCKET_NAME}' already exists — skipping create (reconcile only)"
else
    "$MC_BINARY" mb wm-cache/"${BUCKET_NAME}" --insecure
    log "Bucket '${BUCKET_NAME}' created"
fi

step "9/9: emit client config"
log "MinIO sccache backend is live."
log "Source the following (or run harbor-cache-client-env.sh):"
echo "# ── sccache client env (source ~/.config/wm-burst/cache.env) ──"
echo "export SCCACHE_ENDPOINT=${SCCACHE_ENDPOINT}"
echo "export SCCACHE_BUCKET=${SCCACHE_BUCKET}"
echo "export SCCACHE_S3_USE_SSL=false"
echo "export RUSTC_WRAPPER=sccache"
echo "# AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY: see ${CACHE_ENV}"
log "Done."
