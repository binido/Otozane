# Otozane Implementation Plan

Status legend: [x] done, [~] partial, [ ] not started.

## Crates

| Crate | Role |
|-------|------|
| `otozane-core` | Platform-independent DSP engine, built-in effects, RNNoise FFI, chain, config/profile model, IPC protocol types |
| `otozane-pipewire` | Linux backend: device discovery, capture stream, `Otozane Microphone` virtual source |
| `otozane-service` | `otozaned` background service: owns the engine and backend, persists config, serves IPC over a Unix socket |
| `otozane-ui` | `otozane` egui desktop app, talks to the service over IPC |
| `otozane-plugins` | Third-party plugin hosting (VST3), subprocess scanning |

## Phases

1. [x] Functional pipeline: workspace, engine, PipeWire capture, virtual source, gain. Verified with `scripts/e2e.sh` (`pw-record` receives the processed signal).
2. [x] RNNoise: FFI to system `librnnoise`, 480-sample framing, mix, VAD gate. Verified suppressing white noise end to end.
3. [x] Desktop UI: egui theme, Space Grotesk, Lucide icons, device selection, meters, IPC client.
4. [x] DSP and profiles: gate, parametric EQ (with response curve), compressor, limiter, chain editor with drag-and-drop, profile management.
5. [~] Plugin hosting. Done: VST3 (scan in a separate process per bundle, load, process, parameters, state in profiles, bypass, latency, load errors shown on the effect card, plugin browser). Verified with LSP plugins (`tests/lsp.rs`, and `scripts/e2e.sh` with LSP Limiter). Open: JSFX via ysfx (source in `third_party/ysfx`, not built yet); latency changes reported by a plugin at runtime are ignored until it is re-added; plugin editor windows.
6. [~] Service hardening: done: IPC, single instance, systemd user unit with a "Start with session" switch in the UI (verified: enable, start, restart after kill -9), device unplug/replug recovery, PipeWire reconnect loop, diagnostics, coexistence with a running EasyEffects. Open: verify behavior across suspend/resume and PipeWire restarts on a real session.
7. [ ] Expansion: CLAP, plugin editors, packaging (PKGBUILD), macOS backend.

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
