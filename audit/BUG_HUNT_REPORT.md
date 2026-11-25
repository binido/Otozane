# Otozane bug hunt report

- Baseline commit: `d69c9cd14427bbe11d82d4ae03b16eeb8b37ba05`
- Audit branch: `audit/bug-hunt`
- Date: 2026-11-25
- Scope: Otozane source, controlled local tests, and directly relevant dependency
  sources. No external reconnaissance, third-party testing, or system changes.
- Authorization: the repository owner requested a defensive audit of this project.

## Executive summary

The audit found and fixed 28 confirmed defects and hardening gaps, in 26 fix/test commits (plus this documentation commit).
The most consequential were a use-after-free in the Linux PipeWire backend's
listener teardown, permanent NaN poisoning of built-in DSP state by one bad
sample, silent profile data loss on case-insensitive macOS volumes, plugin state
not being restored when switching profiles, a bypass/removal click bug in the
engine, and an unbounded scanner output that a hostile plugin could use to
exhaust the service.

Security-relevant open items are documented rather than patched because they need
an architectural or platform change: the HAL driver's writer check is a
self-declared bundle ID, the macOS service is ad-hoc signed and inherits its
microphone TCC grant into a user-controlled LaunchAgent, the Unix-socket fallback
lacks peer-credential checks, and update authenticity derives from the same
mutable GitHub release.

Linux runtime audio (PipeWire/WirePlumber), live macOS audio and driver
installation, TCC behavior, and installer/update execution were **not** run on
this host. Those findings are source-level or static-only and are marked as such.

## Environment

| Item | Value |
|---|---|
| OS | macOS 27.2.0 (Darwin 27.2.0), arm64 |
| Toolchain | rustc/cargo 1.96.0, edition 2024 |
| Native inputs present | `third_party/rnnoise/librnnoise.a`, `third_party/hush` (library + model), `third_party/ysfx` |
| Provider setup | Existing gateway configuration routing Claude model aliases to `deepseek-v4.1-flash` |

## Methodology

Primary agent:

- `claude-bughunter:bb-methodology` — adapted to an authorized internal desktop
  audit. Hygiene is context, not a finding; only impact-demonstrated defects are
  reported.
- `claude-bughunter:triage-validation` — 7-Question Gate adapted so Q1 is a
  concrete local reproduction, Q3 is an in-scope subsystem, Q4 treats the
  same-user local boundary explicitly, Q5 checks documented behavior, and the
  pre-severity gate is applied before Medium or higher.
- `claude-bughunter:evidence-hygiene` — no microphone audio, private
  configuration, credentials, or agent transcripts were collected or shared.
  Tests used disposable directories and synthetic signals.
- `report-writing`, `recon-scope-triage`, `supply-chain-attack-recon`,
  `redteam-mindset`, `hunt-exceptional-conditions` were inspected for reporting,
  scope, and supply-chain questions.

Delegated agents were required to load the installed skills and return candidate
findings with file:line evidence, reproduction proposals, false-positive
explanations, and coverage limits. Every accepted finding was re-read in the
source by the primary agent; agreement between agents was not treated as
evidence.

## Findings fixed

Each entry states the defect, impact, fix, and validation. Line numbers are from
the baseline; the fix commits are on `audit/bug-hunt`.

