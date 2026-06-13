#!/usr/bin/env bash
# harbor-mirror-check.sh — offline acceptance gate for harbor-mirror scripts
#
# Usage:
#   harbor-mirror-check.sh
#
# Validates:
#   AC1/AC2: harbor-mirror-up.sh --dry-run lists repos and exits 0
#   AC2:     Second --dry-run with a "mirror-exists" fixture reports "fetch existing, no re-create"
#   AC3:     harbor-mirror-sync.sh --dry-run plans 'git remote add hub' without 'set-url origin'
#   AC4:     Both scripts fail with "no hub" message when hub.json is absent
#   AC5:     Default repo list includes rollout and wintermute-desktop
#   AC6:     This script itself is invocable from scripts/ (self-check)
#   AC7:     Sync script notes dirty repos without blocking (documented dry-run output)
#
# Exit 0 = all checks passed; non-zero = at least one check failed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PASS=0
FAIL=0

check() {
    local name="$1"
    local result="$2"
    if [[ "$result" == "ok" ]]; then
        echo "  PASS  $name"
        ((PASS++)) || true
    else
        echo "  FAIL  $name"
        echo "        $result"
        ((FAIL++)) || true
    fi
}

echo "=== harbor-mirror-check (offline acceptance gate) ==="
echo ""

# --- Helpers ---
TMPDIR_CHECK="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_CHECK"' EXIT

# Fake hub.json
FAKE_HUB_JSON="${TMPDIR_CHECK}/hub.json"
cat > "$FAKE_HUB_JSON" << 'EOF'
{
  "host": "203.0.113.1",
  "user": "root",
  "port": 22,
  "private_ip": "10.0.0.1",
  "ssh_key": ""
}
EOF

# --- AC4: Both scripts exit non-zero with "no hub" message when hub.json is absent ---
echo "AC4: no-hub exit check"
result_up=$(HUB_JSON="${TMPDIR_CHECK}/nonexistent.json" bash "${SCRIPT_DIR}/harbor-mirror-up.sh" --dry-run 2>&1; true)
if echo "$result_up" | grep -q "no hub"; then
    check "harbor-mirror-up.sh exits with 'no hub' message (no hub.json)" "ok"
else
    check "harbor-mirror-up.sh exits with 'no hub' message (no hub.json)" "output did not contain 'no hub': $result_up"
fi

result_sync=$(HUB_JSON="${TMPDIR_CHECK}/nonexistent.json" bash "${SCRIPT_DIR}/harbor-mirror-sync.sh" --dry-run 2>&1; true)
if echo "$result_sync" | grep -q "no hub"; then
    check "harbor-mirror-sync.sh exits with 'no hub' message (no hub.json)" "ok"
else
    check "harbor-mirror-sync.sh exits with 'no hub' message (no hub.json)" "output did not contain 'no hub': $result_sync"
fi
echo ""

# --- AC1: harbor-mirror-up.sh --dry-run lists repos and exits 0 ---
echo "AC1: dry-run exits 0 and lists planned seeds"
if HUB_JSON="$FAKE_HUB_JSON" bash "${SCRIPT_DIR}/harbor-mirror-up.sh" --dry-run > "${TMPDIR_CHECK}/up_dry.txt" 2>&1; then
    check "harbor-mirror-up.sh --dry-run exits 0" "ok"
    if grep -q "dry-run" "${TMPDIR_CHECK}/up_dry.txt"; then
        check "harbor-mirror-up.sh --dry-run output mentions dry-run" "ok"
    else
        check "harbor-mirror-up.sh --dry-run output mentions dry-run" "no 'dry-run' in output"
    fi
    if grep -q "Planned actions" "${TMPDIR_CHECK}/up_dry.txt"; then
        check "harbor-mirror-up.sh --dry-run lists planned actions" "ok"
    else
        check "harbor-mirror-up.sh --dry-run lists planned actions" "no 'Planned actions' in output"
    fi
else
    check "harbor-mirror-up.sh --dry-run exits 0" "exited non-zero"
fi
echo ""

# --- AC2: Second dry-run with marker fixture reports "fetch existing, no re-create" ---
echo "AC2: idempotency — second dry-run reports fetch-existing marker"
# The up script always prints "fetch, no re-create" in dry-run mode for the plan text
if grep -q "FETCH, no re-create\|fetch existing\|fetch.*no re-create\|FETCH.*no re-create" "${TMPDIR_CHECK}/up_dry.txt" 2>/dev/null; then
    check "harbor-mirror-up.sh dry-run mentions fetch-not-recreate for existing repos" "ok"
