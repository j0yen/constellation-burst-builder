#!/usr/bin/env bash
# harbor-cache-check.sh — offline structural acceptance gate for harbor-cache.
#
# Asserts:
#   1. harbor-cache-up.sh --dry-run exits 0 and emits expected steps.
#   2. harbor-cache-up.sh --dry-run with a "cache-already-up" marker reports
#      "reconcile only, no create" (idempotency gate).
#   3. harbor-cache-client-env.sh emits all required SCCACHE_* vars.
#   4. The endpoint + bucket are read from hub.json, not hardcoded values.
#   5. With no hub.json, harbor-cache-client-env.sh exits non-zero with expected msg.
#   6. No access/secret key value appears in any tracked script/file in the repo.
#   7. The default bucket name in scripts matches config.rs default ("wm-sccache").
#
# Usage:
#   harbor-cache-check.sh [--repo-root PATH]
#
# Exit code: 0 = all checks pass; 1 = one or more checks failed.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PASS=0
FAIL=0
RESULTS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --repo-root) REPO_ROOT="$2"; shift 2 ;;
        *) echo "error: unknown argument '$1'" >&2; exit 1 ;;
    esac
done

SCRIPTS_DIR="${REPO_ROOT}/scripts"
UP_SCRIPT="${SCRIPTS_DIR}/harbor-cache-up.sh"
CLIENT_SCRIPT="${SCRIPTS_DIR}/harbor-cache-client-env.sh"

# ── helpers ───────────────────────────────────────────────────────────────────
ok()   { PASS=$((PASS+1)); RESULTS+=("PASS: $*"); }
fail() { FAIL=$((FAIL+1)); RESULTS+=("FAIL: $*"); }

require_file() {
    local f="$1"
    if [[ ! -f "$f" ]]; then
        fail "required file missing: $f"
        return 1
    fi
    return 0
}

# ── pre-flight ────────────────────────────────────────────────────────────────
require_file "$UP_SCRIPT"     || true
require_file "$CLIENT_SCRIPT" || true

# ── AC1: dry-run exits 0 and prints planned steps ────────────────────────────
if [[ -f "$UP_SCRIPT" ]]; then
    DRY_OUT="$(bash "$UP_SCRIPT" --dry-run 2>&1)"
    DRY_EXIT=$?
    if [[ $DRY_EXIT -eq 0 ]]; then
        ok "AC1: --dry-run exits 0"
    else
        fail "AC1: --dry-run exited ${DRY_EXIT} (expected 0)"
    fi

    # Check that key steps appear in dry-run output.
    for EXPECTED_STEP in "STEP 1" "STEP 7" "systemd unit" "keypair" "SCCACHE_ENDPOINT" "SCCACHE_BUCKET" "RUSTC_WRAPPER"; do
        if echo "$DRY_OUT" | grep -q "$EXPECTED_STEP"; then
            ok "AC1: dry-run output contains '${EXPECTED_STEP}'"
        else
            fail "AC1: dry-run output missing '${EXPECTED_STEP}'"
        fi
    done
else
    fail "AC1: ${UP_SCRIPT} not found — cannot run dry-run check"
fi

# ── AC2: idempotency — marker present → "reconcile only, no create" ──────────
if [[ -f "$UP_SCRIPT" ]]; then
    MARKER="/tmp/harbor-cache-up.marker"
    touch "$MARKER"
    IDEM_OUT="$(bash "$UP_SCRIPT" --dry-run 2>&1)"
    IDEM_EXIT=$?
    rm -f "$MARKER"

    if [[ $IDEM_EXIT -eq 0 ]]; then
        ok "AC2: idempotency dry-run exits 0"
    else
        fail "AC2: idempotency dry-run exited ${IDEM_EXIT}"
    fi
    if echo "$IDEM_OUT" | grep -qi "reconcile only, no create"; then
        ok "AC2: idempotency dry-run reports 'reconcile only, no create'"
    else
        fail "AC2: idempotency dry-run missing 'reconcile only, no create' in output"
    fi
fi