| ID | Severity | Source | Fix commit | Summary |
|---|---|---|---|---|
| F-01 | Medium | Primary + BH-05 | `415dd74` | Disabling a latent effect faded into stale silence |
| F-02 | Medium | Primary + BH-05 | `eb279f7` | Effect removal could swap before the fade ran |
| F-03 | Medium | BH-08 | `1ffb9e9` | Two services could start together and unlink each other's socket |
| F-04 | Medium | Primary + BH-08 | `b058e3f` | Case-only profile names silently overwrote profiles |
| F-05 | Medium | BH-08 | `5a12392`, `931ad2b` | `nan`/`inf` parameters broke `Status` and looped the UI |
| F-06 | Low | Primary + BH-08 | `5a12392` | `next_id` overflow on `u64::MAX` |
| F-07 | Medium | BH-08 | `1b213db`, `931ad2b` | Profile switch ignored saved plugin state |
| F-08 | Medium | BH-08 | `931ad2b` | Malformed active profile overwritten with defaults |
| F-09 | Low | BH-08 | `931ad2b` | Unreadable config/profile files were not preserved |
| F-10 | Low | BH-08 | `931ad2b` | Failed profile saves returned `Ok` and lost edits |
| F-11 | Medium | Primary + BH-07 | `7cb8de8` | Plugin scanner stdout was unbounded in memory |
| F-12 | **High** | BH-04 | `26848f7` | PipeWire listeners dropped after their objects (use-after-free) |
| F-13 | Low | BH-04 | `26848f7` | Monitor ring replayed stale audio on re-enable |
| F-14 | Medium-Low | BH-04 | `26848f7` | Automatic input could select synthetic `Audio/Source` nodes |
| F-15 | Low | BH-04 | `26848f7` | Feed stream lacked `node.autoconnect=false`; stale status after reconnect |
| F-16 | Medium | BH-05 | `abf0bbb` | One non-finite sample poisoned DSP state permanently |
| F-17 | Medium | BH-10 | `c19390b` | Release build jobs held a write token |
| F-18 | Medium | BH-10 | `b51faa7` | Privileged package manager resolved through PATH; tar payload not validated |
| F-19 | Medium-Low | BH-07 | `244866b` | `.jsfx` files were parsed in-process without a size bound |
| F-20 | Low | BH-07 | `244866b` | Non-UTF-8 plugin paths panicked the scanner / could not reload |
| F-21 | Low | BH-07 | `244866b` | Zero-channel VST3 output bus panicked the audio thread |
| F-22 | Low | BH-07 | `244866b` | VST3 transport context reported "stopped" |
| F-23 | Low | BH-07 | `244866b` | `MemStream` unchecked read pointer / unbounded seek |
| F-24 | Low-Med | BH-07 | `f7402fa` | Failed plugin rebuild kept the previous plugin's control |
| F-25 | Low | BH-07 | `f7402fa` | Corrupt `plugins.json` suppressed rescanning; write not atomic |
| F-26 | Low-Med | BH-03 | `8345f67` | CoreAudio treated xrun as fatal and logged on the RT thread |
| F-27 | Low | BH-03 | `a8dfa21` | HAL read boundary aliased a slot being written; driver entry could throw |
| F-28 | Low-Med | Primary | `6ae55fe` | Non-finite plugin parameter metadata could panic or break `Status` |

### Core engine

**F-01 — disabling a latent effect faded into stale silence (Medium, High).**
`Slot::process` captured the delayed dry path only during a crossfade. While an
effect was fully wet the delay was never primed, so disabling Hush (2119 frames)
faded through the 480-frame fade to silence and then jumped to dry audio. Fixed
by priming the dry delay on every active block. Regression test
`disabling_a_latent_slot_keeps_the_dry_path_aligned` failed before the fix.

**F-02 — removal could beat the fade (Medium, High).**
The service slept 15 ms after disabling removed effects, but a capture callback
may carry a 1024-frame block (21.3 ms). Fixed with a named 25 ms `FADE_OUT_WAIT`;
a unit test pins the invariant to `MAX_BLOCK`.

**F-28 — non-finite plugin parameter metadata (Low-Medium, High).** A plugin's
`ParameterInfo`/slider range can report a NaN or infinite default. A NaN bound
makes `f32::clamp` panic during chain build, and a NaN default serializes as
`null` in `Status`, reproducing the F-05 UI loop. `ParamDesc::sanitize` now
normalizes bounds/default, and the controller applies it to every description.
Tests: `non_finite_descriptions_are_sanitized`,
`non_finite_plugin_parameters_are_sanitized`.

**F-16 — bad samples poisoned DSP state (Medium, High).**
One `inf` from a hosted plugin made the compressor's gain reduction NaN forever;
the gate stuck open; the EQ state became NaN. A huge finite value such as
`f32::MAX` also overflows a biquad. The engine now replaces non-finite samples
with silence and clamps extreme finite samples to ±1e6 at the input and after
every slot. Tests cover a bad input sample, a plugin emitting `inf` before a
compressor, and a plugin emitting `f32::MAX` before a +24 dB EQ band.

