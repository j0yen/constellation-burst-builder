# CLAUDE_SELF.md — wm-burst

Agent-maintained notes for future Claude instances working on this crate.

## Changelog

- **v0.2.0** (2026-06-05): Implemented `HcloudPodProvider` (real Hetzner Cloud x86
  pod lifecycle via hcloud REST API), `wm-burst build --burst` on-demand flow,
  `wm-burst provision --snapshot`, and `HcloudPodConfig` with x86 defaults + Hetzner
  Object Storage sccache backend. All 8 ACs of PRD-constellation-burst-builder-hcloud
  verified; 43 tests green.
- **v0.1.0**: Initial ship — generic pod lifecycle, SSH+sccache build, mock provider,
  cost log, monthly budget cap.

## Architecture notes

- `src/provider.rs` owns the `PodProvider` trait + `HcloudPodProvider` + `MockPodProvider`
  + the `make_provider` factory. The token is `Box<str>`, excluded from `Debug`.
- `src/config.rs` has two pod config structs: `PodConfig` (legacy generic) and
  `HcloudPodConfig` (new, used by `HcloudPodProvider`). Config `[hcloud]` section maps
  to `HcloudPodConfig`.
- `src/commands/build.rs` handles `--burst` flag: reads `hcloud` config, calls
  `provider::make_provider`, wraps the lifecycle with cost logging + budget check.
- The `real-hardware` feature gates actual SSH/sccache calls; tests run with the
  mock provider and don't require live infra.
- `hcloud_post`/`hcloud_delete` use `curl` (no reqwest dep) — a future iter can swap.
- `destroy_pod` is idempotent: HTTP 404 is treated as success.