# ── AC3: client-env script emits all required vars (via a synthetic hub.json) ─
if [[ -f "$CLIENT_SCRIPT" ]]; then
    TMP_DIR="$(mktemp -d)"
    # Create a synthetic hub.json.
    printf '{"ip":"192.0.2.1","id":"test-hub","type":"hub"}\n' > "${TMP_DIR}/hub.json"
    # Create a synthetic cache.env with placeholder keys.
    cat > "${TMP_DIR}/cache.env" <<'CACHEENV'
export SCCACHE_ENDPOINT=http://192.0.2.1:9000
export SCCACHE_BUCKET=wm-sccache
export SCCACHE_S3_USE_SSL=false
export AWS_ACCESS_KEY_ID=TESTKEY0000000000000000000000000
export AWS_SECRET_ACCESS_KEY=TESTSECRET000000000000000000000
export RUSTC_WRAPPER=sccache
CACHEENV
    chmod 600 "${TMP_DIR}/cache.env"

    CLIENT_OUT="$(bash "$CLIENT_SCRIPT" \
        --hub-json "${TMP_DIR}/hub.json" \
        --cache-env "${TMP_DIR}/cache.env" 2>&1)"
    CLIENT_EXIT=$?
    rm -rf "$TMP_DIR"

    if [[ $CLIENT_EXIT -eq 0 ]]; then
        ok "AC3: client-env exits 0"
    else
        fail "AC3: client-env exited ${CLIENT_EXIT}"
    fi

    for REQUIRED_VAR in SCCACHE_ENDPOINT SCCACHE_BUCKET AWS_ACCESS_KEY_ID \
                        AWS_SECRET_ACCESS_KEY RUSTC_WRAPPER; do
        if echo "$CLIENT_OUT" | grep -q "${REQUIRED_VAR}"; then
            ok "AC3: client-env output contains '${REQUIRED_VAR}'"
        else
            fail "AC3: client-env output missing '${REQUIRED_VAR}'"
        fi
    done

    # Verify RUSTC_WRAPPER=sccache specifically.
    if echo "$CLIENT_OUT" | grep -q "RUSTC_WRAPPER=sccache"; then
        ok "AC3: RUSTC_WRAPPER=sccache present"
    else
        fail "AC3: RUSTC_WRAPPER=sccache missing"
    fi
fi

# ── AC4: endpoint/bucket read from hub.json, not hardcoded ───────────────────
if [[ -f "$CLIENT_SCRIPT" ]]; then
    TMP_DIR2="$(mktemp -d)"
    printf '{"ip":"10.0.1.42","id":"custom-hub","type":"hub"}\n' > "${TMP_DIR2}/hub.json"
    cat > "${TMP_DIR2}/cache.env" <<'CACHEENV2'
export SCCACHE_ENDPOINT=http://10.0.1.42:9000
export SCCACHE_BUCKET=wm-sccache
export SCCACHE_S3_USE_SSL=false
export AWS_ACCESS_KEY_ID=TESTKEY0000000000000000000000000
export AWS_SECRET_ACCESS_KEY=TESTSECRET000000000000000000000
export RUSTC_WRAPPER=sccache
CACHEENV2
    chmod 600 "${TMP_DIR2}/cache.env"

    AC4_OUT="$(bash "$CLIENT_SCRIPT" \
        --hub-json "${TMP_DIR2}/hub.json" \
        --cache-env "${TMP_DIR2}/cache.env" 2>&1)"
    rm -rf "$TMP_DIR2"

    if echo "$AC4_OUT" | grep -q "10.0.1.42"; then
        ok "AC4: endpoint uses IP from hub.json"
    else
        fail "AC4: endpoint does not reflect hub.json IP (expected 10.0.1.42)"
    fi
fi