### Service, IPC, configuration

**F-03 — duplicate-service startup race (Medium, High).** `connect`, `remove_file`,
and `bind` were not atomic; two starts could both listen and one socket could be
unlinked. An exclusive `File::try_lock` beside the socket now serializes startup
for the process lifetime. Test: `a_second_instance_cannot_take_the_lock`.

**F-04 — case-only profile overwrite (Medium, High).** APFS is case-insensitive;
`stream` replaced `Stream.toml`. New saves reject a case-insensitive collision;
import and the preset picker choose a free case-insensitive name. Tests:
`profile_names_collide_case_insensitively`,
`preset_names_do_not_collide_by_case`.

**F-05 — non-finite parameters broke the UI (Medium, High).** TOML accepts
`nan`/`inf`; serde_json emits `null`, which cannot parse back into `f32`, so every
`Status` failed and the UI reconnect-looped. `EffectConfig::sanitize` drops
non-finite values on load, import, and chain build.

**F-06 — id overflow (Low, High).** `next_id` now uses checked arithmetic and
returns the first unused id instead of panicking or returning a duplicate.

**F-07 — profile switch ignored plugin state (Medium, High).** The engine reused
a plugin slot when id, kind, and `PluginRef` matched, so the loaded profile's
opaque state was never applied. `Controller::restore_chain` forces plugin entries
through the factory while reusing built-ins; `switch_profile` uses it. Test:
`restoring_a_chain_recreates_plugins_from_saved_state`.

**F-08/F-09 — unreadable profiles/config were discarded (Medium/Low, High).** A
malformed active profile was replaced by the default chain on the first flush;
invalid UTF-8 or permission errors on `config.toml` were treated as first run.
Non-`NotFound` profile errors are now moved to `.bak`, empty profiles are treated
as truncated, config read errors are backed up, and the original is preserved.
Test: `unreadable_config_and_profile_are_preserved`.

**F-10 — save failures reported success (Low, High).** `flush` only logged.
`SaveProfileAs` and `switch_profile` now abort when the dirty flag remains,
instead of returning `Ok` and dropping pending edits.

### PipeWire (Linux)

**F-12 — listener/stream drop order (High, Medium-High).** `Capture`, `Main.monitor`,
and `Main._metadata` declared the PipeWire object before its listener. Rust drops
fields top-to-bottom, so `pw_stream_destroy`/`pw_proxy_destroy` freed the object
first. `pw_stream_destroy` calls `spa_hook_list_clean`, which unlinks each hook
but does not reinitialize it; `StreamListener::drop` then calls `spa_hook_remove`
through stale links and writes into freed memory. Confirmed against PipeWire's
`stream.c`/`hook.h`. Listeners are now declared first. This path triggers on every
mute, input/channel switch, device removal, monitor toggle, and reconnect. It is
classified High rather than Critical because no runtime sanitizer run was
possible; the write-after-free mechanism is confirmed from both Rust and
PipeWire C source, and the practical impact is a crash or heap corruption in a
local daemon, not a privilege boundary crossing.

**F-13/F-14/F-15 — lifecycle and routing hardening (Low/Medium).** The monitor
ring is drained when monitoring stops; a source without a `device.id` (or with
`node.virtual=true`) is treated as virtual so Automatic stays on hardware; the
feed stream sets `node.autoconnect=false`; reconnect clears stale devices and
capture state.

### Plugin hosting

**F-11 — scanner output bound (Medium, Medium).** The parent collected a helper's
stdout for up to 20 s with no limit; a plugin can print to stdout. Capped at
1 MiB; the helper is killed and the entry errors. Test:
`scanner_output_is_bounded`.

**F-19/F-20 — JSFX size and path handling (Medium-Low/Low).** `.jsfx` files were
parsed in-process with no size limit (only extensionless scripts were capped);
non-UTF-8 paths went through `env::args` (panic) and lossy conversion. Both are
now bounded/rejected with an error. Test: `oversized_scripts_are_skipped`.

