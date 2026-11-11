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
- **Output stream** is not autoconnected; the backend links its ports to the
  virtual microphone's input ports itself (client-owned links), because
  WirePlumber will not link a playback stream into a source node.
- The two sides can run on different graph drivers (and quanta). A lock-free
  ring buffer decouples them. The playback side discards stale audio beyond
  one capture quantum, so latency stays bounded after idle periods or clock
  drift. There is no resampling; drift is corrected by dropping frames.
- "Automatic" input follows the system default source (the `default`
  metadata's `default.audio.source`), switching live when it changes. A virtual
  default (our own microphone, EasyEffects' output) is skipped in favor of the
  first physical device that looks like a microphone (not a line input).
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
- Effects that are removed are faded out (disabled, then removed 15 ms later).
- Gain ramps per block; dynamics effects smooth their gain per sample.
- Reordering is applied instantly and can produce a small discontinuity.

### Latency

Reported in Settings → Diagnostics: capture quantum + effect latency
(RNNoise 480 frames, limiter 240 frames) + audio queued between streams.

## Effects

All effects implement `effects::Processor` (`process`, `latency`, `meter`) and
declare parameters as static `ParamSpec`s (key, range, default, unit, kind).
The UI renders controls from these specs, so adding an effect needs no UI code
(the equalizer has a custom editor for its response curve).

| Effect | Notes |
|--------|-------|
| RNNoise | FFI to system `librnnoise`; 480-sample frames, one frame latency regardless of host block size; wet/dry mix; optional VAD gate with grace period |
| Gain | ±36 dB, per-block ramp |
| Noise gate | Linked peak detector, hysteresis, hold, attack/release, range |
| Equalizer | 5 RBJ biquad bands (peak, shelves, low/high pass); coefficients recomputed only on change |
| Compressor | Feed-forward, soft knee, log-domain attack/release, makeup |
| Limiter | 5 ms lookahead, hold, release, final clamp to the ceiling |

## Service and IPC

`otozaned` owns the engine and backend. It listens on
`$XDG_RUNTIME_DIR/otozane.sock` (mode 0600) and speaks newline-delimited JSON
(`protocol::Request` / `protocol::Response`). Requests over 1 MiB are rejected.
A second instance refuses to start while the socket answers.

State is saved under `$XDG_CONFIG_HOME/otozane/`. Changes to the active
profile are written within 100 ms. Unparseable `config.toml` is moved to
`config.toml.bak` instead of being overwritten. Profile names are validated
before being used as file names.

## UI

`otozane` is an egui/eframe app. A background thread polls status at ~30 Hz and
forwards requests; the render loop never blocks on IPC and never processes
audio. Edits apply to a local copy immediately, and incoming status does not
overwrite it for 500 ms after the last edit, so sliders stay responsive.

Styling is libadwaita-inspired (palette, 12 px rounded cards, sidebar), using
bundled Space Grotesk and a Lucide icon font subset. Fallback fonts cover
scripts Space Grotesk lacks (e.g. Cyrillic device names).

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
