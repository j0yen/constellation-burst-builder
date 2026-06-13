#!/usr/bin/env bash
# harbor-bridge-up.sh — install NATS server + harbor-heartbeat on the hub
# Usage: harbor-bridge-up.sh [--dry-run] [--hub-user USER] [--hub-ip IP]
#
# Reads hub config from hub.json (same dir as this script or CWD).
# Token is written ONLY to ~/.config/wm-burst/bridge.env (gitignored, never tracked).
#
# Scope: this is the LANDING PAD for constellation-bus (the agorabus↔NATS bridge).
# It does NOT build the full bridge — that's the constellation-bus PRD.
# See REMOTE-SETUP.md §harbor-bridge for the boundary.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
# Allow env override for testing
HUB_JSON="${HUB_JSON:-${REPO_ROOT}/hub.json}"
SECRET_STORE="${SECRET_STORE:-${HOME}/.config/wm-burst/bridge.env}"

# ---- subjects (must match constellation day-2 contracts) --------------------
SUBJ_UP="wm.fleet.hub.up"
SUBJ_DOWN="wm.fleet.hub.down"
SUBJ_HEARTBEAT="wm.fleet.hub.heartbeat"

DRY_RUN=false
HUB_USER=""
HUB_IP=""

usage() {
  echo "Usage: $0 [--dry-run] [--hub-user USER] [--hub-ip IP]"
  echo "  --dry-run     Print planned actions; do NOT touch the hub"
  echo "  --hub-user    SSH user on the hub (default: root)"
  echo "  --hub-ip      Hub IP (overrides hub.json)"
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run)    DRY_RUN=true ;;
    --hub-user)   HUB_USER="$2"; shift ;;
    --hub-ip)     HUB_IP="$2"; shift ;;
    -h|--help)    usage ;;
    *) echo "Unknown arg: $1" >&2; exit 1 ;;
  esac
  shift
done

# ---- load hub.json -----------------------------------------------------------
load_hub_json() {
  if [[ ! -f "$HUB_JSON" ]]; then
    echo "ERROR: hub.json not found at $HUB_JSON" >&2
    echo "       Create it with: {\"id\": \"hub1\", \"type\": \"hub\", \"ip\": \"<IP>\", \"nats_port\": 4222}" >&2
    exit 1
  fi
  if ! command -v jq &>/dev/null; then
    echo "ERROR: jq is required" >&2; exit 1
  fi
  HUB_ID=$(jq -r '.id // "hub1"' "$HUB_JSON")
  HUB_TYPE=$(jq -r '.type // "hub"' "$HUB_JSON")
  HUB_IP_JSON=$(jq -r '.ip // empty' "$HUB_JSON")
  NATS_PORT=$(jq -r '.nats_port // 4222' "$HUB_JSON")
  [[ -z "$HUB_IP" ]] && HUB_IP="$HUB_IP_JSON"
  [[ -z "$HUB_USER" ]] && HUB_USER=$(jq -r '.ssh_user // "root"' "$HUB_JSON")
  if [[ -z "$HUB_IP" ]]; then
    echo "ERROR: no ip in hub.json and --hub-ip not supplied" >&2; exit 1
  fi
}

# ---- generate auth token -----------------------------------------------------
gen_token() {
  # idempotent: reuse existing token if already stored
  if [[ -f "$SECRET_STORE" ]]; then
    existing=$(grep '^export NATS_BRIDGE_TOKEN=' "$SECRET_STORE" 2>/dev/null | head -1 | cut -d= -f2- | tr -d "'\"")
    if [[ -n "$existing" ]]; then
      echo "$existing"
      return
    fi
  fi
  openssl rand -hex 32
}

# ---- nats-server unit snippet ------------------------------------------------
nats_server_unit() {
  local port="$1" token="$2"
  cat <<UNIT
[Unit]
Description=NATS Server (harbor-bridge landing pad)
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/nats-server \\
  --jetstream \\
  --port ${port} \\
  --auth ${token}
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT
}

# ---- harbor-heartbeat service unit ------------------------------------------
heartbeat_service_unit() {
  local hub_id="$1" hub_type="$2" hub_ip="$3" nats_port="$4"
  cat <<UNIT
[Unit]
Description=harbor-heartbeat — hub announces on wm.fleet.hub.*
After=nats-server.service
Wants=nats-server.service

[Service]
EnvironmentFile=/etc/harbor-bridge/bridge.env
ExecStart=/usr/local/bin/harbor-heartbeat-loop \\
  --id "${hub_id}" \\
  --type "${hub_type}" \\
  --ip "${hub_ip}" \\
  --nats-port "${nats_port}"
ExecStop=/usr/local/bin/harbor-heartbeat-publish \\
  --subj ${SUBJ_DOWN} \\
  --id "${hub_id}" --type "${hub_type}" --ip "${hub_ip}"
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
UNIT
}

