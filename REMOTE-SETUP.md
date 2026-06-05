# Hetzner Cloud build box — manual setup (until `wm-burst` hcloud provider ships)

Goal: a cheap, on-demand **x86 AMD** Rust build box that matches this laptop
(x86_64, rustc 1.85.0), so offloaded builds produce runnable binaries and share an
sccache cache. ~€0.03/hr — destroy it when you're not building.

## 1. Console: project + API token + SSH key
1. https://console.hetzner.cloud → create a **Project** (e.g. `wintermute-build`).
2. Project → **Security → API Tokens** → Generate. Scope: **Read & Write**.
   Copy it once (shown only once). Store as an env var locally, never in a file:
   ```sh
   # add to ~/.config/wm-burst/.env or a password manager — NOT committed
   export HCLOUD_TOKEN='...'
   ```
3. Project → **Security → SSH Keys** → add your laptop public key
   (`cat ~/.ssh/id_ed25519.pub`). Generate one if needed:
   `ssh-keygen -t ed25519 -C wintermute-build`.

## 2. Create the server
Console → **Add Server**:
- **Location**: an EU region (Falkenstein/Nuremberg/Helsinki) — cheapest; latency
  doesn't matter for batch builds. (US Ashburn if you ever want lower RTT.)
- **Image**: **Ubuntu 24.04**.
- **Type**: **CCX23** (Dedicated vCPU, 4 AMD cores / 16 GB) — predictable build times.
  CPX41 (8 shared vCPU) is cheaper but variable.
- **SSH key**: select the one from step 1.
- Create. Note the public IP.

(CLI alternative once `hcloud` is installed: `hcloud server create --name builder1
--type ccx23 --image ubuntu-24.04 --location fsn1 --ssh-key wintermute-build`.)

## 3. Provision the box (toolchain + sccache + deps)
SSH in (`ssh root@<IP>`) and run:
```sh
apt-get update && apt-get install -y build-essential pkg-config libssl-dev curl git
# rustup + pin 1.85.0 (matches wm-burst); add 1.88.0 for brain crates
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
  --default-toolchain 1.85.0 --profile minimal -c rustfmt,clippy
. "$HOME/.cargo/env"
rustup toolchain install 1.88.0 --profile minimal
cargo install sccache --locked
```
Verify it matches the laptop:
```sh
rustc -vV | grep -E 'release|host'   # expect 1.85.0 + x86_64-unknown-linux-gnu
```

## 4. Shared sccache cache (cross-run hits)
Because the box is ephemeral, point sccache at off-box object storage so a destroyed
server doesn't lose the cache. Create a **Hetzner Object Storage** bucket (console →
Object Storage), then on the builder:
```sh
export SCCACHE_BUCKET=wintermute-sccache
export SCCACHE_ENDPOINT=<your-hetzner-s3-endpoint>
export AWS_ACCESS_KEY_ID=...   AWS_SECRET_ACCESS_KEY=...
export RUSTC_WRAPPER=sccache
```
(Simpler interim option: skip object storage and just rely on the box's local
sccache while it's alive — you lose cache across destroy/recreate but pay nothing
extra. Fine until builds are frequent.)

## 5. Use it
- **Remote build (simplest):** rsync the crate up and build over SSH:
  ```sh
  rsync -az --delete ~/wintermute/<crate>/ root@<IP>:/root/build/<crate>/
  ssh root@<IP> 'cd /root/build/<crate> && RUSTC_WRAPPER=sccache ~/.cargo/bin/cargo build --release'
  rsync -az root@<IP>:/root/build/<crate>/target/release/<bin> ~/.local/bin/
  ```
- **Distributed (later):** sccache-dist so local `cargo` offloads `rustc` invocations
  transparently — that's the end-state the constellation-cloud-build PRD covers.

## 6. Snapshot, then destroy when idle
Once provisioned, take a **snapshot** (console → server → Snapshots) so future boxes
boot ready in ~30s. Then **delete the server** when you're done building — you stop
paying immediately; recreate from the snapshot next time.

## 7. When `wm-burst` hcloud provider ships
PRD-constellation-burst-builder-hcloud automates 2–6:
```sh
wm-burst init --remote-host <IP> --sccache-endpoint <s3> --sccache-bucket wintermute-sccache
wm-burst provision --snapshot          # bakes the snapshot
wm-burst doctor                        # checks 1.85.0 + x86_64 + cache writable
wm-burst build --burst -- --release    # boots a pod, builds, tears down, logs cost
```
Until then, steps 1–6 are the manual equivalent.
