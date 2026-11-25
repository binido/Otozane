# Remediation record

Baseline: `d69c9cd14427bbe11d82d4ae03b16eeb8b37ba05`
Branch: `audit/bug-hunt`

## Resolved

| Finding | Commit | Change |
|---|---|---|
| F-01 | `415dd74` | Prime the latency-aligned dry path while a slot is wet |
| F-02 | `eb279f7` | Wait 25 ms (one 1024-frame block + margin) before removing effects |
| F-03 | `1ffb9e9` | Exclusive lock file serializes service startup |
| F-04 | `b058e3f` | Reject case-only profile collisions; case-insensitive import/preset names |
| F-05 | `5a12392`, `931ad2b` | Drop non-finite parameter values on load/import/build |
| F-06 | `5a12392` | Checked id allocation that skips used ids |
| F-07 | `1b213db`, `931ad2b` | Restore plugin state when switching profiles |
| F-08 | `931ad2b` | Back up an unreadable active profile; treat empty profiles as truncated |
| F-09 | `931ad2b` | Back up `config.toml` on any read error |
| F-10 | `931ad2b` | Report failed profile saves instead of returning `Ok` |
| F-11 | `7cb8de8` | 1 MiB cap on scanner stdout |
| F-12 | `26848f7` | Drop PipeWire listeners before their objects |
| F-13 | `26848f7` | Drain the monitor ring when monitoring stops |
| F-14 | `26848f7` | Treat source nodes without a backing device as virtual |
| F-15 | `26848f7` | `node.autoconnect=false`; clear stale status after reconnect |
| F-16 | `abf0bbb`, `c10cac5` | Replace non-finite and clamp extreme finite samples at slot boundaries |
| F-17 | `c19390b` | Read-only release token for build jobs; no persisted credentials |
| F-18 | `b51faa7` | Absolute privileged/security tool paths; bounded tar payload validation |
| F-19 | `244866b` | 1 MiB cap on `.jsfx` scripts |
| F-20 | `244866b` | Reject non-UTF-8 plugin paths |
| F-21 | `244866b` | Reject/guard a zero-channel VST3 output bus |
| F-22 | `244866b` | Set VST3 `kPlaying` and validity bits |
| F-23 | `244866b` | Bound `MemStream` reads/writes/seeks |
| F-24 | `f7402fa` | Remove stale plugin control on failed instantiation |
| F-25 | `f7402fa` | Atomic `plugins.json`; rescan when the cache is unparseable |
| F-26 | `8345f67` | Ignore xrun; no logging on the RT thread |
| F-27 | `a8dfa21` | Exclude the aliased HAL boundary frame; contain driver exceptions |
| F-28 | `6ae55fe` | Sanitize non-finite plugin parameter descriptions |

Also `0654818` adds the fade-wait invariant test.

## Outstanding confirmed risks (prioritized)

### P1 — platform trust and user-visible correctness

1. **C-01 HAL writer identity.** Bundle ID alone can be claimed by a local
   process; verify a signing identity/Team ID or soften the documented claim.
2. **C-02 macOS TCC inheritance.** Ad-hoc service in a user LaunchAgent can be
   launched with `DYLD_INSERT_LIBRARIES`; adopt hardened runtime, a root/SMAppService
   launch path, and rewrite the agent plist instead of preserving unknown keys.
3. **C-22 Update authenticity.** Sign the update manifest with a key the app
   pins; adopt Developer ID/notarization and verify the signer identity. Treat the
   current SHA256SUMS as integrity only.
4. **C-13 Hush on large CoreAudio buffers.** Cap the resampled callback to ≤1024
   output frames (fixed device buffer sized by rate) or lengthen the Hush budget.
5. **C-03 Mute tail.** Flush output/monitor rings on mute and stream recreation
   so the virtual microphone is silent immediately.

### P2 — boundary hardening and recovery

6. **C-08 Socket fallback.** Private 0700 directory, mode at creation, peer
   credential check.
7. **C-09 IPC connections.** Read/write timeouts, connection cap, accept backoff.
8. **C-12 Startup with duplicate ids.** Validate before `apply_chain`; back up and
   start with defaults.
9. **C-05 Driver install.** Verify, stage, swap with rollback; do not remove the
   working driver first.
