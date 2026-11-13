# Architecture

## Crates

```
otozane-core       platform-independent: engine, effects, config, IPC protocol
otozane-pipewire   Linux backend (depends on core)
otozane-service    otozaned binary (core + pipewire)
otozane-plugins    third-party plugin hosting (VST3)
otozane-ui         otozane binary (core only; talks to the service over IPC)
```

`otozane-core` has no Linux- or GUI-specific code apart from the Unix-socket
client helper, so a future Core Audio backend only needs a new backend crate
that drives `engine::Engine` from its audio callback.

## Audio path (Linux)

```
 mic node ──► otozane_capture ─┐                       ┌─► otozane_output ──► otozane_microphone ──► apps
             (pw_stream, in)   │  Engine::process      │   (pw_stream, out)   (null-audio-sink,
                               └─► SPSC ring buffer ───┘                       Audio/Source/Virtual)
```

- **Capture stream** targets the selected node by `node.name` with
  `node.dont-fallback`, `node.dont-reconnect` and `node.dont-move`, so
  WirePlumber never falls back to the default source (which might be our own
  virtual microphone) and tools that retarget streams through metadata
  (e.g. EasyEffects) cannot move it. The engine runs inside this stream's
  real-time process callback.
- **Virtual microphone** is a `support.null-audio-sink` adapter created by the
  service with `media.class = Audio/Source/Virtual` and `object.linger = false`,
  so PipeWire removes it when the service exits or crashes.
- The capture stream requests `node.latency = 1024/48000`, so the graph
  quantum stays at or below one engine block (see Hush below).
- **Output stream** is not autoconnected; the backend links its ports to the
  virtual microphone's input ports itself (client-owned links), because
  WirePlumber will not link a playback stream into a source node.
- The two sides can run on different graph drivers (and quanta). A lock-free
  ring buffer decouples them. The playback side discards stale audio beyond
  one capture quantum, so latency stays bounded after idle periods or clock
  drift. There is no resampling; drift is corrected by dropping frames.