# ---- harbor-heartbeat timer unit --------------------------------------------
heartbeat_timer_unit() {
  cat <<UNIT
[Unit]
Description=harbor-heartbeat periodic timer
Requires=harbor-heartbeat.service

[Timer]
OnBootSec=10
OnUnitActiveSec=30

[Install]
WantedBy=timers.target
UNIT
}

# ---- heartbeat publish helper script ----------------------------------------
heartbeat_publish_script() {
  local hub_id="$1" hub_type="$2" hub_ip="$3" nats_port="$4"
  cat <<'SCRIPT'
#!/usr/bin/env bash
# harbor-heartbeat-publish — publish one event to NATS wm.fleet.hub.*
# Usage: harbor-heartbeat-publish --subj SUBJ --id ID --type TYPE --ip IP [--nats-port PORT]
set -euo pipefail
source /etc/harbor-bridge/bridge.env 2>/dev/null || true

SUBJ=""
HUB_ID=""
HUB_TYPE=""
HUB_IP=""
NATS_PORT="${NATS_PORT:-4222}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --subj)      SUBJ="$2";     shift ;;
    --id)        HUB_ID="$2";   shift ;;
    --type)      HUB_TYPE="$2"; shift ;;
    --ip)        HUB_IP="$2";   shift ;;
    --nats-port) NATS_PORT="$2";shift ;;
  esac; shift
done

TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)
PAYLOAD=$(printf '{"id":"%s","type":"%s","ip":"%s","ts":"%s"}' \
  "$HUB_ID" "$HUB_TYPE" "$HUB_IP" "$TS")

nats pub --server "nats://${NATS_BRIDGE_TOKEN}@127.0.0.1:${NATS_PORT}" \
  "$SUBJ" "$PAYLOAD"
SCRIPT
}

# ---- heartbeat loop script ---------------------------------------------------
heartbeat_loop_script() {
  local hub_id="$1" hub_type="$2" hub_ip="$3" nats_port="$4"
  cat <<'SCRIPT'
#!/usr/bin/env bash
# harbor-heartbeat-loop — publish up then periodic heartbeats
set -euo pipefail
source /etc/harbor-bridge/bridge.env 2>/dev/null || true

HUB_ID="${1:-hub1}"
HUB_TYPE="${2:-hub}"
HUB_IP="${3:-127.0.0.1}"
NATS_PORT="${4:-4222}"
INTERVAL=30

while [[ $# -gt 0 ]]; do
  case "$1" in
    --id)        HUB_ID="$2";   shift ;;
    --type)      HUB_TYPE="$2"; shift ;;
    --ip)        HUB_IP="$2";   shift ;;
    --nats-port) NATS_PORT="$2";shift ;;
    --interval)  INTERVAL="$2"; shift ;;
  esac; shift
done

SUBJ_UP="wm.fleet.hub.up"
SUBJ_HB="wm.fleet.hub.heartbeat"

pub_one() {
  local subj="$1"
  local ts; ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  local payload; payload=$(printf '{"id":"%s","type":"%s","ip":"%s","ts":"%s"}' \
    "$HUB_ID" "$HUB_TYPE" "$HUB_IP" "$ts")
  nats pub --server "nats://${NATS_BRIDGE_TOKEN}@127.0.0.1:${NATS_PORT}" \
    "$subj" "$payload"
}

pub_one "$SUBJ_UP"
while true; do
  sleep "$INTERVAL"
  pub_one "$SUBJ_HB"
done
SCRIPT
}

# =============================================================================
# main
# =============================================================================

load_hub_json

TOKEN=$(gen_token)

if "$DRY_RUN"; then
  echo "=== harbor-bridge-up.sh --dry-run ==="
  echo "Hub:          ${HUB_USER}@${HUB_IP}"
  echo "Hub ID:       ${HUB_ID}  type=${HUB_TYPE}"
  echo "NATS port:    ${NATS_PORT}"
  echo "Subjects:     ${SUBJ_UP}  ${SUBJ_DOWN}  ${SUBJ_HEARTBEAT}"
  echo "Secret store: ${SECRET_STORE}  (token NEVER written to tracked files)"
  echo ""
  echo "--- Planned: nats-server.service ---"
  nats_server_unit "$NATS_PORT" "<NATS_BRIDGE_TOKEN>"
  echo ""
  echo "--- Planned: harbor-heartbeat.service ---"
  heartbeat_service_unit "$HUB_ID" "$HUB_TYPE" "$HUB_IP" "$NATS_PORT"
  echo ""
  echo "--- Planned: heartbeat payload sample ---"
  TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  printf '{"id":"%s","type":"%s","ip":"%s","ts":"%s"}\n' \
    "$HUB_ID" "$HUB_TYPE" "$HUB_IP" "$TS"
  echo ""
  # Idempotency marker: check if bridge.env already has a token
  if [[ -f "$SECRET_STORE" ]] && grep -q '^export NATS_BRIDGE_TOKEN=' "$SECRET_STORE" 2>/dev/null; then
    echo "IDEMPOTENCY: bridge.env already contains NATS_BRIDGE_TOKEN — reconcile units, no duplicate"
  else
    echo "IDEMPOTENCY: fresh install — token will be written to ${SECRET_STORE}"
  fi
  exit 0