**F-21/F-22/F-23 — VST3 host contracts (Low).** Reject a zero-channel main output
bus and guard `process`; set `kPlaying`, tempo, time-signature, and continuous-time
valid bits; null-check and bound `MemStream` reads/writes/seeks, returning
`kOutOfMemory` instead of aborting. Test:
`mem_stream_rejects_bad_buffers_and_huge_seeks`.

**F-24/F-25 — control and cache consistency (Low-Medium).** A failed plugin
instantiation now removes any old control for that id, so one plugin's state is
never saved into another's entry. `plugins.json` is written through a temp file;
an unparseable cache is removed so startup rescans.

### macOS Core Audio and HAL driver

**F-26 — xrun treated as fatal (Low-Medium, High).** cpal reports overloads on the
RT thread; the error callback logged and marked the stream dead, causing teardown
and reopen. Only terminal errors retire the stream, and the callback no longer
logs.

**F-27 — HAL read boundary and exception boundary (Low, High).** The reader
accepted a frame exactly `RingFrames` behind the writer; the power-of-two mask
aliases it onto the slot the writer is about to overwrite. Changed `<=` to `<`.
`CreateDriver()` is now wrapped in `try/catch` so a C++ exception cannot unwind
through the Core Audio C ABI.

### Update and CI

**F-17 — release token scope (Medium, High).** The workflow granted
`contents: write` to build jobs that execute repository/dependency code. It now
grants `read` by default, `write` only to `publish`, and checkout does not persist
credentials.

**F-18 — privileged paths and payload validation (Medium, High).** `pkexec` and
the package managers are invoked by absolute path, so a writable PATH entry
cannot supply a root program. macOS security tools and `osascript` are absolute.
The Linux tarball uses `--no-same-owner`, requires a real archive root, and
rejects symlinked `otozane`/`otozaned` binaries.

## Outstanding confirmed risks (not fixed)

These are source-confirmed or strongly evidenced but need a platform test or an
architectural change. They are not counted as fixed. Two are already documented
by the project: `DEVELOPMENT.md` states that update trust rests on HTTPS and
`SHA256SUMS`, not the ad-hoc signature (C-22), and `PLAN.md` lists runtime plugin
latency changes as unsupported. The driver claim was tested against an ordinary
foreign writer, not against a process presenting the service bundle ID (C-01).

