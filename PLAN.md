# Otozane Implementation Plan

Status legend: [x] done, [~] partial, [ ] not started.

## Crates

| Crate | Role |
|-------|------|
| `otozane-core` | Platform-independent DSP engine, built-in effects, RNNoise FFI, chain, config/profile model, IPC protocol types |
| `otozane-pipewire` | Linux backend: device discovery, capture stream, `Otozane Microphone` virtual source |
| `otozane-service` | `otozaned` background service: owns the engine and backend, persists config, serves IPC over a Unix socket |
| `otozane-ui` | `otozane` egui desktop app, talks to the service over IPC |

Plugin hosting gets its own crate once it has real code (Phase 5).

## Phases

1. [ ] Functional pipeline: workspace, engine, PipeWire capture, virtual source, gain, verified with `pw-record`.
2. [ ] RNNoise: FFI to system `librnnoise`, 480-sample framing, enable/disable.
3. [ ] Desktop UI: egui theme, Space Grotesk, Lucide icons, device selection, meters.
4. [ ] DSP and profiles: gate, parametric EQ, compressor, limiter, chain editor, profiles.
5. [ ] Plugin hosting: VST3, then JSFX (ysfx).
6. [ ] Service hardening: IPC, systemd user unit, device recovery, diagnostics.
7. [ ] Expansion: CLAP, editors, packaging, macOS backend.

## Key decisions

- Two `pw_stream`s (capture + `Audio/Source/Virtual` playback) connected by a lock-free SPSC ring buffer, the same shape as `module-loopback`.
- The capture stream always targets an explicit node and never our own virtual source, so no feedback loop is possible through auto-linking.
- Parameters are atomics read by the RT thread; structural chain changes are built off-thread and swapped in through an SPSC queue, and the old chain is returned for deallocation off the RT thread.
- IPC: newline-delimited JSON over `$XDG_RUNTIME_DIR/otozane.sock`.
- Config: TOML under `$XDG_CONFIG_HOME/otozane/`.
