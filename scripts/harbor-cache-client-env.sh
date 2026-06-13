#!/usr/bin/env bash
# harbor-cache-client-env.sh — emit SCCACHE_* env vars that clients source.
#
# Usage:
#   harbor-cache-client-env.sh [--hub-json PATH] [--cache-env PATH] [--write]
#
# Options:
#   --hub-json PATH   Path to hub.json (default: ~/.config/wm-burst/hub.json).
#   --cache-env PATH  Path to cache.env (default: ~/.config/wm-burst/cache.env).
#   --write           Generate a fresh cache.env stub (no secrets) so wm-burst
#                     can populate keys later. Skipped if cache.env already exists.
#
# Outputs shell-exportable lines to stdout; no secrets are emitted to stdout
# beyond what already lives in cache.env (which is 600, gitignored).
#
# Exit codes:
#   0  Success — all required vars emitted.
#   1  No hub.json found — run `wm-burst hub up` first.
#   2  No cache.env found — run harbor-cache-up.sh on the hub first.

set -euo pipefail

HUB_JSON="${HOME}/.config/wm-burst/hub.json"
CACHE_ENV="${HOME}/.config/wm-burst/cache.env"
WRITE_STUB=0
MINIO_PORT=9000
BUCKET_NAME="wm-sccache"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --hub-json)   HUB_JSON="$2"; shift 2 ;;
        --cache-env)  CACHE_ENV="$2"; shift 2 ;;
        --write)      WRITE_STUB=1; shift ;;
        *)  echo "error: unknown argument '$1'" >&2; exit 1 ;;
    esac
done

# ── require hub.json ─────────────────────────────────────────────────────────
if [[ ! -f "$HUB_JSON" ]]; then
    echo "error: no hub — run \`wm-burst hub up\` first (expected: ${HUB_JSON})" >&2
    exit 1
fi

# Parse hub IP from hub.json.
HUB_IP="$(python3 -c "import json,sys; d=json.load(open('$HUB_JSON')); print(d.get('ip',''))" 2>/dev/null || true)"
if [[ -z "$HUB_IP" ]]; then
    echo "error: hub.json exists but contains no 'ip' field — re-run \`wm-burst hub up\`" >&2
    exit 1
fi

SCCACHE_ENDPOINT="http://${HUB_IP}:${MINIO_PORT}"
SCCACHE_BUCKET="${BUCKET_NAME}"

# ── write stub if requested ───────────────────────────────────────────────────
if [[ $WRITE_STUB -eq 1 && ! -f "$CACHE_ENV" ]]; then
    mkdir -p "$(dirname "$CACHE_ENV")"
    cat > "$CACHE_ENV" <<EOF
# wm-burst sccache client environment — DO NOT COMMIT
# Stub written by harbor-cache-client-env.sh — populate keys after running harbor-cache-up.sh on hub.
export SCCACHE_ENDPOINT=${SCCACHE_ENDPOINT}
export SCCACHE_BUCKET=${SCCACHE_BUCKET}
export SCCACHE_S3_USE_SSL=false
export AWS_ACCESS_KEY_ID=REPLACE_ME
export AWS_SECRET_ACCESS_KEY=REPLACE_ME
export RUSTC_WRAPPER=sccache
EOF
    chmod 600 "$CACHE_ENV"
    echo "# Stub written to ${CACHE_ENV} — replace REPLACE_ME values after running harbor-cache-up.sh on the hub." >&2
fi

# ── require cache.env ────────────────────────────────────────────────────────
if [[ ! -f "$CACHE_ENV" ]]; then
    echo "error: no cache.env — run harbor-cache-up.sh on the hub first (expected: ${CACHE_ENV})" >&2
    exit 2
fi

# Load credentials from cache.env (keys never re-emitted to stdout here).
# shellcheck disable=SC1090
source "$CACHE_ENV"

# Override endpoint/bucket from live hub.json in case they differ from stored values.
export SCCACHE_ENDPOINT="${SCCACHE_ENDPOINT}"
export SCCACHE_BUCKET="${SCCACHE_BUCKET}"
export SCCACHE_S3_USE_SSL="${SCCACHE_S3_USE_SSL:-false}"
export RUSTC_WRAPPER="sccache"

# Validate required vars are set (sourced from cache.env).
MISSING=()
for VAR in AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY; do
    if [[ -z "${!VAR:-}" ]]; then
        MISSING+=("$VAR")
    fi
done
if [[ ${#MISSING[@]} -gt 0 ]]; then
    echo "error: cache.env is missing required vars: ${MISSING[*]}" >&2
    echo "error: run harbor-cache-up.sh on the hub to populate them" >&2
    exit 2
fi

# Emit all required vars to stdout for sourcing.
cat <<EOF
export SCCACHE_ENDPOINT=${SCCACHE_ENDPOINT}
export SCCACHE_BUCKET=${SCCACHE_BUCKET}
export SCCACHE_S3_USE_SSL=${SCCACHE_S3_USE_SSL:-false}
export AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}
export AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}
export RUSTC_WRAPPER=sccache
EOF
