# Changelog

Every release has a section here. The release workflow publishes the section
for the tagged version as the GitHub release notes and fails if it is missing.

## Unreleased

### New

- **macOS version** (macOS 26+, Apple Silicon), shipped as a `.dmg` with
  `Otozane.app`. The processed microphone goes to
  [BlackHole 2ch](https://existential.audio/blackhole/), which apps select as
  their microphone. All built-in effects except Hush, profiles, modes and
  **Listen to myself** work as on Linux. **Start with session** installs a
  launchd agent. The window has the system frame, and a menu bar icon
  switches the mode and brings the window back after it is closed. No Hush
  or VST3 plugins on macOS yet.

## 0.3.0 - 2026-11-13

### New

- **Hush Noise Suppression.** A new built-in effect that removes background
  noise and other people's voices, keeping only yours. Built on Weya AI's Hush
  model (DeepFilterNet3, fine-tuned). In a test with a competing speaker it
  cut the other voice by about 13 dB, where RNNoise managed under 1 dB. It adds
  about 44 ms of latency.
  - **Attenuation limit** sets how far noise is pushed down (0-100 dB, 100 =
    no limit), for a more natural sound with some background left in.
  - **Mix** blends the processed and the original signal.
  - The Hush runtime ships with the Linux packages in `/usr/lib/otozane/hush/`.
    When building from source, run `scripts/build-hush.sh`. Without it Otozane
    still works and the effect shows "Hush is not installed".
- **Listen to myself.** A switch on **Overview → Input** plays the processed
  microphone, exactly as apps hear it, to your default output. It follows the
  output when you switch devices, and its volume is in the system mixer as
  "Otozane Monitor". It is always off when the service starts. Use headphones:
  speakers feed back into the microphone.
- **Muted mode.** Next to **On** and **No effects**: capture stops,
  "Otozane Microphone" outputs silence, and almost no CPU is used. Not
  remembered: the service always starts unmuted.
- **CPU-free bypass.** A bypassed effect no longer uses any CPU. Turning it
  back on is still click-free.
- **Peak hold** on the Overview level meters.

### Fixed

- A re-enabled effect could briefly play audio left over from before it was
  bypassed. It now stays dry until its buffers have refilled.
- PipeWire could raise the capture block size to 2048 frames when another app
  asked for more latency (for example `pw-play`). Capture is now capped at
  1024 frames, so Hush does not drop frames.
- Switching between **On** and **No effects** no longer sends a mute request
  that older services rejected.

### Other

- The README is restructured, with screenshots in light and dark themes.

**Full diff**: https://github.com/binido/Otozane/compare/v0.2.0...v0.3.0