10. **C-23 macOS update recovery.** Journal/marker, predictable backup, startup
    reaper, signal trap.
11. **C-24/C-25 UI.** Move autostart work off the render thread; preserve/restore
    login settings across stop-on-close.

### P3 — defense in depth

12. C-04 aggregate self-input; C-06 TCC status; C-07 atomic HAL ring; C-10 fsync;
    C-33 status payload size; C-34 profile-restore gap; C-35 fade acknowledgement;
    C-36 PipeWire hardware classification;
    C-11 file modes; C-14 insertion gap; C-15 denormal flush; C-16 model latency
    tests; C-17 EQ state reset; C-19 scanner stdout redirection; C-20 restore
    failures; C-21 `ModuleExit`; C-26 Holding restore; C-27 archive member
    validation; C-28 pinned actions/toolchain; C-29 temporary plugin control map.
13. C-18 limiter clipping is documented behavior, not a defect.

## Independent review follow-ups

The core/service regression review (BH-R1) accepted these residuals:

| Item | Disposition |
|---|---|
| BHR1-1: ASCII-only profile collision check | Fixed: filesystem `profile_exists`/`same_profile` and a filesystem-aware free-name picker (`799bdb7`) |
| BHR1-2: invalid `active_profile` blocks profile management | Fixed: startup falls back to `Default` (`799bdb7`) |
| BHR1-5: plugin-state capture failure ignored by `flush` | Fixed: `flush` returns the failure; save-as/switch abort (`799bdb7`) |
| BHR1-6: preset `SetChain` could land in the current profile after a rejected name | Fixed by suffixing instead of rejecting (`799bdb7`) |
| BHR1-7: imported `nan`/`inf` profiles rejected before sanitization | Fixed: UI sanitizes before sending (`58336de`) |
| BHR1-3: profile restore re-instantiates plugins at full wet | Open as C-34 |
| BHR1-4: fixed fade wait may be shorter than a macOS device buffer | Open as C-35 |
| BHR2-1: reconnect reset overwrote the error | Fixed: preserve the error (`49ea16f`) |
| BHR2-3: JSFX cap only covered discovery | Fixed: loader guard + report oversized scripts (`df1ca48`) |
| BHR2-4: tar layout followed symlinks for non-binary entries | Fixed: typed `symlink_metadata` checks (`ebacedd`) |
| BHR2-2: `device.id` heuristic may exclude a hardware source | Open as C-36; verify on Linux |

## Architectural improvements

- **HAL trust:** a real writer boundary needs a signed, non-ad-hoc service and a
  verification step against a pinned requirement. The current bundle-ID check is
  a filter, not a security boundary.
- **Update trust:** a detached signature over the update manifest (minisign,
  sigstore, or an Apple Developer ID signature with Team ID verification) would
  give the updater an independent root instead of the mutable release.
- **Socket trust:** a private runtime directory plus peer credentials would make
  the IPC boundary independent of umask and parent-directory permissions.
- **CoreAudio scheduling:** process the resampled capture in bounded output
  blocks tied to the device buffer, so Hush's sample budget always has a real
  inter-callback gap.
- **Plugin isolation:** in-process plugin crashes are a documented design
  limitation; out-of-process hosting would be the only real mitigation.

## Manual verification required

macOS:

- Rebuild the driver (`scripts/build-driver.sh`) and run `scripts/e2e-macos.sh`
  for capture, mute timing, monitor, device switching, and stop-on-close silence.
- Test the bundle-ID writer claim with a second app presenting the identifier,
  and test TCC inheritance with a hardened/non-hardened comparison.
- Install/remove the driver, including a failed copy, and verify the old driver
  survives.
- Test a large-buffer (1024+ frames) 16 kHz Bluetooth microphone with Hush and
  check for dry-fallback bursts.

Linux:

- Run `scripts/e2e.sh` with mute, input, channel, monitor, device removal,
  WirePlumber restart, and duplicate node names.
- Run the PipeWire backend under ASan to confirm F-12's listener teardown is
  clean and no other use-after-free remains.
- Test `pkexec` update paths from a PATH containing a writable directory.
- Verify the virtual microphone survives stop-on-close and receives silence.

Both:

- Exercise updater success, checksum failure, partial download, installer
  failure, and restart races in disposable environments.
- Run `cargo audit`/`cargo deny` and sanitizers where supported.