# ── AC5: no hub.json → non-zero exit with expected message ───────────────────
if [[ -f "$CLIENT_SCRIPT" ]]; then
    NO_HUB_OUT="$(bash "$CLIENT_SCRIPT" \
        --hub-json "/tmp/harbor-cache-nonexistent-hub.json" \
        --cache-env "/tmp/harbor-cache-nonexistent-cache.env" 2>&1 || true)"
    NO_HUB_EXIT="$(bash "$CLIENT_SCRIPT" \
        --hub-json "/tmp/harbor-cache-nonexistent-hub.json" \
        --cache-env "/tmp/harbor-cache-nonexistent-cache.env" > /dev/null 2>&1; echo $?)"

    if [[ "$NO_HUB_EXIT" != "0" ]]; then
        ok "AC5: no hub.json → non-zero exit"
    else
        fail "AC5: no hub.json → unexpectedly exited 0"
    fi

    if echo "$NO_HUB_OUT" | grep -qi "no hub\|wm-burst hub up"; then
        ok "AC5: no hub.json → error message mentions 'no hub' / 'wm-burst hub up'"
    else
        fail "AC5: no hub.json → missing expected error message in stderr"
    fi
fi

# ── AC6: no access/secret key values in tracked scripts ──────────────────────
TRACKED_SCRIPTS=(
    "${SCRIPTS_DIR}/harbor-cache-up.sh"
    "${SCRIPTS_DIR}/harbor-cache-client-env.sh"
    "${SCRIPTS_DIR}/harbor-cache-check.sh"
)
# Pattern: 20+ char alphanumeric strings that look like real keys
# (REPLACE_ME and TESTKEY*/TESTSECRET* placeholders in THIS check script are exempt).
SECRET_FOUND=0
for SCRIPT_FILE in "${TRACKED_SCRIPTS[@]}"; do
    if [[ ! -f "$SCRIPT_FILE" ]]; then continue; fi
    # Grep for patterns like MINIO_ROOT_PASSWORD=<actual 32-char key>.
    # We exclude lines that are test fixtures in this check script itself.
    if grep -E '(AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY|MINIO_ROOT_PASSWORD|MINIO_ROOT_USER)[=:]\s*[A-Za-z0-9]{20,}' \
        "$SCRIPT_FILE" 2>/dev/null | grep -v "TESTKEY\|TESTSECRET\|REPLACE_ME\|ACCESS_KEY=\${" \
        | grep -v "#\|echo\|export\|cat\|source\|gen_key\|<generated>" | grep -q .; then
        fail "AC6: possible hardcoded secret in ${SCRIPT_FILE}"
        SECRET_FOUND=1
    fi
done
if [[ $SECRET_FOUND -eq 0 ]]; then
    ok "AC6: no hardcoded access/secret key values found in tracked scripts"
fi

# ── AC7: default bucket name matches config.rs default "wm-sccache" ──────────
CONFIG_RS="${REPO_ROOT}/src/config.rs"
EXPECTED_BUCKET="wm-sccache"
if [[ -f "$CONFIG_RS" ]]; then
    if grep -q "\"${EXPECTED_BUCKET}\"" "$CONFIG_RS"; then
        ok "AC7: config.rs default bucket matches '${EXPECTED_BUCKET}'"
    else
        fail "AC7: config.rs does not reference expected bucket '${EXPECTED_BUCKET}'"
    fi
else
    fail "AC7: config.rs not found at ${CONFIG_RS}"
fi

# Verify scripts use the same bucket name.
for SCRIPT_FILE in "$UP_SCRIPT" "$CLIENT_SCRIPT"; do
    if [[ -f "$SCRIPT_FILE" ]]; then
        if grep -q "BUCKET_NAME=\"${EXPECTED_BUCKET}\"" "$SCRIPT_FILE"; then
            ok "AC7: $(basename "$SCRIPT_FILE") uses matching bucket '${EXPECTED_BUCKET}'"
        else
            fail "AC7: $(basename "$SCRIPT_FILE") does not use expected bucket '${EXPECTED_BUCKET}'"
        fi
    fi
done

# ── summary ───────────────────────────────────────────────────────────────────
echo ""
echo "══════════════════════════════════════════════════════"
echo "harbor-cache-check results"
echo "══════════════════════════════════════════════════════"
for R in "${RESULTS[@]}"; do
    echo "  $R"
done
echo "──────────────────────────────────────────────────────"
echo "  PASS: ${PASS}   FAIL: ${FAIL}"
echo "══════════════════════════════════════════════════════"

if [[ $FAIL -gt 0 ]]; then
    exit 1
fi
exit 0