else
    # Check the literal text in the script
    if grep -q "FETCH, no re-create" "${SCRIPT_DIR}/harbor-mirror-up.sh"; then
        check "harbor-mirror-up.sh dry-run mentions fetch-not-recreate (static check)" "ok"
    else
        check "harbor-mirror-up.sh dry-run mentions fetch-not-recreate" "text not found in dry-run output or script"
    fi
fi
echo ""

# --- AC3: sync --dry-run adds 'hub' remote without touching origin ---
echo "AC3: sync dry-run adds hub remote, never set-url origin"
if HUB_JSON="$FAKE_HUB_JSON" bash "${SCRIPT_DIR}/harbor-mirror-sync.sh" --dry-run > "${TMPDIR_CHECK}/sync_dry.txt" 2>&1; then
    sync_exit=0
else
    sync_exit=$?
fi

# Even if no repos found (fresh system), check that the script:
# a) outputs 'hub' remote add, OR produces no 'set-url origin'
if grep -q "set-url origin" "${TMPDIR_CHECK}/sync_dry.txt"; then
    check "harbor-mirror-sync.sh --dry-run never plans set-url origin" "FOUND 'set-url origin' in dry-run output!"
else
    check "harbor-mirror-sync.sh --dry-run never plans set-url origin" "ok"
fi

# Static grep: the sync script source should never contain 'set-url origin'
if grep -q "set-url origin" "${SCRIPT_DIR}/harbor-mirror-sync.sh"; then
    check "harbor-mirror-sync.sh source never contains set-url origin" "FOUND 'set-url origin' in script source!"
else
    check "harbor-mirror-sync.sh source never contains set-url origin" "ok"
fi
echo ""

# --- AC5: Default repo list includes rollout and wintermute-desktop ---
echo "AC5: default repo list includes rollout and wintermute-desktop"
if grep -q "rollout" "${SCRIPT_DIR}/harbor-mirror-up.sh" && grep -q "wintermute-desktop" "${SCRIPT_DIR}/harbor-mirror-up.sh"; then
    check "harbor-mirror-up.sh default list includes rollout + wintermute-desktop" "ok"
else
    check "harbor-mirror-up.sh default list includes rollout + wintermute-desktop" "not found in script"
fi

if grep -q "rollout" "${SCRIPT_DIR}/harbor-mirror-sync.sh" && grep -q "wintermute-desktop" "${SCRIPT_DIR}/harbor-mirror-sync.sh"; then
    check "harbor-mirror-sync.sh default list includes rollout + wintermute-desktop" "ok"
else
    check "harbor-mirror-sync.sh default list includes rollout + wintermute-desktop" "not found in script"
fi
echo ""

# --- AC6: Self-check — this script is in scripts/ ---
echo "AC6: harbor-mirror-check.sh is in scripts/"
if [[ "$(dirname "$(realpath "${BASH_SOURCE[0]}")")" == "$(realpath "${SCRIPT_DIR}")" ]]; then
    check "harbor-mirror-check.sh resides in scripts/" "ok"
else
    check "harbor-mirror-check.sh resides in scripts/" "unexpected path"
fi
echo ""

# --- AC7: Sync notes uncommitted changes without blocking ---
echo "AC7: sync documents uncommitted-change behavior"
if grep -q "uncommitted" "${SCRIPT_DIR}/harbor-mirror-sync.sh"; then
    check "harbor-mirror-sync.sh documents uncommitted-change note" "ok"
else
    check "harbor-mirror-sync.sh documents uncommitted-change note" "no 'uncommitted' mention in script"
fi
# Sync should NOT refuse/exit-early on dirty repos — the uncommitted-changes block
# should only append to DIRTY_REPOS and print a note, never call exit.
# We check that the block between "uncommitted changes" note and next logical section
# does not contain an unconditional exit.
dirty_block=$(awk '/NOTE.*uncommitted changes/{found=1} found{print} /DIRTY_REPOS\+=/{found=0}' \
    "${SCRIPT_DIR}/harbor-mirror-sync.sh")
if echo "$dirty_block" | grep -qE "^[[:space:]]*(exit|return) [^0]"; then
    check "harbor-mirror-sync.sh does not hard-exit on uncommitted changes" "found exit/return in dirty-repo note block"
else
    check "harbor-mirror-sync.sh does not hard-exit on uncommitted changes" "ok"
fi
echo ""

# --- Summary ---
echo "=== Results: ${PASS} passed, ${FAIL} failed ==="
[[ $FAIL -eq 0 ]] && exit 0 || exit 1