- "Automatic" input follows the system default source (the `default`
  metadata's `default.audio.source`), switching live when it changes. A virtual
  default (our own microphone, EasyEffects' output) is skipped: the last
  physical default seen while running is kept, else the first physical device.
- Device discovery uses the registry. Our own virtual microphone is never
  listed, and the service rejects it as an input. When the selected device
  disappears the capture stream is destroyed; it is recreated when a node with
  the same `node.name` appears again.
- If the PipeWire connection is lost, the backend thread reconnects every second.

## Real-time model

`engine::new` returns a pair:

- `Engine` (audio thread): processes interleaved `f32` at 48 kHz, 1 or 2
  channels, in internal blocks of up to 1024 frames with preallocated buffers.
- `Controller` (normal thread): builds effect instances and sends them over.

Rules followed on the audio thread:

- **No allocation or deallocation.** A new chain arrives as a boxed `Chain`
  through an `rtrb` SPSC queue. Effects that keep the same id and kind are
  *moved* from the old chain into the new one (their state survives
  reordering). The old chain is pushed back through a second queue and freed
  by the controller. A new chain is only accepted when the return queue has
  room, so nothing is ever dropped on the audio thread.
- **No blocking locks.** Parameters, enable flags and meters are atomics
  (`AtomicF32`). The backend shares the engine with its main loop through a
  mutex that the audio callback only `try_lock`s; it is uncontended in normal
  operation because only one capture stream exists at a time.
- **No I/O or logging.** Diagnostics are atomic counters read by the service.
- When no capture callback runs (no device), the main loop "pumps" the engine
  with empty blocks every 100 ms so chain updates are still applied. It only
  does so after the callback has been idle for a full period.

### Click prevention

- Each chain slot crossfades between processed and dry signal over 10 ms when
  enabled/disabled. The dry path is delayed by the effect's latency, so
  bypassing RNNoise or the limiter does not shift audio in time.
- A fully bypassed effect is not processed. When it is re-enabled, the slot
  stays dry for the effect's latency before fading in, so audio left in the
  effect's buffers from before the bypass is never heard.
- Effects that are removed are faded out (disabled, then removed 15 ms later).
- Gain ramps per block; dynamics effects smooth their gain per sample.
- Reordering is applied instantly and can produce a small discontinuity.

### Latency

Reported in Settings → Diagnostics: capture quantum + effect latency
(RNNoise 480 frames, Hush 2119 frames, limiter 240 frames) + audio queued
between streams.

## Effects

All effects implement `effects::Processor` (`process`, `latency`, `meter`) and
declare parameters as static `ParamSpec`s (key, range, default, unit, kind).
The UI renders controls from these specs, so adding an effect needs no UI code
(the equalizer has a custom editor for its response curve).

| Effect | Notes |
|--------|-------|
| RNNoise | FFI to system `librnnoise`; 480-sample frames, one frame latency regardless of host block size; wet/dry mix; optional VAD gate with grace period |
| Hush | FFI to the official `libweya_nc` (DeepFilterNet-based, 16 kHz), loaded at runtime; suppresses noise and competing speakers; attenuation limit, wet/dry mix; see below |
| Gain | ±36 dB, per-block ramp |
| Noise gate | Linked peak detector, hysteresis, hold, attack/release, range |
| Equalizer | 5 RBJ biquad bands (peak, shelves, low/high pass); coefficients recomputed only on change |
| Compressor | Feed-forward, soft knee, log-domain attack/release, makeup |
| Limiter | 5 ms lookahead, hold, release, final clamp to the ceiling |

### Hush

Hush (Weya AI) is a DeepFilterNet3 model fine-tuned to keep one speaker and
suppress background noise and other voices. Otozane uses the project's own C
library, `libweya_nc`, built from the pinned Hush sources by
`scripts/build-hush.sh`, and its `advanced_dfnet16k` model bundle.

- **Loading.** `EffectKind::create` loads the library with `libloading` and
  the model once per process (on the controller thread), then creates one
  session per channel. The files are looked up in `$OTOZANE_HUSH_DIR`,
  `<binary dir>/../lib/otozane/hush` (packages, `~/.local` installs) and
  `third_party/hush` in the source tree (development). If they are missing or
  fail to load, the slot becomes a pass-through with the error on its card;
  its parameters are kept in the profile. A failed load is not cached.
- **Worker thread.** `weya_nc_process_frame` allocates on every call (tract
  creates tensors per run), so it never runs on the audio thread. The audio
  thread sends 10 ms frames (160 samples at 16 kHz) to a `hush` worker thread
  through an `rtrb` queue, wakes it with `Thread::unpark`, and reads finished
  frames from a second queue. Frames carry sequence numbers; a frame that has
  not come back when it is due is played dry (the effect's own latency-aligned
  dry signal) and its late result is dropped, so latency never changes.
- **Budget.** A frame completed during one host callback can only be used in a
  later one, so results are read 345 samples at 16 kHz (1035 at 48 kHz) after
  the frame is complete: more than one 1024-frame quantum. This is why the
  capture stream caps the quantum at 1024. With larger forced quanta Hush
  falls back to dry audio for the frames that miss the deadline.
- **Resampling.** 48 kHz to 16 kHz and back with a 127-tap linear-phase
  windowed-sinc low-pass (7.6 kHz, Blackman), polyphase on the way up, with
  state kept across callbacks. The library's own resampler is not used: it
  interpolates each frame on its own (linear, endpoints stretched), which is
  not continuous across frames.
- **Latency** is constant: 3 × (160 frame + 345 budget + 160 model STFT delay)
  + 124 filter delay = 2119 frames (44 ms). The 160-sample model delay was
  measured with the real model.
- **Channels.** Each channel has its own session (stereo costs twice the CPU).
- **Parameters.** Attenuation limit (dB, 100 = unlimited) and Mix. The
  library applies its attenuation limit as `enhanced * (1 - lim) + noisy * lim`
  with the noisy signal aligned to the output, and only at session creation.
  Otozane computes the same blend against its latency-aligned dry signal, so
  the limit changes live without recreating sessions (and the noisy part
  keeps its content above 8 kHz). Mix then scales the wet amount. The meter
  shows the model's estimate of the input's speech-to-noise ratio.
- **Failure modes.** `libweya_nc` is built with `panic = "abort"`: a panic
  inside it would stop the service (like a crashing plugin).

## Service and IPC

`otozaned` owns the engine and backend. It listens on
`$XDG_RUNTIME_DIR/otozane.sock` (mode 0600) and speaks newline-delimited JSON
(`protocol::Request` / `protocol::Response`). Requests over 1 MiB are rejected.
A second instance refuses to start while the socket answers.

State is saved under `$XDG_CONFIG_HOME/otozane/`. Changes to the active
profile are written within 100 ms. Unparseable `config.toml` is moved to
`config.toml.bak` instead of being overwritten. Profile names are validated
before being used as file names.

Profile export and import go through the UI: it asks the service for a
profile (`get_profile`) and writes the TOML file the user picks in the system
file dialog, or reads a file and sends it (`import_profile`). The service never
reads or writes paths chosen by a client. An imported profile is stored under
the file name (with ` 2`, ` 3`... if taken) and is not activated. Plugin
entries keep their bundle paths; a plugin missing on this machine shows a load
error on its card.

## UI

`otozane` is an egui/eframe app. A background thread polls status at ~30 Hz and
forwards requests; the render loop never blocks on IPC and never processes
audio. Edits apply to a local copy immediately, and incoming status does not
overwrite it for 500 ms after the last edit, so sliders stay responsive.

Styling is monochrome Material 3 (fixed palette, tonal surfaces, M3 controls), using
bundled Roboto and a Lucide icon font subset. egui's default fonts stay as
fallbacks for scripts Roboto lacks.

## Plugin hosting

`otozane-plugins` hosts VST3 effects (Linux bundles,
`Contents/<arch>-linux/*.so`) through the `vst3` bindings crate.

- **Scanning** looks in `~/.vst3`, `/usr/lib/vst3`, `/usr/local/lib/vst3` and
  the `plugin_dirs` setting. Each bundle is loaded by a separate
  `otozaned --scan BUNDLE` process with a 20 s timeout, so a plugin that crashes
  or hangs while loading only fails its own entry. Results are cached in
  `plugins.json` in the config directory; the service scans on first start and
  on request.
- **Instances run inside `otozaned`.** There is no crash isolation: a plugin
  that crashes during processing stops the service (PipeWire then removes the
  virtual microphone).
- A chain entry with `kind = "plugin"` names the plugin (format, bundle path,
  class id). The engine controller asks the service's factory to build it; a
  plugin that fails to load becomes a pass-through slot and the error is shown
  on its card.
- Setup order: initialize, connect component and controller, set bus
  arrangements (mono/stereo main bus, all buses active, aux inputs get
  silence), `setupProcessing` (realtime, 32-bit, 1024 frames), then restore
  state, then `setActive`. State is restored after `setupProcessing` because
  some plugins (LSP) divide by the sample rate in `setState`.
- **Parameters** are normalized 0..1 and keyed by VST3 parameter id. Values
  reach the plugin as one-point parameter queues per block, built in
  preallocated host objects (no allocation on the audio thread from our side;
  what the plugin does inside `process` is up to the plugin). Hidden,
  read-only and bypass parameters are not exposed; bypass uses the chain's
  crossfade.
- **State** (component + controller) is stored hex-encoded in the profile and
  refreshed whenever the profile is saved.
- **Latency** is read when the plugin is activated (capped at one second).
  Later `restartComponent` latency changes are not applied yet.
- Controller calls (parameter text, state) come from service threads under
  the service lock, not from one dedicated UI thread.
