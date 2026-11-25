# Otozane agent guide

## Repository language

Write all repository content in English, including documentation, code comments,
user-facing text, tests, and release notes. Use English when refreshing
`BIG-PICTURE.md`, regardless of the conversation language.

## Project snapshot

Otozane is a Rust desktop application and background service for real-time microphone processing. It captures a physical microphone, applies an effect chain, and exposes **Otozane Microphone** for recording apps. Linux uses PipeWire and WirePlumber. The macOS release targets Apple Silicon on macOS 26+ and uses Core Audio. The default branch is `main`.

## Sources of truth

- Product behavior, supported platforms, and configuration locations → [README.md](README.md)
- Local setup, test fixtures, and release procedure → [DEVELOPMENT.md](DEVELOPMENT.md)
- Module navigation and design, audio routing, timing, IPC, and plugin lifecycle → [ARCHITECTURE.md](ARCHITECTURE.md)
- Current product and feature status, plus the sourced backlog → [BIG-PICTURE.md](BIG-PICTURE.md)
- Dependency requirements and resolved versions → [Cargo.toml](Cargo.toml) and [Cargo.lock](Cargo.lock)

## Architecture and repository layout

| Path | Responsibility |
| --- | --- |
| `crates/otozane-core/` | Platform-independent DSP, effects, shared backend helpers, settings and profile types, and the IPC contract. |
| `crates/otozane-service/` | `otozaned` owns the engine and target backend, handles IPC and plugin discovery, and persists settings and profiles. |
| `crates/otozane-pipewire/`, `crates/otozane-coreaudio/` | Target-specific device discovery, stream routing, and callback integration. Share device-selection and ring-buffer logic through core. |
| `crates/otozane-plugins/` | VST3 hosting on Linux and JSFX hosting through ysfx on both platforms. |
| `crates/otozane-ui/` | `otozane`, the egui desktop client. Queue IPC on the background client thread and keep device and DSP ownership in the service. |
| `packaging/`, `scripts/` | Platform packaging, macOS HAL driver source, dependency builders, and validation scripts. `third_party/`, `target/`, and `dist/` hold local build inputs and outputs. |

The backends feed the engine interleaved `f32` at 48 kHz, mono or stereo, in internal blocks of up to 1024 frames. Build effects and reclaim old chains on the controller thread. Use preallocated buffers, atomics, and `rtrb` queues for callback work. Keep allocation, deallocation, blocking locks, I/O, and logging out of audio callbacks. Preserve state on same-ID, same-kind reorders and preserve latency-aligned dry audio on bypass.

## Tooling and validation

- Use stable Rust and Cargo with edition 2024. The workspace requires Rust 1.95, which the locked `egui`/`eframe` 0.37.0 need. The backends use `pipewire` 0.10 and `cpal` 0.18.
- **Linux** needs a C/C++ toolchain, Clang, `pkgconf`, and PipeWire development headers (`libpipewire-0.3-dev` on Debian). Use system RNNoise where packaged or run `scripts/build-rnnoise.sh` to supply the static library. Audio checks require a running PipeWire/WirePlumber session, PipeWire tools, and Python 3.
- **macOS** needs Xcode command-line tools. Run `scripts/build-rnnoise.sh` before linking. For virtual microphone checks, build the driver with `scripts/build-driver.sh` (Git and CMake), install it via the app, and allow microphone access. The E2E script also needs FFmpeg and Python 3.
- Run `scripts/build-hush.sh` to build the pinned Hush runtime and copy its model bundle, and `scripts/fetch-ysfx.sh` before building and testing JSFX. Both scripts need Git and place their output in `third_party/`. The JSFX build also needs CMake and a C++ toolchain.

Run these commands from the repository root after setup.

