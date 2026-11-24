# Development

## Setup

Arch/CachyOS packages: `rust`, `clang`, `cmake`, `pkgconf`, `pipewire`, `rnnoise`.
Optional for scripts: `python`, `python-pillow` (screenshots), `python-fonttools` (icons).

```sh
scripts/build-hush.sh    # once: Hush runtime into third_party/hush (Hush tests skip without it)
scripts/fetch-ysfx.sh    # once: ysfx sources into third_party/ysfx, built by build.rs (needs cmake; without it JSFX is disabled)
cargo build
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all
```

macOS: Rust, Xcode command line tools, `cmake`, the Otozane Microphone driver
installed (`scripts/build-driver.sh`, then Install in the app) and, for
`scripts/e2e-macos.sh`, `ffmpeg`. RNNoise is not in Homebrew, build it once:

```sh
scripts/build-rnnoise.sh   # static librnnoise.a into third_party/rnnoise, picked up by build.rs
cargo build
cargo test --workspace
```

## Running against a separate configuration

Use a throwaway config directory so development never touches your real profiles:

```sh
XDG_CONFIG_HOME=/tmp/otozane-dev ./target/debug/otozaned
XDG_CONFIG_HOME=/tmp/otozane-dev ./target/debug/otozane
```

Only one service can run per user (the socket is `$XDG_RUNTIME_DIR/otozane.sock`).
To run a second, test service next to a real one, set `OTOZANE_SOCKET` (socket
path, also used by the UI and `scripts/ipc.py`) and `OTOZANE_NODE_PREFIX`
(prefix for its PipeWire node names). `scripts/e2e.sh` does both.

For a stop-on-close check on Linux, use a private PipeWire and WirePlumber
session and an isolated service socket. Send `{"cmd":"set_stop_on_close","enabled":true}`,
then `{"cmd":"shutdown"}` and `{"cmd":"start"}` with `scripts/ipc.py`.
Compare the microphone's `object.serial` in `pw-dump` before shutdown, while
stopped, and after start. The capture and output streams disappear while
stopped. A single `pw-record` should continue to record silence until processing
resumes. On macOS use one `ffmpeg -f avfoundation -i ":Otozane Microphone"`
recording through an IPC shutdown and a new service launch.

## macOS app updates

The macOS side of the in-app updater lives in
`crates/otozane-ui/src/update_macos.rs`. The generic updater downloads the
release asset and checks it against `SHA256SUMS`; this module trusts that
result and handles only the app bundle:

- `detect()` finds the running `Otozane.app` and its version. It fails with a
  message for the Updates section when Otozane runs outside a bundle, from
  Gatekeeper's App Translocation copy, or from a folder this user cannot write.
- `asset_name(version)` returns `otozane-vX.Y.Z-macos-arm64.dmg` and refuses
  Intel Macs.
- `stage(dmg, bundle, version)` mounts the DMG read-only, requires exactly one
  `.app`, checks the bundle id, version, Apple Silicon executable and code
  signature, and copies the newer bundle to a private directory beside the
  installed app.
- `Staged::restart()` prepares a shell helper, stops the audio service and
  starts the helper. The app exits after the helper starts. The helper waits
  up to a minute for that exit, swaps the bundles on the same filesystem,
  starts the service through launchd or as a detached process, and opens the
  new app. If starting the service or opening the app fails, it stops the new
  service before restoring the previous bundle.
- `driver_state()` compares the virtual microphone driver in the bundle with
  the installed one. `needs_reinstall` tells Settings → Virtual microphone to
  offer a reinstall after the update.

The downloaded DMG is removed after staging. The helper writes
`last-update.log` under `~/Library/Application Support/Otozane/updates` (or
`$XDG_CONFIG_HOME/otozane/updates`). If recovery fails, the helper logs the
location of the backup beside the installed app. The macOS update tests use a
signed fixture app and DMG in a temporary folder with stand-ins for
`launchctl` and `open`.

The app is signed ad-hoc and not notarized. Gatekeeper cannot assess that
signature, so after the checksum, bundle id, version and signature checks the
staged app gets `xattr -dr com.apple.quarantine`. Without it a downloaded
update would not open. Trust in the update therefore rests on the HTTPS
download and the release's `SHA256SUMS`, not on the signature.

### Clean Mac checklist

Automatic tests cover the bundle checks and the swap/rollback helper. These
need a real machine before the update can be called working:

- Update from the previous release to a test release with Otozane in
  `/Applications`, as an admin user and as a non-admin user. The app opens
  again, shows the new version, `otozaned` is running and Otozane Microphone
  records.
- Start from a freshly downloaded DMG on a Mac that has never run Otozane.
  The updated app launches without the right-click → Open step.
- Install the driver from the old release, update to a release with a changed
  driver, and check that Settings → Virtual microphone offers a reinstall and
  that the microphone works afterwards.
- Try a corrupt or wrong-version DMG and confirm that the error appears and
  the installed app still opens. Simulate a failure to open the replacement
  and confirm that the helper restores the old app and service.
- Open Otozane from `Downloads` (App Translocation) and from a read-only
  folder: no Update button, only the release-page link.
- With login start on, check that the launchd agent starts the service from
  the new bundle. With it off, the new app still opens and Start service works.
- Let the app exit after confirmation and confirm that the helper replaces
  the bundle. To test the timeout, keep the old process alive for one minute
  and confirm that the helper leaves the bundle in place.

