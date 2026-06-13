#!/usr/bin/env bash
# harbor-mirror-up.sh — stand up a bare git mirror on the permanent hub
#
# Usage:
#   harbor-mirror-up.sh [--dry-run] [--repo-list FILE]
#
# Reads hub connection details from hub.json (produced by harbor-hub or wm-burst hub up).
# Creates a git user + ~/mirror/ on the hub, seeds bare repos, and enables git-daemon
# on the private interface for read-only clone access plus SSH push for the jsy key.
#
# Default repo list: rollout, wintermute-desktop, plus all ~/wintermute/* repos that
# have a remote.  Override with --repo-list pointing at a file of repo paths (one per line).
#
# Idempotency: if a bare repo already exists it is fetched, not re-cloned.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HUB_JSON="${HUB_JSON:-${SCRIPT_DIR}/../hub.json}"
DRY_RUN=0
REPO_LIST_FILE=""

usage() {
    echo "Usage: $(basename "$0") [--dry-run] [--repo-list FILE]"
    echo ""
    echo "  --dry-run       Print planned actions without touching the hub"
    echo "  --repo-list F   Path to a file listing repos to mirror (one per line)"
    echo ""
    echo "Requires hub.json in the repo root (or set HUB_JSON env var)."
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

# Parse hub connection info
HUB_HOST=$(jq -r '.host // empty' "$HUB_JSON")
HUB_USER=$(jq -r '.user // "root"' "$HUB_JSON")
HUB_PORT=$(jq -r '.port // 22' "$HUB_JSON")
HUB_KEY=$(jq -r '.ssh_key // empty' "$HUB_JSON")
HUB_PRIVATE_IP=$(jq -r '.private_ip // .host' "$HUB_JSON")

if [[ -z "$HUB_HOST" ]]; then
    echo "ERROR: hub.json missing 'host' field" >&2
    exit 1
fi

SSH_OPTS=(-o StrictHostKeyChecking=accept-new -p "$HUB_PORT")
[[ -n "$HUB_KEY" ]] && SSH_OPTS+=(-i "$HUB_KEY")

ssh_hub() {
    ssh "${SSH_OPTS[@]}" "${HUB_USER}@${HUB_HOST}" "$@"
}

# Build repo list
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
                # Skip already-added
                local already=0
                for r in "${repos[@]:-}"; do
                    [[ "$r" == "$repo_dir" ]] && already=1 && break
                done
                [[ $already -eq 1 ]] && continue
                # Only include if it has at least one remote
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

echo "=== harbor-mirror-up ==="
echo "Hub:  ${HUB_USER}@${HUB_HOST}:${HUB_PORT}"
echo "Mirror URL form: hub:mirror/<repo>.git"
echo "Repos to seed (${#REPOS[@]}):"
for repo in "${REPOS[@]+"${REPOS[@]}"}"; do
    name="$(basename "$repo")"
    echo "  $repo  →  hub:mirror/${name}.git"
done
echo ""

if [[ $DRY_RUN -eq 1 ]]; then
    echo "[dry-run] Planned actions:"
    echo "  1. Ensure git user + ~/mirror/ directory on hub"
    echo "  2. Enable git-daemon on private interface (${HUB_PRIVATE_IP}) for read-only clone"
    echo "  3. Install systemd unit: git-daemon.service"
    echo ""
    for repo in "${REPOS[@]+"${REPOS[@]}"}"; do
        name="$(basename "$repo")"
        echo "  [dry-run] Seed: hub:mirror/${name}.git"
        echo "    - If exists: git fetch --all --prune (FETCH, no re-create)"
        echo "    - If absent: git clone --mirror <source_url>"
    done
    echo ""
    echo "[dry-run] No changes made."
    exit 0
fi

# Live execution
echo "Preparing hub environment..."

ssh_hub bash -s << 'ENDSSH'
set -euo pipefail

# Create git user if absent
if ! id git &>/dev/null; then
    useradd -m -s /usr/bin/git-shell git
    echo "Created git user"
else
    echo "git user exists"
fi

# Ensure mirror directory
install -d -o git -g git /home/git/mirror
echo "mirror/ ready"

# Install authorized key for jsy (key should be in ~root/.ssh/authorized_keys or passed)
# We reuse the hub's jsy key if present
JSY_KEY=""
for f in /root/.ssh/authorized_keys /home/jsy/.ssh/authorized_keys; do
    [[ -f "$f" ]] && JSY_KEY="$(cat "$f")" && break
done
if [[ -n "$JSY_KEY" ]]; then
    install -d -m 700 -o git /home/git/.ssh
    echo "$JSY_KEY" > /home/git/.ssh/authorized_keys
    chmod 600 /home/git/.ssh/authorized_keys
    chown git:git /home/git/.ssh/authorized_keys
    echo "authorized_keys installed for git user"
fi

# Ensure git-shell commands directory
install -d -o git -g git /home/git/git-shell-commands

ENDSSH

# Seed repos
for repo in "${REPOS[@]+"${REPOS[@]}"}"; do
    name="$(basename "$repo")"
    source_remote=""

    # Determine source URL (prefer origin, fall back to first remote)
    if git -C "$repo" remote 2>/dev/null | grep -q '^origin$'; then
        source_remote=$(git -C "$repo" remote get-url origin 2>/dev/null || true)
    elif git -C "$repo" remote 2>/dev/null | grep -q .; then
        first=$(git -C "$repo" remote | head -1)
        source_remote=$(git -C "$repo" remote get-url "$first" 2>/dev/null || true)
    fi

    echo "Seeding ${name}.git on hub..."
    ssh_hub bash -s "$name" "$source_remote" << 'ENDSSH'
set -euo pipefail
name="$1"
source_remote="$2"
mirror_path="/home/git/mirror/${name}.git"

if [[ -d "$mirror_path" ]]; then
    echo "  ${name}.git: EXISTS — fetching (no re-create)"
    git -C "$mirror_path" fetch --all --prune || echo "  WARNING: fetch failed for ${name}.git"
else
    if [[ -z "$source_remote" ]]; then
        echo "  ${name}.git: ABSENT + no source remote — creating empty bare repo"
        git -C /home/git/mirror init --bare "${name}.git"
    else
        echo "  ${name}.git: ABSENT — cloning mirror from ${source_remote}"
        git clone --mirror "$source_remote" "$mirror_path" || {
            echo "  WARNING: clone failed for ${name}.git, creating empty bare repo"
            git -C /home/git/mirror init --bare "${name}.git"
        }
    fi
    chown -R git:git "$mirror_path"
fi
ENDSSH
done

# Install git-daemon systemd unit
echo "Installing git-daemon.service..."
ssh_hub bash -s "$HUB_PRIVATE_IP" << 'ENDSSH'
set -euo pipefail
private_ip="$1"
unit_path="/etc/systemd/system/git-daemon.service"

if [[ ! -f "$unit_path" ]]; then
cat > "$unit_path" << UNIT
[Unit]
Description=Git Daemon (read-only, private interface)
After=network.target

[Service]
ExecStart=/usr/bin/git daemon \
    --listen=${private_ip} \
    --port=9418 \
    --base-path=/home/git/mirror \
    --export-all \
    --informative-errors \
    --verbose
Restart=on-failure
User=git

[Install]
WantedBy=multi-user.target
UNIT
    systemctl daemon-reload
    systemctl enable --now git-daemon.service
    echo "git-daemon.service installed and started"
else
    echo "git-daemon.service already installed"
    systemctl is-active git-daemon.service >/dev/null 2>&1 || systemctl start git-daemon.service
fi
ENDSSH

echo ""
echo "=== harbor-mirror-up complete ==="
echo "Clone URL:  git://${HUB_PRIVATE_IP}/<repo>.git"
echo "Push URL:   git@${HUB_HOST}:mirror/<repo>.git"
echo ""
echo "Next: run harbor-mirror-sync.sh on the laptop to add the 'hub' remote."