| Task | Command |
| --- | --- |
| Build debug binaries | `cargo build --locked` |
| Format check | `cargo fmt --all -- --check` |
| Lint | `cargo clippy --locked --workspace --all-targets` |
| Focused DSP test | `cargo test --locked -p otozane-core applies_gain_and_ramps` |
| Workspace tests | `cargo test --locked --workspace` |
| Linux audio smoke test | `scripts/e2e.sh` |
| macOS audio smoke test | `scripts/e2e-macos.sh` |

Keep unit tests next to the implementation in `#[cfg(test)]` modules. Plugin integration tests and fixtures live in `crates/otozane-plugins/tests/`. Hush, JSFX, and LSP checks can skip without the runtime and model, ysfx sources, or `/usr/lib/vst3/lsp-plugins.vst3`. Install the relevant dependency before claiming coverage. Run the focused check first, then the workspace suite for shared engine, IPC, or profile changes. For routing or device lifecycle changes, use the affected platform's smoke test. Linux prints levels from a synthetic source for inspection. macOS records the real mic and checks for room noise with effects off and digital silence when muted.

## Patterns and contracts

- Add a built-in effect using [`effects/gain.rs`](crates/otozane-core/src/effects/gain.rs) as a reference. Implement `Processor`, declare static `ParamSpec`s, register the effect in `EffectKind` and `EffectKind::ALL`, and add a behavior test. The UI renders controls from the specs, and `effects/mod.rs` checks mono and stereo at extreme parameters and varied block sizes.
- Put shared IPC changes in [`protocol.rs`](crates/otozane-core/src/protocol.rs) and update the service handler and [background client](crates/otozane-ui/src/client.rs) together. The Unix socket carries newline-delimited JSON with a 1 MiB request limit and mode `0600`.
- Preserve existing TOML settings and profiles when changing [`config.rs`](crates/otozane-core/src/config.rs). Keep name validation and atomic writes in [`store.rs`](crates/otozane-service/src/store.rs). The UI owns import/export file dialogs and file I/O. IPC carries profile data.
- Follow nearby Rust naming and `rustfmt`. Reuse UI controls and theme primitives from `widgets.rs` and `theme.rs` in the UI crate.

## Safety and gotchas

- Test with a disposable `XDG_CONFIG_HOME` and `OTOZANE_SOCKET` shared by the service, UI, and `scripts/ipc.py`. For a parallel Linux instance, also set `OTOZANE_NODE_PREFIX`. Keep live profiles and microphone recordings out of commits and test fixtures.
- Keep **Automatic** input selection limited to physical devices. Preserve the PipeWire flags that prevent fallback to the virtual mic and rerouting by tools such as EasyEffects. When testing stop-on-close, verify that an existing recording keeps the same microphone and gets silence until processing resumes. Linux keeps the PipeWire node alive. On macOS the HAL driver does it.
- Preserve the macOS driver UID and service bundle ID used to authorize writes. Driver installation and removal require administrator access and restart system audio. Confirm before doing either on a user's machine.
- Scan VST3 bundles in helper processes. Loaded plugins execute inside `otozaned` and can crash it, so use trusted fixtures. Hush inference runs on a worker and plays latency-aligned dry audio on a missed deadline. Recheck that timing if changing capture quantum or engine block size.

## Git and release workflow

Use Conventional Commits, for example `fix(macos): preserve the selected input`. Before handoff, check the diff, run validation for the changed behavior, and report any platform or dependency coverage gaps. Update affected docs and tests alongside behavior changes.

For a release, add user-facing notes under `## X.Y.Z - YYYY-MM-DD` in `CHANGELOG.md`, bump the workspace version, and build to refresh `Cargo.lock`. Validate the notes with `scripts/release-notes.sh` and the release version as its argument. A pushed `vX.Y.Z` tag matching the manifest triggers the release workflow. It builds and tests with `--release --locked` before publishing Linux x86_64/aarch64 packages and a macOS arm64 DMG. Keep tag pushes and publishing to explicitly requested release work. See the [release workflow](.github/workflows/release.yml).
