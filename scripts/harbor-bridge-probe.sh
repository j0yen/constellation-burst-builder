#!/usr/bin/env bash
# harbor-bridge-probe.sh — check if the hub NATS heartbeat is live
# Usage: harbor-bridge-probe.sh [--dry-run] [--timeout SECS]
#
# Connects to the hub NATS endpoint (from hub.json + bridge.env token),
# subscribes to wm.fleet.hub.heartbeat, waits up to --timeout seconds,
# and reports: "hub reachable" / "hub stale" / "hub down".
#
# Exit codes:
#   0  hub reachable (recent heartbeat seen)
#   1  hub stale (heartbeat > 90 s old or no heartbeat within timeout)
#   2  hub down (connection refused / no hub.json)
#   3  no hub configured (hub.json missing)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
# Allow env override for testing
HUB_JSON="${HUB_JSON:-${REPO_ROOT}/hub.json}"
SECRET_STORE="${SECRET_STORE:-${HOME}/.config/wm-burst/bridge.env}"

SUBJ_HEARTBEAT="wm.fleet.hub.heartbeat"
STALE_SECS=90
TIMEOUT=10
DRY_RUN=false

usage() {
  echo "Usage: $0 [--dry-run] [--timeout SECS] [--stale-secs SECS]"
  echo "  --dry-run      Explain what would happen; do not connect"
  echo "  --timeout N    Max seconds to wait for a heartbeat (default: ${TIMEOUT})"
  echo "  --stale-secs N Heartbeat age threshold for 'stale' (default: ${STALE_SECS})"
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run)    DRY_RUN=true ;;
    --timeout)    TIMEOUT="$2"; shift ;;
    --stale-secs) STALE_SECS="$2"; shift ;;
    -h|--help)    usage ;;
    *) echo "Unknown arg: $1" >&2; exit 1 ;;
  esac
  shift
done

# ---- no hub.json → exit 3 ----------------------------------------------------
if [[ ! -f "$HUB_JSON" ]]; then
  echo "ERROR: no hub — hub.json not found at ${HUB_JSON}" >&2
  echo "       Create it with: {\"id\": \"hub1\", \"type\": \"hub\", \"ip\": \"<IP>\", \"nats_port\": 4222}" >&2
  exit 3
fi

if ! command -v jq &>/dev/null; then
  echo "ERROR: jq is required" >&2; exit 2
fi

HUB_IP=$(jq -r '.ip // empty' "$HUB_JSON")
NATS_PORT=$(jq -r '.nats_port // 4222' "$HUB_JSON")

if [[ -z "$HUB_IP" ]]; then
  echo "ERROR: no ip in hub.json" >&2; exit 3
fi

# ---- load token from secret store -------------------------------------------
NATS_BRIDGE_TOKEN=""
if [[ -f "$SECRET_STORE" ]]; then
  NATS_BRIDGE_TOKEN=$(grep '^export NATS_BRIDGE_TOKEN=' "$SECRET_STORE" 2>/dev/null | head -1 | cut -d= -f2- | tr -d "'\"")
fi
if [[ -z "$NATS_BRIDGE_TOKEN" ]]; then
  echo "WARN: no NATS_BRIDGE_TOKEN in ${SECRET_STORE} — connection will attempt without auth" >&2
fi

NATS_SERVER="nats://${NATS_BRIDGE_TOKEN:+${NATS_BRIDGE_TOKEN}@}${HUB_IP}:${NATS_PORT}"

# ---- dry-run -----------------------------------------------------------------
if "$DRY_RUN"; then
  echo "=== harbor-bridge-probe.sh --dry-run ==="
  echo "Hub:          ${HUB_IP}:${NATS_PORT}"
  echo "Subject:      ${SUBJ_HEARTBEAT}"
  echo "Timeout:      ${TIMEOUT}s"
  echo "Stale after:  ${STALE_SECS}s"
  echo ""
  echo "Would subscribe to ${SUBJ_HEARTBEAT} for up to ${TIMEOUT}s."
  echo "Classify result:"
  echo "  - heartbeat received with ts ≤ ${STALE_SECS}s ago → EXIT 0 (hub reachable)"
  echo "  - heartbeat received with ts >  ${STALE_SECS}s ago → EXIT 1 (hub stale)"
  echo "  - no heartbeat within ${TIMEOUT}s                  → EXIT 1 (hub stale)"
  echo "  - connection refused / NATS error                  → EXIT 2 (hub down)"
  exit 0
fi

# ---- check nats CLI ----------------------------------------------------------
if ! command -v nats &>/dev/null; then
  echo "ERROR: nats CLI not found; install from https://github.com/nats-io/natscli/releases" >&2
  exit 2
fi

# ---- subscribe and parse heartbeat -------------------------------------------
echo "[probe] Subscribing to ${SUBJ_HEARTBEAT} on ${HUB_IP}:${NATS_PORT} (timeout ${TIMEOUT}s)..."

MSG=$(timeout "$TIMEOUT" nats sub --server "$NATS_SERVER" \
  --count 1 --timeout "${TIMEOUT}s" \
  "$SUBJ_HEARTBEAT" 2>/dev/null | tail -n1) || true

if [[ -z "$MSG" ]]; then
  echo "RESULT: hub down (no message received within ${TIMEOUT}s)" >&2
  exit 2
fi

# parse ts from JSON payload
TS=$(echo "$MSG" | jq -r '.ts // empty' 2>/dev/null)
if [[ -z "$TS" ]]; then
  echo "RESULT: hub stale (heartbeat received but ts field missing in payload)" >&2
  exit 1
fi

NOW=$(date -u +%s)
HB_EPOCH=$(date -u -d "$TS" +%s 2>/dev/null) || {
  echo "RESULT: hub stale (could not parse ts=${TS})" >&2; exit 1
}
AGE=$(( NOW - HB_EPOCH ))

if [[ "$AGE" -le "$STALE_SECS" ]]; then
  echo "RESULT: hub reachable (heartbeat ${AGE}s ago, payload: ${MSG})"
  exit 0
else
  echo "RESULT: hub stale (heartbeat ${AGE}s ago > threshold ${STALE_SECS}s, payload: ${MSG})" >&2
  exit 1
fi