| ID | Severity | Confidence | Area | Issue | Recommended fix |
|---|---|---|---|---|---|
| C-01 | Medium | Medium | HAL | Writer authorization is a self-declared bundle ID; a process can present it | Verify `SecCode` identity/Team ID against a pinned requirement, or correct the docs |
| C-02 | Medium | Medium | macOS TCC | Ad-hoc service in a user LaunchAgent inherits its mic grant; `DYLD_*` injection is possible | Hardened runtime, SMAppService/root job, rewrite the plist instead of preserving unknown keys |
| C-03 | Low | High | CoreAudio | Mute leaves queued audio in the output ring; the virtual mic is not immediately silent | Shared mute flag + drain the rings in the output callback/on stream drop |
| C-04 | Low | Medium | CoreAudio | An aggregate device containing the virtual mic bypasses the self-input guard | Inspect aggregate sub-devices; include `AutoAggregate` in `is_virtual` |
| C-05 | Low | High | Driver install | Unverified root copy; working driver removed before the new copy | Verify signature/CDHash, stage, swap with rollback |
| C-06 | Info | Low | CoreAudio | TCC denial is not a distinct state; a denied capture can look Running with silence | Query authorization status and map it to `CaptureState` |
| C-07 | Low | High | HAL | `ring_` is read/written non-atomically (C++ data race) | Atomic cells or a seqlock; the frame at the boundary is now excluded |
| C-08 | Low | Medium | IPC | Fallback socket path is predictable; chmod happens after bind; no peer-credential check | Private 0700 dir, mode at creation, `SO_PEERCRED`/`LOCAL_PEERCRED` |
| C-09 | Low | High | IPC | Unbounded idle connections; accept errors are swallowed | Read/write timeouts, connection cap, backoff on accept errors |
| C-10 | Low | Medium | Persistence | `write_atomic` does not fsync | `sync_all` + parent-dir fsync |
| C-11 | Info | High | Persistence | Config/profile files are created 0644/0755 | 0600/0700 if plugin state is private |
| C-12 | Low | High | Startup | Duplicate effect ids in a hand-edited active profile still abort startup | Validate before building; back up and use defaults |
| C-13 | Medium | High | CoreAudio/Hush | A callback longer than ~1036 internal frames makes Hush miss every deadline | Cap the resampled callback to ≤1024 output frames, or lengthen the budget |
| C-14 | Low-Med | Medium | Engine | Inserting a latency effect inserts a gap equal to its latency | Chain-level crossfade or document the accepted gap |
| C-15 | Low | High | DSP | No denormal flush; low-frequency/high-Q EQ after silence can spike x86 CPUs | Flush tiny state or set FTZ/DAZ on the audio thread |
| C-16 | Info | Medium | DSP | RNNoise/Hush model latency is not measured against the real libraries | Impulse/cross-correlation tests with the real model/library |
| C-17 | Low | Medium | DSP | EQ keeps biquad state across Off→On/type changes | Reset the band state or crossfade |
| C-18 | Info | High | DSP | Limiter clips extreme transients rather than limiting them pre-clamp | Documented safety-net behavior; no ceiling violation |
| C-19 | Low | High | Plugins | Plugin stdout contaminates the scanner JSON | Redirect fd 1 around loading, write results on a private fd |
| C-20 | Low | High | Plugins | VST3/JSFX state-restore failures are silent | Check results and surface/log a rejection |
| C-21 | Info | High | Plugins | `ModuleExit` is skipped when the factory export is missing | Call `ModuleExit` on that error path |
| C-22 | Medium | High | Update | Checksum/signature/package trust all derive from the same mutable release; macOS signature is ad-hoc, no signer identity | Signed manifest with a pinned key; Developer ID + notarization and signer check |
| C-23 | Low-Med | High | Update | macOS bundle swap has no crash/power-loss recovery | Journal/marker, predictable backup, startup reaper, signal trap |
| C-24 | Low | High | UI | Render thread blocks on `systemctl`/`launchctl` | Move autostart work to the background client thread |
| C-25 | Low | High | UI | Enabling stop-on-close deletes autostart settings; disabling does not restore them | Persist and restore, or warn |
| C-26 | Low | Medium | Update | A failed Linux update can kill a Holding daemon and not restart it | Treat Holding as running for restore |
| C-27 | Low | Medium | Update | Linux archive member types/traversal rely on the host tar | List members and reject links/traversal before extraction |
| C-28 | Low-Med | High | CI | Actions/container/rustup are referenced by mutable tags/versions | Pin actions to commit SHAs, container digest, pinned toolchain |
| C-29 | Low | Medium | Plugins | On a persistent Busy build failure, new plugin controls can be installed before the chain is applied | Collect controls in a temporary map and merge after success |
| C-30 | Low-Med | Medium | PipeWire | Duplicate `node.name` values break removal recovery and can bind the wrong node | Track the selected global id, not only the name |
| C-31 | Medium-Low | Medium | PipeWire | A partial link creation failure or server-side link destruction leaves `links` non-empty and blocks re-linking | Link listeners or retry both FL/FR links |
| C-32 | Low | Low-Med | PipeWire | The stale-audio skip threshold can exceed ring capacity with an extreme quantum, leaving the ring full and permanently behind | Clamp `keep`/`capture_quantum` |
| C-33 | Low | High | IPC/UI | `Status` carries the full chain, including opaque plugin state, at ~30 Hz; a large state produces heavy IPC and CPU | Send state only on change/on demand, or keep it client-side |
| C-34 | Low-Med | Medium | Engine/plugins | Profile restore re-instantiates plugins at full wet; a latency plugin inserts a gap and skips the fade | Apply saved state to the live instance or crossfade the swap |
| C-35 | Low | Med-High | Service/engine | Removal fade waits a fixed 25 ms; a callback period above that (large macOS device buffer) can still skip the fade | Acknowledge fade completion from the audio thread |
| C-36 | Low-Med | Medium | PipeWire | Automatic input treats a source without `device.id`/`node.virtual` as virtual; unverified for all hardware | Verify on ALSA/USB/Bluetooth; fall back to class + `node.virtual` if a real mic is excluded |

