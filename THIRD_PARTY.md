# Third-party components

| Component | Use | License |
|-----------|-----|---------|
| [Roboto](https://github.com/googlefonts/roboto) (v2.138) | UI font, bundled in `crates/otozane-ui/assets/fonts/` | Apache License 2.0 (`Roboto-LICENSE.txt`) |
| [Lucide](https://github.com/lucide-icons/lucide) (`lucide-static` 1.62.0) | UI icons, bundled as a font subset | ISC (`Lucide-LICENSE.txt`) |
| [RNNoise](https://github.com/xiph/rnnoise) | Noise suppression. Linked from the system library, statically into release builds (licence shipped as `RNNoise-COPYING`) | BSD-3-Clause |
| [Hush](https://github.com/pulp-vision/Hush) (Weya AI, commit `9f6414e`): `libweya_nc` and the `advanced_dfnet16k` model | Hush Noise Suppression. Built from source by `scripts/build-hush.sh`, shipped in `/usr/lib/otozane/hush/` and `Otozane.app/Contents/Resources/hush/` (licence as `Hush-LICENSE`) | Apache License 2.0 |
| [DeepFilterNet](https://github.com/Rikorose/DeepFilterNet) `deep_filter` 0.5.3 (vendored in Hush) and the Rust crates linked into `libweya_nc` ([tract](https://github.com/sonos/tract), rustfft, ndarray, ...) | Inference inside `libweya_nc.so`. Crates and licences are listed in `Hush-crates.txt` | MIT OR Apache-2.0 (`DeepFilterNet-LICENSE-MIT`, `DeepFilterNet-LICENSE-APACHE`); a few crates MIT, Unlicense, Zlib, BSD-2-Clause, CC0-1.0 or Unicode-3.0 alternatives, see the list |
| [PipeWire](https://pipewire.org) via [pipewire-rs](https://gitlab.freedesktop.org/pipewire/pipewire-rs) | Audio backend | MIT |
| [egui / eframe](https://github.com/emilk/egui) | Desktop UI (OpenGL on Linux, Metal through wgpu on macOS) | MIT OR Apache-2.0 |
| [cpal](https://github.com/RustAudio/cpal), [objc2](https://github.com/madsmtm/objc2) crates | macOS audio backend and AppKit calls | Apache-2.0 / MIT |
| [tray-icon](https://github.com/tauri-apps/tray-icon) | macOS menu bar icon | MIT OR Apache-2.0 |
| [libASPL](https://github.com/gavv/libASPL) v3.1.2 | Otozane Microphone audio driver on macOS, linked statically into `Otozane.driver` | MIT, with parts under Apple sample code licences (`libASPL-LICENSE*`) |
| [rtrb](https://github.com/mgeier/rtrb) | Lock-free SPSC ring buffers | MIT OR Apache-2.0 |
| serde, serde_json, toml | Configuration and IPC | MIT OR Apache-2.0 |
| [vst3](https://github.com/coupler-rs/vst3-rs) (vst3-rs 0.3) | VST3 interface bindings for plugin hosting | MIT OR Apache-2.0 |
| [rfd](https://github.com/PolyMeilex/rfd) | Native file dialogs (profile import/export, via the XDG desktop portal) | MIT |
| [libloading](https://github.com/nagisa/rust_libloading) | Loading plugin libraries and the Hush runtime | ISC |

No third-party audio plugins are bundled.
