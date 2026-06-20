# wm-burst

Point this laptop's `cargo` at a cheap on-demand cloud box and a shared sccache, so heavy compiles and CPU jobs stop pinning local cores — with no fleet mesh to stand up first.

## Why it exists

A cold Rust build pins every core for minutes; an autobuilder determinism run or a wake-word/ML job does the same. The full fleet has a proper dispatch path for that, but it needs a NATS mesh, a coordinator, and a capability registry — infrastructure you can't stand up in an afternoon. wm-burst is the version you can: `ssh` + `sccache` + one config file. It spins up a pod, builds there, tears it down, and bills you for the minutes used. The work moves off the laptop today; it graduates to the mesh later.

The guardrail that makes remote builds trustworthy: wm-burst refuses to run when the remote toolchain doesn't match `rust-toolchain.toml`. Cross-machine toolchain drift can't silently corrupt a burst.

## Install

Builds with cargo; not published to crates.io.

```sh
cargo install --path .
# or copy the release binary
install -Dm755 target/release/wm-burst ~/.local/bin/wm-burst
```

## Quickstart

```sh
wm-burst init          # write ~/.config/wm-burst/config.toml
wm-burst init --show    # round-trip and display current config
wm-burst doctor         # verify remote reachability, toolchain match, sccache
wm-burst build          # cargo build routed through sccache on the remote
wm-burst status         # load / cache hit rate / spend-vs-cap / last N jobs
```

## Subcommands

- **`init`** — writes or edits `~/.config/wm-burst/config.toml`.
- **`provision`** — generates an Ansible playbook (and runs it, unless `--generate-only` / `--check`) to converge a fresh host to builder-ready: pinned toolchain, sccache, shared-cache creds.
- **`doctor`** — checks reachability, toolchain match, and cache. Hard-fails on a toolchain mismatch with the exact local-vs-remote diff.
- **`build [-- <cargo args>]`** — builds on the remote with `RUSTC_WRAPPER=sccache`, streams output locally, and reports where it ran plus cache stats. `--burst` runs the on-demand pod lifecycle: create, build, tear down.
- **`exec -- <cmd>`** — runs any CPU job on the remote and propagates its exit status.
- **`pod up|down`** — manages ephemeral burst pods. A configurable monthly budget cap blocks new pods on overrun, so there's no surprise bill.
- **`status`** — load averages, cache hit rate, month-to-date spend against the cap, and the last N jobs.

## The box

The pod provider is Hetzner Cloud by default — a CCX23 at roughly €0.03/hr, x86 AMD to match the laptop's `x86_64` target so offloaded binaries run locally. It's on-demand, not always-on: spin it up for the build, destroy it after. `init` sets the provider, server type, location, image, and sccache bucket; `provider` is pluggable (`hcloud`, with `runpod` / `vast` config slots present). `REMOTE-SETUP.md` walks the one-time manual Hetzner setup (API token, SSH key, server creation) for use before the hcloud provider is wired end to end.

Live SSH / sccache / provider calls are gated behind the `real-hardware` cargo feature, off by default so `cargo test` stays green on a host with no infrastructure.

## In-DC git mirror

The hub can hold a bare git mirror of the `~/wintermute/` repos. Burst pods clone from it over the private network instead of hitting github.com on every build — faster, and it removes a GitHub-availability dependency from the build path. The same mirror doubles as a WIP backup remote: committed work that isn't ready for GitHub can be pushed there with one command. This is not a GitHub push — safe for branches that aren't public yet.

```sh
# On the hub (one-time, idempotent)
scripts/harbor-mirror-up.sh --dry-run   # preview planned seeds
scripts/harbor-mirror-up.sh             # create git user + bare repos + git-daemon

# On the laptop
scripts/harbor-mirror-sync.sh --dry-run # preview: adds a 'hub' remote, never rewrites origin
scripts/harbor-mirror-sync.sh           # back up all committed refs to the hub
```

Repos with uncommitted changes are noted, not blocked — committed refs still mirror, and the dry-run output flags the dirty ones. `scripts/harbor-mirror-check.sh` validates the scripts offline, no hub needed.

## Build and test

```sh
cargo build --release
cargo test
```

## Where it fits

wm-burst is the mesh-free on-ramp for the wintermute fleet's build offload: no NATS, no JetStream, no capability registry — an ssh alias and an sccache bucket. When the full fleet mesh is live, burst jobs graduate to the dispatcher. It sits alongside [constellation](https://github.com/j0yen/constellation), which provisions the fleet's nodes and the always-on hub the mirror lives on.

## Status

Early. The config, toolchain-match guard, cost log, and command surface are in place; the live `real-hardware` path is feature-gated and the hcloud provider is still being wired end to end (hence the manual `REMOTE-SETUP.md`).
