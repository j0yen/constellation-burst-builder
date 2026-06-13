# wm-burst

`wm-burst` is a small Rust CLI that points this laptop's `cargo` at **one
always-on, cheap, dedicated cloud box** (a Hetzner server-auction Ryzen 9 9950X,
32 threads / 64–128 GB, ~€50–70/mo) and a **shared sccache object cache**, so cold
builds, autobuilder determinism runs, and wake-word/ML CPU jobs stop pinning the
local cores. Unlike `constellation-cloud-build`, it requires **no NATS mesh, no
dispatch coordinator, no capability registry** — it is `ssh` + `sccache` + a config
file. It is the thing you can stand up *today* and the on-ramp the full fleet
graduates from later. Crucially it refuses to run a remote build when the remote
toolchain doesn't match `rust-toolchain.toml`, so the 1.85/1.88 drift that has
bitten cold builds before can't silently corrupt a burst.

## Subcommands

- **`wm-burst init`** — writes/edits `~/.config/wm-burst/config.toml`
- **`wm-burst provision`** — idempotent Ansible playbook to ready a fresh builder host
- **`wm-burst doctor`** — checks reachability, toolchain match, cache writability + hit rate
- **`wm-burst build [-- <cargo args>]`** — builds with `RUSTC_WRAPPER=sccache` on the remote
- **`wm-burst exec -- <cmd>`** — runs any CPU job on the remote and streams output
- **`wm-burst pod up|down`** — manages ephemeral burst pods with budget enforcement
- **`wm-burst status`** — shows load, cache hit rate, month-to-date spend vs cap, last N jobs

## Acceptance criteria

1. `wm-burst init` writes a valid `config.toml` and `--show` round-trips it; missing/invalid config produces a clear error.
2. `wm-burst provision` converges a fresh host to a ready builder (pinned 1.85 + 1.88 toolchains, sccache).
3. `wm-burst doctor` hard-fails on toolchain mismatch with exact local-vs-remote diff; proven by tests.
4. `wm-burst build` compiles with `RUSTC_WRAPPER=sccache` and reports where it ran + cache stats.
5. `wm-burst exec` runs a non-cargo job on the remote box and propagates exit status.
6. Pod tier creates, runs, tears down ephemeral builder; budget cap blocks new pods on overrun.
7. `wm-burst status` reports load, cache hit rate, spend vs cap, last N jobs.
8. CLI hygiene: `sigpipe::reset()` first line of `main()`; `--help`/`--version` work; MSRV 1.85, no let-chains.

## Install

```sh
cargo install --path .
# or grab from the latest release:
install -Dm755 target/release/wm-burst ~/.local/bin/wm-burst
```

## Quick start

```sh
wm-burst init              # write ~/.config/wm-burst/config.toml
wm-burst init --show       # round-trip and display current config
wm-burst doctor            # verify remote reachability + toolchain match
wm-burst build             # cargo build routed through sccache on the remote
wm-burst status            # load / cache / spend summary
```

## Git mirror (in-DC clone + WIP backup)

The hub holds a **bare git mirror** of all `~/wintermute/` repos.  Burst pods clone
from this mirror over the private Hetzner network instead of hitting `github.com` on
every build — faster and removes a GitHub-availability dependency from the build path.

The same mirror serves as a **WIP backup remote**.  Any committed work that hasn't
reached GitHub (self-review regularly surfaces 20+ "dirty" repos) can be pushed here
with a single command.  This is **not a GitHub push** — safe for branches that are
not yet ready to be public.

### Setup

```sh
# On the hub — one-time setup (idempotent)
scripts/harbor-mirror-up.sh --dry-run   # preview planned seeds
scripts/harbor-mirror-up.sh             # create git user + bare repos + git-daemon

# On the laptop — add hub remote + mirror push
scripts/harbor-mirror-sync.sh --dry-run # preview: shows 'git remote add hub …', never set-url origin
scripts/harbor-mirror-sync.sh           # back up all committed refs to hub
```

Repos with uncommitted changes are **noted** but not blocked — committed refs are
still mirrored.  The dry-run output calls out any dirty repos so you can commit
before syncing if you want the latest included.

Run `scripts/harbor-mirror-check.sh` to validate the scripts offline (no hub needed).

## Part of the wintermute constellation fleet

`wm-burst` is the mesh-free on-ramp beneath
[constellation-cloud-build](https://github.com/j0yen/constellation-cloud-build).
It requires no NATS, no JetStream, no capability registry — just an ssh alias and
an sccache bucket. When the full fleet mesh is live, burst jobs graduate to the
dispatcher automatically.
