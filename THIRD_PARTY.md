# Third-party components

| Component | Use | License |
|-----------|-----|---------|
| [Space Grotesk](https://github.com/floriankarsten/space-grotesk) | UI font, bundled in `crates/otozane-ui/assets/fonts/` | SIL Open Font License 1.1 (`SpaceGrotesk-OFL.txt`) |
| [Lucide](https://github.com/lucide-icons/lucide) (`lucide-static` 1.62.0) | UI icons, bundled as a font subset | ISC (`Lucide-LICENSE.txt`) |
| [RNNoise](https://github.com/xiph/rnnoise) | Noise suppression, linked from the system library | BSD-3-Clause |
| [PipeWire](https://pipewire.org) via [pipewire-rs](https://gitlab.freedesktop.org/pipewire/pipewire-rs) | Audio backend | MIT |
| [egui / eframe](https://github.com/emilk/egui) | Desktop UI | MIT OR Apache-2.0 |
| [rtrb](https://github.com/mgeier/rtrb) | Lock-free SPSC ring buffers | MIT OR Apache-2.0 |
| serde, serde_json, toml | Configuration and IPC | MIT OR Apache-2.0 |

No third-party audio plugins are bundled.
