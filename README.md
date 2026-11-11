# Otozane

Otozane is an open-source, real-time microphone processor for Linux. It captures a
physical microphone through PipeWire, runs it through a chain of effects, and
publishes the result as a virtual input device, **Otozane Microphone**, that
Discord, OBS, browsers and other applications can record from.

```
Physical microphone → effect chain → Otozane Microphone → applications
```

It processes microphone input only. It does not touch system output, other
applications' audio, or your default devices.

## Features

- Virtual microphone (`Otozane Microphone`) owned by the service and removed
  automatically when it stops.
- Microphone selection ("Automatic" follows the system default microphone), with automatic recovery when a device is unplugged and
  plugged back in.
- Built-in effects: RNNoise noise suppression (with optional voice-activity
  gate), gain, noise gate, five-band parametric EQ, compressor, lookahead limiter.
- Reorderable effect chain with live parameter changes and click-free bypass.
- Voice preset (default for new setups): gain, RNNoise with voice detection,
  gate, compressor, limiter. Profiles → Presets creates it as a new profile.
- Profiles: create, rename, delete, switch. The active profile saves automatically.
- Background service (`otozaned`); the GUI (`otozane`) is optional.
- egui desktop app with light/dark/system themes, level meters and diagnostics.

- VST3 effect plugins (Linux builds, e.g. `lsp-plugins-vst3`): plugin browser
  with search, parameters, state saved in profiles. Plugins are scanned in
  separate processes, but run inside the service: a plugin that crashes while
  processing stops the service. Plugin editor windows are not supported.

Not implemented yet: JSFX, CLAP, plugin editor windows. See [PLAN.md](PLAN.md).

## Requirements

- Linux with PipeWire and WirePlumber (developed on CachyOS, PipeWire 1.6).
- Rust stable (1.85+), `clang` (for PipeWire bindings), `pkgconf`.
- System libraries: `pipewire`, `rnnoise`. On Arch: `pacman -S pipewire rnnoise clang`.

## Build and run

```sh
cargo build --release
./target/release/otozaned &      # audio service
./target/release/otozane          # desktop app (can also start the service)
```

Then pick **Otozane Microphone** as the input device in your application.

### Run the service with your session (optional)

```sh
install -Dm755 target/release/otozaned ~/.local/bin/otozaned
install -Dm755 target/release/otozane  ~/.local/bin/otozane
install -Dm644 packaging/otozane.service ~/.config/systemd/user/otozane.service
systemctl --user daemon-reload
systemctl --user enable --now otozane
```

Nothing is enabled automatically.

## Configuration

Stored under `$XDG_CONFIG_HOME/otozane/` (usually `~/.config/otozane/`):

- `config.toml` — selected microphone, processing on/off, active profile, channel count.
- `profiles/<name>.toml` — effect chains.
- `ui.toml` — UI theme.

Example profile:

```toml
[[chain]]
id = 1
kind = "rnnoise"
enabled = true
params = { mix = 100.0, vad_threshold = 0.0, vad_grace = 200.0 }

[[chain]]
id = 2
kind = "compressor"
params = { threshold = -18.0, ratio = 3.0 }
```

## Coexistence with EasyEffects

Otozane runs alongside EasyEffects without changing its configuration. If
EasyEffects' "process all inputs" option is on, it may move recording
applications onto its own source; Otozane's internal streams opt out of being
moved, but applications recording from Otozane Microphone may still be moved
by EasyEffects. Exclude them in EasyEffects if needed.

## Documentation

- [ARCHITECTURE.md](ARCHITECTURE.md) — design and real-time model.
- [DEVELOPMENT.md](DEVELOPMENT.md) — building, testing, verification scripts.
- [PLAN.md](PLAN.md) — roadmap and status.
- [THIRD_PARTY.md](THIRD_PARTY.md) — bundled assets and dependency licenses.

## License

MIT, see [LICENSE](LICENSE). Bundled fonts and icons keep their own licenses.