## Scripts

| Script | Purpose |
|--------|---------|
| `scripts/e2e.sh [input.wav] [chain-json]` | End-to-end audio check. Creates a temporary test source with `pw-loopback`, starts `otozaned` with an isolated config, sets the chain, plays a file into the test source and records **Otozane Microphone** with `pw-record`, then prints peak/RMS per 100 ms. Does not touch real devices. |
| `scripts/e2e-macos.sh` | macOS end-to-end check. Starts `otozaned` with an isolated config, records **Otozane Microphone** with `ffmpeg` with effects off (expects microphone room noise) and muted (expects digital silence). Uses the real microphone, `INPUT=<device UID>` picks one. |
| `scripts/build-driver.sh [OUT_DIR]` | Builds the Otozane Microphone audio driver (`Otozane.driver`) from `packaging/macos/driver` with the pinned libASPL release (default `third_party/driver`). |
| `scripts/build-rnnoise.sh [OUT_DIR]` | Builds static RNNoise 0.2 (default `third_party/rnnoise`), for systems without a package. |
| `scripts/package-macos.sh VERSION OUT_DIR` | Builds `Otozane.app` from `target/release` (`TARGET_DIR` overrides), signs it ad-hoc and packs `otozane-vVERSION-macos-arm64.dmg`. |
| `scripts/tone.py OUT.wav SECONDS AMP [noise]` | Writes a 1 kHz sine or white noise test file. |
| `scripts/analyze.py FILE.wav` | Peak and RMS per 100 ms block. |
| `scripts/ipc.py JSON...` | Sends raw requests to the running service. |
| `scripts/screenshot.sh OUT.png [page] [dark\|light]` | Renders the UI and saves a screenshot (uses `OTOZANE_SCREENSHOT`, `OTOZANE_PAGE`, `OTOZANE_THEME`; `OTOZANE_SCREENSHOT_SIZE=720x480` sets the window size). |
| `scripts/icons.py` | Regenerates the Lucide font subset and `src/icons.rs`. |
| `scripts/fetch-ysfx.sh [OUT_DIR]` | Fetches the pinned ysfx commit with its submodules (default `third_party/ysfx`). `otozane-plugins/build.rs` builds it with CMake; without it JSFX plugins fail to load with "built without JSFX support". |
| `scripts/build-hush.sh [OUT_DIR]` | Builds `libweya_nc.so` from the pinned Hush commit and copies it with the model and licences (default `third_party/hush`). `OTOZANE_HUSH_DIR` points the service at another directory. |

Example checks used during development:

```sh
# +6 dB gain: a 0.25 sine should arrive at ~0.5.
scripts/e2e.sh "" '[{"id":1,"kind":"gain","params":{"gain":6}}]'

# RNNoise on white noise: output should fall to silence after ~0.5 s.
python3 scripts/tone.py /tmp/noise.wav 4 0.1 noise
scripts/e2e.sh /tmp/noise.wav '[{"id":1,"kind":"rnnoise"}]'

# Hush: CPU use and worst 10 ms block on this machine.
cargo test --release -p otozane-core hush::tests::bench -- --ignored --nocapture
scripts/e2e.sh /tmp/noise.wav '[{"id":1,"kind":"hush"}]'

# Service requests
python3 scripts/ipc.py '{"cmd":"status"}'
python3 scripts/ipc.py '{"cmd":"set_param","id":2,"key":"gain","value":3}'
```

Note: when EasyEffects processes all inputs/outputs, test streams must opt out
of being moved (`node.dont-move = true`), which the scripts do.

## Plugins

```sh
# List the effects in a bundle (loads it in this process)
cargo run -p otozane-plugins --example scan -- /usr/lib/vst3/lsp-plugins.vst3
# Load one, print its parameters, process a sine, round-trip its state
cargo run -p otozane-plugins --example probe -- /usr/lib/vst3/lsp-plugins.vst3 "Limiter Mono" 5=0.3
# End to end with a plugin in the chain (output gain about -38 dB -> ~0.003)
scripts/e2e.sh "" '[{"id":1,"kind":"plugin","params":{"409613204":0.3},"plugin":{"format":"vst3","path":"/usr/lib/vst3/lsp-plugins.vst3","uid":"6473702062316D20202020206A7A357A","name":"Limiter Mono"}}]'
```

`tests/lsp.rs` runs against LSP plugins when `lsp-plugins-vst3` is installed
and skips otherwise.

## Adding an effect

1. Implement `Processor` in `crates/otozane-core/src/effects/`.
2. Declare its `ParamSpec`s and add a variant to `EffectKind` (name, params,
   meter, create, `ALL`).
3. The robustness test in `effects/mod.rs` covers it automatically; add a
   behavioral test next to the implementation.

The UI picks up parameters from the specs.

## Commit style

[Conventional Commits](https://www.conventionalcommits.org/): `type(scope): description`.

## Releasing

1. Add a `## X.Y.Z - YYYY-MM-DD` section to `CHANGELOG.md`: what is new, what
   was fixed, written for users. The release workflow publishes it as the
   GitHub release notes and fails without it (`scripts/release-notes.sh X.Y.Z`
   prints it locally).
2. Set `version` in `Cargo.toml`, build to refresh `Cargo.lock`, commit
   `chore: release X.Y.Z`.
3. Tag `vX.Y.Z` and push the branch and the tag.