fi

# ---- write token to secret store (never to tracked files) -------------------
mkdir -p "$(dirname "$SECRET_STORE")"
if ! grep -q '^export NATS_BRIDGE_TOKEN=' "$SECRET_STORE" 2>/dev/null; then
  echo "export NATS_BRIDGE_TOKEN='${TOKEN}'" >> "$SECRET_STORE"
  chmod 600 "$SECRET_STORE"
  echo "[harbor-bridge] Token written to ${SECRET_STORE}"
else
  echo "[harbor-bridge] Token already in ${SECRET_STORE} — reusing (idempotent)"
fi

# ---- deploy to hub via SSH --------------------------------------------------
SSH="ssh ${HUB_USER}@${HUB_IP}"

echo "[harbor-bridge] Installing nats-server on ${HUB_IP}..."
$SSH bash -s <<EOF
set -euo pipefail
if ! command -v nats-server &>/dev/null; then
  curl -sf https://get-nats.io/install.sh | bash
fi
# Install nats CLI if absent (for pub/sub)
if ! command -v nats &>/dev/null; then
  NATS_CLI_VER=\$(curl -s https://api.github.com/repos/nats-io/natscli/releases/latest | grep tag_name | cut -d'"' -f4)
  curl -sL "https://github.com/nats-io/natscli/releases/download/\${NATS_CLI_VER}/nats-\${NATS_CLI_VER}-linux-amd64.zip" -o /tmp/nats-cli.zip
  unzip -q /tmp/nats-cli.zip -d /tmp/nats-cli-tmp
  install -m755 /tmp/nats-cli-tmp/nats-*/nats /usr/local/bin/nats
  rm -rf /tmp/nats-cli.zip /tmp/nats-cli-tmp
fi
echo "nats-server: \$(nats-server --version)"
EOF

echo "[harbor-bridge] Writing unit files..."
NATS_UNIT=$(nats_server_unit "$NATS_PORT" "$TOKEN")
HB_SVC=$(heartbeat_service_unit "$HUB_ID" "$HUB_TYPE" "$HUB_IP" "$NATS_PORT")
HB_PUBLISH=$(heartbeat_publish_script "$HUB_ID" "$HUB_TYPE" "$HUB_IP" "$NATS_PORT")
HB_LOOP=$(heartbeat_loop_script "$HUB_ID" "$HUB_TYPE" "$HUB_IP" "$NATS_PORT")

$SSH bash -s <<REMOTE
set -euo pipefail
mkdir -p /etc/harbor-bridge /etc/systemd/system

# bridge.env on hub (secrets, not tracked)
cat > /etc/harbor-bridge/bridge.env <<ENV
export NATS_BRIDGE_TOKEN='${TOKEN}'
export NATS_PORT='${NATS_PORT}'
ENV
chmod 600 /etc/harbor-bridge/bridge.env

# nats-server unit
cat > /etc/systemd/system/nats-server.service <<'UNIT'
${NATS_UNIT}
UNIT

# harbor-heartbeat service
cat > /etc/systemd/system/harbor-heartbeat.service <<'UNIT'
${HB_SVC}
UNIT

# helper scripts
cat > /usr/local/bin/harbor-heartbeat-publish <<'SCR'
${HB_PUBLISH}
SCR
chmod +x /usr/local/bin/harbor-heartbeat-publish

cat > /usr/local/bin/harbor-heartbeat-loop <<'SCR'
${HB_LOOP}
SCR
chmod +x /usr/local/bin/harbor-heartbeat-loop

systemctl daemon-reload
systemctl enable --now nats-server.service
systemctl enable --now harbor-heartbeat.service
echo "[harbor-bridge] Units enabled and started — reconcile complete, no duplicate"
REMOTE

echo "[harbor-bridge] Done. Hub is announcing on ${SUBJ_UP} / ${SUBJ_HEARTBEAT} / ${SUBJ_DOWN}"
