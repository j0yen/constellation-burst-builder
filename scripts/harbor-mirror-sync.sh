#!/usr/bin/env bash
# harbor-mirror-sync.sh — sync local repos to the bare hub mirror
#
# Usage:
#   harbor-mirror-sync.sh [--dry-run] [--repo-list FILE]
#
# For each repo in the list:
#   - Adds a 'hub' remote pointing at hub:mirror/<repo>.git  (never touches origin)
#   - Runs 'git push --mirror hub' to back up all committed refs
#   - Notes any repo with uncommitted changes (does NOT push working tree)
#
# This is explicitly NOT a GitHub push — safe for WIP that isn't ready to be public.
#
# Requires hub.json in the repo root (or set HUB_JSON env var).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HUB_JSON="${HUB_JSON:-${SCRIPT_DIR}/../hub.json}"
DRY_RUN=0
REPO_LIST_FILE=""

usage() {
    echo "Usage: $(basename "$0") [--dry-run] [--repo-list FILE]"
    echo ""
    echo "  --dry-run       Print planned remote-add + push actions without executing"
    echo "  --repo-list F   Path to a file listing repos to sync (one per line)"
    echo ""
    echo "This is NOT a GitHub push.  It mirrors committed refs to the hub only."
    echo "Repos with uncommitted changes are noted but still synced (committed refs only)."
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run)   DRY_RUN=1 ;;
        --repo-list) REPO_LIST_FILE="$2"; shift ;;
        -h|--help)   usage ;;
        *) echo "Unknown argument: $1" >&2; usage ;;
    esac
    shift
done

# Require hub.json
if [[ ! -f "$HUB_JSON" ]]; then
    echo "ERROR: no hub — run \`wm-burst hub up\` first (expected: $HUB_JSON)" >&2
    exit 1
fi

HUB_HOST=$(jq -r '.host // empty' "$HUB_JSON")
HUB_PORT=$(jq -r '.port // 22' "$HUB_JSON")

if [[ -z "$HUB_HOST" ]]; then
    echo "ERROR: hub.json missing 'host' field" >&2
    exit 1
fi

# Build repo list (same logic as harbor-mirror-up.sh)
build_repo_list() {
    local repos=()

    if [[ -n "$REPO_LIST_FILE" ]]; then
        while IFS= read -r line; do
            [[ -z "$line" || "$line" =~ ^# ]] && continue
            repos+=("$line")
        done < "$REPO_LIST_FILE"
    else
        # Always include the named unpushed repos from self-review
        for named in rollout wintermute-desktop; do
            local p="$HOME/wintermute/$named"
            if [[ -d "$p/.git" || -d "$p" ]]; then
                repos+=("$p")
            fi
        done

        # Add all ~/wintermute/* repos that have a remote
        if [[ -d "$HOME/wintermute" ]]; then
            while IFS= read -r -d '' repo_path; do
                local repo_dir="${repo_path%/.git}"
                local already=0
                for r in "${repos[@]:-}"; do
                    [[ "$r" == "$repo_dir" ]] && already=1 && break
                done
                [[ $already -eq 1 ]] && continue
                if git -C "$repo_dir" remote 2>/dev/null | grep -q .; then
                    repos+=("$repo_dir")
                fi
            done < <(find "$HOME/wintermute" -maxdepth 2 -name ".git" -type d -print0 2>/dev/null)
        fi
    fi

    printf '%s\n' "${repos[@]+"${repos[@]}"}"
}

REPOS=()
while IFS= read -r r; do
    [[ -n "$r" ]] && REPOS+=("$r")
done < <(build_repo_list)

echo "=== harbor-mirror-sync ==="
echo "Hub: ${HUB_HOST}  (NOT pushing to GitHub)"
echo ""

DIRTY_REPOS=()
SYNCED=0
FAILED=0

for repo in "${REPOS[@]+"${REPOS[@]}"}"; do
    if [[ ! -d "$repo/.git" && ! -d "$repo" ]]; then
        echo "  SKIP $repo (not found)"
        continue
    fi

    name="$(basename "$repo")"
    hub_url="git@${HUB_HOST}:mirror/${name}.git"

    # Check for uncommitted changes (AC7: note, don't block)
    if ! git -C "$repo" diff --quiet 2>/dev/null || ! git -C "$repo" diff --cached --quiet 2>/dev/null; then
        echo "  NOTE: $repo has uncommitted changes — only committed refs will be mirrored"
        echo "        Commit your work first if you want the latest changes included."
        DIRTY_REPOS+=("$repo")
    fi

    # Check / add hub remote — never touch origin
    existing_hub=$(git -C "$repo" remote get-url hub 2>/dev/null || true)
    if [[ -z "$existing_hub" ]]; then
        if [[ $DRY_RUN -eq 1 ]]; then
            echo "  [dry-run] git remote add hub ${hub_url}  (origin untouched)"
        else
            git -C "$repo" remote add hub "$hub_url"
            echo "  Added remote 'hub' → ${hub_url}"
        fi
    elif [[ "$existing_hub" != "$hub_url" ]]; then
        if [[ $DRY_RUN -eq 1 ]]; then
            echo "  [dry-run] git remote set-url hub ${hub_url}  (was: ${existing_hub})"
        else
            git -C "$repo" remote set-url hub "$hub_url"
            echo "  Updated remote 'hub' → ${hub_url}"
        fi
    else
        echo "  remote 'hub' already correct for ${name}"
    fi

    # Push
    if [[ $DRY_RUN -eq 1 ]]; then
        echo "  [dry-run] git push --mirror hub  (${name} → hub:mirror/${name}.git)"
    else
        echo -n "  Pushing ${name} → hub..."
        if git -C "$repo" push --mirror hub 2>&1 | tail -1; then
            echo "  OK"
            ((SYNCED++)) || true
        else
            echo "  FAILED"
            ((FAILED++)) || true
        fi
    fi
    echo ""
done

if [[ $DRY_RUN -eq 1 ]]; then
    echo "[dry-run] No changes made."
    if [[ ${#DIRTY_REPOS[@]} -gt 0 ]]; then
        echo ""
        echo "Repos with uncommitted changes (would be noted at sync time):"
        for dr in "${DIRTY_REPOS[@]}"; do echo "  $dr"; done
    fi
    exit 0
fi

echo "=== harbor-mirror-sync complete ==="
echo "  Synced: ${SYNCED}  Failed: ${FAILED}"
if [[ ${#DIRTY_REPOS[@]} -gt 0 ]]; then
    echo "  Repos with uncommitted changes (committed refs were mirrored):"
    for dr in "${DIRTY_REPOS[@]}"; do echo "    $dr"; done
fi
[[ $FAILED -gt 0 ]] && exit 1 || exit 0
