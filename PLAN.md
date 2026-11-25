# Otozane roadmap

## Roadmap to 1.0.0

Otozane 1.0 is for processing a physical microphone and using the result in
calls, recordings, and streams. The built-in effects, profiles, and virtual
microphone in [0.6.1](https://github.com/binido/Otozane/releases/tag/v0.6.1)
provide the baseline. Expanding the audio effect formats is part of 1.0.
Work through the milestones below in order.

### Completed baseline

- [x] The maintainer confirmed two weeks of daily use and completed the device
  and service recovery checks.
- [x] The maintainer confirmed that the `Voice` preset and first-run
  experience already work well.
- [x] The maintainer reported good Hush performance during a five-hour gaming
  session with friends. Keep Hush in the candidate smoke test if audio changes
  affect it.

### 1. Expand plugin format support

| Format | Linux | macOS (Apple Silicon) |
| --- | --- | --- |
| VST3 | Keep existing support | Add |
| CLAP | Add | Add |
| Audio Units (AU) | macOS format | Add |
| LV2 | Add | Add |
| LADSPA | Add | Add |
| JSFX | Keep existing support | Keep existing support |
| VST2 | Resolve distribution feasibility | Resolve distribution feasibility |

- [ ] Add macOS VST3 first, then CLAP on both platforms and AU on macOS. Add
  LV2 and LADSPA for native binaries on each platform.
- [ ] Resolve whether Otozane can distribute a VST2 host on Linux and macOS.
  Include VST2 if a distributable implementation is confirmed. Otherwise
  document the limitation before release. [Steinberg's VST2 host licensing statement](https://forums.steinberg.net/t/can-i-create-vst-2-support-for-host-application-vst-3-license/202012/4)
- [ ] For each format and supported platform, verify discovery, mono and stereo
  processing, parameter editing in Otozane, bypass, latency compensation where
  applicable, and profile state restore using native test effects. Include a
  third-party effect per format and record plugin versions in the test results.
  A rejected plugin or a failed scan must leave the existing chain usable.

The target is real-time audio effects built for the operating system and CPU
architecture. Check that available controls work in Otozane's effect cards.
Instruments, MIDI, and bridges for other operating systems or CPU
architectures stay outside this milestone. AAX stays outside 1.0 because
Avid's public SDK is for building plugins for its hosts. [Avid AAX SDK](https://developer.avid.com/aax/)

### 2. Check profile compatibility

- [ ] Open existing VST3 and JSFX profiles from 0.6.1. For each supported
  format, check a profile with a missing effect, including when imported on a
  platform where the effect is unavailable. The card should show the problem,
  sound should continue through the remaining chain, and saving should
  preserve the missing effect's settings. Verify profile export and import
  as well.

Done when the remaining effects keep processing and the missing effect keeps
its settings through save, export, and import.

### 3. Make installation and publication repeatable

- [ ] Keep the macOS DMG ad-hoc signed and document the first-launch,
  microphone permission, and driver installation steps. Use the release page
  for manual macOS updates. Exercise a failed driver reinstall and verify the
  previous driver remains usable. [Audit, C-05](audit/REMEDIATION.md)
- [ ] Keep version checks and release notes in the app. Enable installation
  inside the app only on channels with independently authenticated packages or
  a signed manifest verified by Otozane. Point other channels to the package
  manager or release page. A failed download, verification, or install must
  leave the current version usable. [Audit, C-22](audit/REMEDIATION.md)
- [ ] Make publishing resumable after a partial upload. Publish only after
  checking that all expected packages and their checksums are present.
  [Release workflow](.github/workflows/release.yml)

Done when the documented upgrade from 0.6.1 preserves profiles, failed upgrades
leave a working installation, and rerunning a partial upload produces a
complete release without manual asset repair.

### 4. Qualify the 1.0.0 release candidate

- [ ] Build candidate packages with the release workflow's build and test steps.
  Check clean installation and upgrade from 0.6.1 on PipeWire Linux and Apple
  Silicon macOS, then record through the packaged virtual microphone in an
  ordinary call or recording app.
- [ ] Run the plugin format checks against the packaged candidate on each
  supported platform and architecture. Document the verified formats and
  any plugin-specific limits in the README.
- [ ] Record the package, architecture, and result for each check. Fix any
  reproducible loss of audio in normal use, unexpected unmute, lost profile
  settings, or failure that leaves the installation unusable. Publish after
  those checks pass and document any known platform limits.

### Deferred

- Apple Developer ID signing and notarization. Keep the ad-hoc macOS trust
  limits documented, including the limits of the driver's writer identity
  check. [Audit, C-01 and C-02](audit/REMEDIATION.md)
- Plugin editor windows, JSFX graphics, and support for changes to plugin
  latency while running.
- Hush energy optimization [#20](https://github.com/binido/Otozane/issues/20)
  and resampling-based clock drift correction. Promote a fix if candidate
  testing finds repeatable audio problems.
- Targeted Hush testing with Bluetooth and large Core Audio buffers. The
  five-hour session covers the maintainer's everyday setup. Use the audit
  scenario for follow-up if the processed microphone drops out or repeatedly
  falls back to dry audio.
  [Audit, C-13](audit/REMEDIATION.md)

## Implementation history

These milestones record earlier implementation checkpoints.

Status legend: [x] done, [~] partial, [ ] not started.

### Crates

| Crate | Role |
|-------|------|
| `otozane-core` | Platform-independent DSP engine, built-in effects, RNNoise and Hush FFI, chain, config/profile model, IPC protocol types |
| `otozane-pipewire` | Linux backend: device discovery, capture stream, `Otozane Microphone` virtual source |
| `otozane-coreaudio` | macOS backend: device polling, capture through `cpal`, output into the Otozane Microphone driver (`packaging/macos/driver`, libASPL), resampling |
| `otozane-service` | `otozaned` background service: owns the engine and backend, persists config, serves IPC over a Unix socket |
| `otozane-ui` | `otozane` egui desktop app, talks to the service over IPC |
| `otozane-plugins` | Third-party plugin hosting (VST3), subprocess scanning |

### Phases

1. [x] Functional pipeline: workspace, engine, PipeWire capture, virtual source, gain. Verified with `scripts/e2e.sh` (`pw-record` receives the processed signal).
2. [x] RNNoise: FFI to system `librnnoise`, 480-sample framing, mix, VAD gate. Verified suppressing white noise end to end.
2a. [x] Hush: runtime-loaded `libweya_nc` (built from source), worker thread with fixed latency, stateful 48/16 kHz resampling, bundled in packages. Verified end to end (no late frames at quantum 1024; competing speaker alone -13 dB vs -0.4 dB with RNNoise).
3. [x] Desktop UI: egui theme, Space Grotesk, Lucide icons, device selection, meters, IPC client.
4. [x] DSP and profiles: gate, parametric EQ (with response curve), compressor, limiter, chain editor with drag-and-drop, profile management.
5. [~] Plugin hosting. Done: VST3 (scan in a separate process per bundle, load, process, parameters, state in profiles, bypass, latency, load errors shown on the effect card, plugin browser). Verified with LSP plugins (`tests/lsp.rs`, and `scripts/e2e.sh` with LSP Limiter). JSFX via ysfx (`scripts/fetch-ysfx.sh`, static build from `build.rs` with CMake; scripts found in `~/.config/REAPER/Effects` (macOS: `~/Library/Application Support/REAPER/Effects`) and the plugin directories, `*.jsfx` and extensionless files with `desc:`; visible sliders as normalized parameters, state with `@serialize` data in profiles, `pdc_delay` as latency, compile errors on the effect card). Verified with `tests/jsfx.rs` (`tests/gain.jsfx`) and `scripts/e2e.sh` with that script in the chain (-24 dB: peak 0.250 to 0.016). Open: latency changes reported by a plugin at runtime are ignored until it is re-added; plugin editor windows.
6. [~] Service hardening: done: IPC, single instance, systemd user unit with a "Start with session" switch in the UI (verified: enable, start, restart after kill -9), device unplug/replug recovery, PipeWire reconnect loop, diagnostics, coexistence with a running EasyEffects. Open: verify behavior across suspend/resume and PipeWire restarts on a real session.
7. [x] Tray and login: StatusNotifierItem tray via `ksni` (service state, show/hide, quit). The tray and menu bar icon is crossed out when Muted, on Audio error or Service stopped, and dimmed when Bypassed (`packaging/tray*.svg`). Closing the window hides it; reopening reuses the eframe event loop. Optional XDG autostart entry (`--hidden`, tray only) behind a Settings switch. Verified on niri with Noctalia by driving the tray over D-Bus (show, hide, repeated show, quit). Not verified: an actual logout/login with the entry.
8. [~] Expansion. Done: packaging (PKGBUILD, deb, rpm), macOS backend (Core Audio through `cpal`, own Otozane Microphone driver (libASPL loopback, writes only from otozaned; verified: tone through, foreign writer dropped, muted is digital silence), launchd agent, `.dmg`, verified with `scripts/e2e-macos.sh` and under launchd). macOS window in the system frame (Metal through wgpu) and a menu bar icon (`tray-icon`, verified: mode switch, hide on close, show, quit), the Dock icon reopens a closed window (reopen Apple event), Hush as `libweya_nc.dylib` in `Contents/Resources/hush` (bench: 1.8% of one core mono, worst 10 ms block 0.29 ms; the running service shows about 9-11% of one core in `top`). Open: CLAP, plugin editors; on macOS VST3 (`Contents/MacOS` bundles), notarization.

### Key decisions

- Capture stream + feed stream connected by a lock-free SPSC ring buffer; the feed is linked by us into a client-owned `support.null-audio-sink` exposed as `Audio/Source/Virtual` (WirePlumber will not link a playback stream into a source).
- The capture stream always targets an explicit node and never our own virtual source, so no feedback loop is possible through auto-linking.
- Parameters are atomics read by the RT thread; structural chain changes are built off-thread and swapped in through an SPSC queue, and the old chain is returned for deallocation off the RT thread.
- IPC: newline-delimited JSON over `$XDG_RUNTIME_DIR/otozane.sock`.
- Config: TOML under `$XDG_CONFIG_HOME/otozane/`.
- macOS keeps the shared egui UI in the system window frame (Metal through wgpu). A native AppKit or Liquid Glass UI would duplicate every screen, the native part is limited to a menu bar icon.