## Independent review follow-ups

Two fresh review sessions examined the final diff. The core/service review
(BH-R1) found no regression in the engine priming/sanitization, the instance
lock, or the id allocator. It found residual profile issues, which were fixed:
filesystem-level (Unicode/case) collision detection for profile names, recovery
from an invalid `active_profile`, reporting plugin-state capture failures, and
sanitizing imported profiles in the UI. Its two remaining design concerns are
recorded as C-34 (plugin-latency gap on profile restore) and C-35 (fixed fade
wait not acknowledged by the audio thread). The platform review (BH-R2) found
no type or behavior error in the listener drop-order fix, CoreAudio xrun
handling, HAL boundary, VST3 checks, scanner cap, or CI permissions. It found
that the PipeWire reconnect reset hid the error (fixed), that the JSFX size cap
did not cover profile-named scripts (fixed), and that the tar layout check
still followed symlinks for non-binary entries (fixed). Its unverified concern
about the `device.id` heuristic is C-36.

## False positives and rejected candidates

- **VST3 negative latency (BH07-5).** The `vst3` binding returns an unsigned
  latency; negative values cannot occur and `.max(0)` was unnecessary. Clippy
  caught the change and it was reverted. Not a defect.
- **Non-finite parameter transport over IPC.** JSON cannot express NaN/Inf, so
  this is not an IPC injection path; the defect was TOML-sourced values echoed in
  `Status` (F-05).
- **1 MiB request limit / JSON framing.** `Take` is recreated per line; a partial
  line at EOF is rejected and the client treats EOF as a disconnect. No smuggling
  or desync found.
- **Duplicate effect ids over IPC, `channels` outside 1..=2, profile path
  traversal, client-supplied file paths.** Checked and guarded. Imported profiles
  are not activated on import.
- **Driver `bytesCount` tail / timestamp choice / `GetPropertyData`
  system-default scope quirk.** Not reachable or no demonstrated caller,
  according to the HAL audit.
- **Linear resampler aliasing.** An explicit `shortcut:` in the code, not a hidden
  defect.
- **A 2048-frame `Slot::process` test panic.** A test-harness mistake; the engine
  always splits host audio into 1024-frame blocks and `Slot` is private.

## Coverage and limitations

- Linux PipeWire runtime, link negotiation, quantum behavior, and stop-on-close
  silence were not executed. PipeWire code is `cfg(target_os = "linux")` and was
  **not type-checked** on this macOS host; rustfmt parsed it.
- macOS audio capture/output, driver installation/removal, `coreaudiod`, launchd,
  and TCC were not executed. Driver changes were not rebuilt here.
- Updater/installer execution, package-manager/polkit flows, and update recovery
  were not executed.
- No Miri, Loom, ASan/TSan, fuzzing, or profiling. Unsafe/FFI paths were reviewed
  statically; real-library latency (RNNoise/Hush) was not measured.
- `cargo audit`/`cargo deny` are not installed; dependency advisories were not
  scanned. Version/CI review is in `COVERAGE.md`.
- The workspace declared `rust-version = "1.85"`, but `egui`/`eframe` 0.37
  requires 1.95 and the `File::try_lock` startup lock requires 1.89. The
  manifest and build instructions now declare 1.95. Builds use the stable
  toolchain (1.96 here).
- VST3 plugins were not loaded; JSFX integration tests ran. In-process plugin
  crashes and hangs remain a documented limitation.

## Verification

`VALIDATION.md` records the commands and results. The workspace build, format,
clippy, and test suite pass after the fixes. New regression tests fail on the
behavior they cover where a pre-fix run was possible, and pass after the fix.
