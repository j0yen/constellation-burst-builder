# Changelog

## v0.3.0 — 2026-06-13

Add permanent hub lifecycle to wm-burst: hub up|down|status with HubConfig, HubState (hub.json), get_server trait method, idempotent up, guarded down, and HUB-UP/HUB-DOWN cost.log entries

## v0.2.0 — 2026-06-05

Implements the real Hetzner Cloud x86 pod provider (`HcloudPodProvider`) behind the
existing `PodProvider` trait abstraction. Selected when `[hcloud] provider = "hcloud"`
is present in config. Key additions:

- `HcloudPodProvider`: `create_pod`/`run_job`/`destroy_pod` via hcloud REST API v1
  using `curl`; API token read-once from `$HCLOUD_TOKEN`, never logged or rendered.
- `HcloudPodConfig` struct with sane defaults (`ccx23`/`fsn1`/`x86_64-unknown-linux-gnu`
  / Hetzner Object Storage sccache endpoint).
- `wm-burst build --burst`: on-demand flow — create hcloud pod, run build with
  `RUSTC_WRAPPER=sccache` against persistent shared cache, report cache hit/miss +
  cost, destroy pod.
- `destroy_pod` is idempotent: 404 responses treated as success (no orphaned servers).
- `wm-burst provision --snapshot`: provisions a server, bakes toolchain + sccache,
  creates a reusable hcloud snapshot for ~30s boot on subsequent `--burst` runs.
- Token redaction enforced in `Debug`, cost log, and rendered config.
- Budget cap regression-free: pod creation blocked when monthly spend exceeds cap.
- All 8 ACs verified: 43 unit tests + integration tests green, MSRV 1.85, SIGPIPE safe.

## v0.1.0 — initial

Shipped `wm-burst` with generic pod lifecycle, SSH+sccache build path, mock provider,
cost log, and monthly budget guardrail.
