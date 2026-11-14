# Otozane Implementation Plan

Status legend: [x] done, [~] partial, [ ] not started.

## Crates

| Crate | Role |
|-------|------|
| `otozane-core` | Platform-independent DSP engine, built-in effects, RNNoise and Hush FFI, chain, config/profile model, IPC protocol types |
| `otozane-pipewire` | Linux backend: device discovery, capture stream, `Otozane Microphone` virtual source |
| `otozane-coreaudio` | macOS backend: device polling, capture through `cpal`, output into BlackHole 2ch, resampling |
| `otozane-service` | `otozaned` background service: owns the engine and backend, persists config, serves IPC over a Unix socket |
| `otozane-ui` | `otozane` egui desktop app, talks to the service over IPC |
| `otozane-plugins` | Third-party plugin hosting (VST3), subprocess scanning |

## Phases

1. [x] Functional pipeline: workspace, engine, PipeWire capture, virtual source, gain. Verified with `scripts/e2e.sh` (`pw-record` receives the processed signal).
2. [x] RNNoise: FFI to system `librnnoise`, 480-sample framing, mix, VAD gate. Verified suppressing white noise end to end.
2a. [x] Hush: runtime-loaded `libweya_nc` (built from source), worker thread with fixed latency, stateful 48/16 kHz resampling, bundled in packages. Verified end to end (no late frames at quantum 1024; competing speaker alone -13 dB vs -0.4 dB with RNNoise).
3. [x] Desktop UI: egui theme, Space Grotesk, Lucide icons, device selection, meters, IPC client.
4. [x] DSP and profiles: gate, parametric EQ (with response curve), compressor, limiter, chain editor with drag-and-drop, profile management.
5. [~] Plugin hosting. Done: VST3 (scan in a separate process per bundle, load, process, parameters, state in profiles, bypass, latency, load errors shown on the effect card, plugin browser). Verified with LSP plugins (`tests/lsp.rs`, and `scripts/e2e.sh` with LSP Limiter). Open: JSFX via ysfx (source in `third_party/ysfx`, not built yet); latency changes reported by a plugin at runtime are ignored until it is re-added; plugin editor windows.
6. [~] Service hardening: done: IPC, single instance, systemd user unit with a "Start with session" switch in the UI (verified: enable, start, restart after kill -9), device unplug/replug recovery, PipeWire reconnect loop, diagnostics, coexistence with a running EasyEffects. Open: verify behavior across suspend/resume and PipeWire restarts on a real session.
7. [x] Tray and login: StatusNotifierItem tray via `ksni` (service state, show/hide, quit). Closing the window hides it; reopening reuses the eframe event loop. Optional XDG autostart entry (`--hidden`, tray only) behind a Settings switch. Verified on niri with Noctalia by driving the tray over D-Bus (show, hide, repeated show, quit). Not verified: an actual logout/login with the entry.
8. [~] Expansion. Done: packaging (PKGBUILD, deb, rpm), macOS backend (Core Audio through `cpal`, BlackHole 2ch as the virtual microphone, launchd agent, `.dmg`, verified with `scripts/e2e-macos.sh` and under launchd). macOS window in the system frame (Metal through wgpu) and a menu bar icon (`tray-icon`, verified: mode switch, hide on close, show, quit), the Dock icon reopens a closed window (reopen Apple event), Hush as `libweya_nc.dylib` in `Contents/Resources/hush` (bench: 1.8% of one core mono, worst 10 ms block 0.29 ms; the running service shows about 9-11% of one core in `top`). Open: CLAP, plugin editors; on macOS VST3 (`Contents/MacOS` bundles), an own virtual device instead of BlackHole, notarization.

## Next steps

- JSFX via ysfx.
- Tune the Voice preset on real speech (not verified on a speech recording yet).
- Optional resampling-based drift correction instead of dropping frames.
- Stereo/mono switch at runtime (currently needs a service restart).

## Key decisions

- Capture stream + feed stream connected by a lock-free SPSC ring buffer; the feed is linked by us into a client-owned `support.null-audio-sink` exposed as `Audio/Source/Virtual` (WirePlumber will not link a playback stream into a source).
- The capture stream always targets an explicit node and never our own virtual source, so no feedback loop is possible through auto-linking.
- Parameters are atomics read by the RT thread; structural chain changes are built off-thread and swapped in through an SPSC queue, and the old chain is returned for deallocation off the RT thread.
- IPC: newline-delimited JSON over `$XDG_RUNTIME_DIR/otozane.sock`.
- Config: TOML under `$XDG_CONFIG_HOME/otozane/`.
- macOS keeps the shared egui UI in the system window frame (Metal through wgpu). A native AppKit or Liquid Glass UI would duplicate every screen, the native part is limited to a menu bar icon.
