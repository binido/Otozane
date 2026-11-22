# Changelog

Every release has a section here. The release workflow publishes the section
for the tagged version as the GitHub release notes and fails if it is missing.

## 0.5.0 - 2026-11-22

### New

- **Stop processing when the window is closed** in **Settings → Background**.
  Otozane starts processing when you open it and exits the interface when you
  close it, so nothing processes audio in the background on a laptop or a
  low-power PC. Apps keep **Otozane Microphone** selected and receive silence
  until you reopen Otozane. The option is off by default and disables login
  start. On Linux a small idle process keeps the microphone so it does not
  disappear.

### Fixed

- **macOS:** switching between Mono and Stereo no longer restarts the
  microphone stream. The silence at the switch went from about 200 ms down to
  about 15 ms.
- **macOS:** the Dock icon has the same size as other apps' icons, whether
  Otozane is running or not.
- Interface polish:
  - The plugin search field no longer runs under **Rescan**.
  - Segmented controls (**Theme**, **Format**) have equal segments and thin
    dividers.
  - The plugin browser shows no stray dots for scripts without an author or
    tags.
  - No empty strip under an effect card with a load error.
  - The Equalizer card is as wide as the other cards, and profile actions
    line up.
  - In narrow windows, the mode switch, row descriptions and the **Bypassed**
    badge no longer overlap other controls.
- Stopping the service during a plugin scan no longer waits for processes
  started by a plugin.
- Building from source on ARM Linux no longer fails in the VST3 hosting code.

### Other

- Development scripts respect `CARGO_TARGET_DIR`. `OTOZANE_SCREENSHOT_SIZE`
  sets the window size for UI screenshots.

**Full diff**: https://github.com/binido/Otozane/compare/v0.4.1...v0.5.0

## 0.4.1 - 2026-11-22

### New

- **JSFX plugins.** Effects written in REAPER's JSFX language now load like
  VST3 plugins. Put the scripts in `~/.config/REAPER/Effects` (on macOS
  `~/Library/Application Support/REAPER/Effects`, or any folder
  added to the plugin directories), press **Rescan**, and pick them in the
  plugin browser, marked JSFX.
  - Sliders appear as parameters and are saved in the profile together with
    the script's own state.
  - A script that fails to compile shows the error on its effect card.
  - Scripts with graphics (`@gfx`) work, but their graphics are not shown.
    MIDI is not passed to scripts.
- **Mono or Stereo without a restart.** Switch the format in **Settings →
  Audio → Format**, the audio service applies it right away. Apps recording
  from Otozane Microphone keep recording, with a short silence at the switch.
  Effects and plugins are rebuilt for the new format and keep their settings.
- `set_channels` IPC request (`{"cmd":"set_channels","channels":2}`).
- **The tray and menu bar icon shows the state.** It is crossed out when
  Muted, on an audio error or when the service is stopped, and dimmed when
  effects are bypassed (**No effects**).

### Fixed

- The Linux tray showed the full app icon with its dark background. Linux and
  macOS now share one monochrome icon that the panel recolours to match its
  theme, and it stays visible on light panels.
- In **Settings**, labels of rows without a description (such as **Theme**)
  are centred vertically.

### Other

- Building from source now needs `cmake` and `scripts/fetch-ysfx.sh` for JSFX
  support. Without the ysfx sources Otozane builds without JSFX. See
  [DEVELOPMENT.md](DEVELOPMENT.md).

**Full diff**: https://github.com/binido/Otozane/compare/v0.4.0...v0.4.1

## 0.4.0 - 2026-11-14

### New

- **macOS version.** Otozane now runs on macOS 26 and later on Apple Silicon.
  Download `otozane-v0.4.0-macos-arm64.dmg` and drag **Otozane** into
  **Applications**. The app is not notarized: on first launch right-click it
  in Finder and choose **Open**.
- **Otozane Microphone on macOS.** A small audio driver ships inside the app.
  Click **Install** on the Overview page, enter the administrator password,
  and select **Otozane Microphone** as the microphone in Discord, OBS, Zoom or
  any other app. No BlackHole or other virtual audio software is needed.
  - Only Otozane can play into it, so audio from other apps never ends up in
    your microphone.
  - It can be the system default input and is never offered as an output.
  - **Settings → Virtual microphone** reinstalls the driver after an update or
    removes it. Installing restarts the system audio service, so all sound
    pauses for a moment.
- **Everything from Linux works on macOS**: all built-in effects including
  RNNoise and Hush, profiles, **On / No effects / Muted** and **Listen to
  myself**. Hush uses about 2% of one core on Apple Silicon.
  - **Automatic** input follows the system default microphone and skips
    virtual devices.
  - Microphones and outputs at other sample rates (Bluetooth headsets at
    16 kHz, outputs at 44.1 kHz) are resampled to and from 48 kHz.
- **Menu bar icon on macOS.** Shows the service state, switches the mode, shows
  the window and quits. Closing the window only hides it, the menu bar icon or
  the Dock icon brings it back.
- **Start with session on macOS** installs a launch agent, so the audio
  service starts when you log in.
- The macOS window uses the system frame (rounded corners, traffic lights)
  and is drawn with Metal.

Not available on macOS yet: VST3 plugins.

### Other

- No behaviour changes on Linux. Device handling shared by both platforms
  moved into `otozane-core`.
- Building on macOS: `scripts/build-rnnoise.sh`, `scripts/build-hush.sh`,
  `scripts/build-driver.sh`, then `cargo build --release` and
  `scripts/package-macos.sh`. See [DEVELOPMENT.md](DEVELOPMENT.md).
- `scripts/ipc.py` finds the service socket on macOS.

**Full diff**: https://github.com/binido/Otozane/compare/v0.3.0...v0.4.0

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
