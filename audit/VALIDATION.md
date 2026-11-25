# Validation record

Baseline commit: `d69c9cd14427bbe11d82d4ae03b16eeb8b37ba05`
Final branch: `audit/bug-hunt`
Host: macOS 27.2.0 (Darwin 27.2.0), arm64, Apple Silicon
Toolchain: rustc/cargo 1.96.0, edition 2024
Date: 2026-11-25

## Baseline (before fixes)

| Command | Result |
|---|---|
| `cargo build --locked` | PASS (exit 0) |
| `cargo fmt --all -- --check` | PASS (exit 0) |
| `cargo clippy --locked --workspace --all-targets` | PASS (exit 0, no diagnostics) |
| `cargo test --locked --workspace` | PASS (exit 0) |

The Hush model tests ran because `third_party/hush/libweya_nc.dylib` and the
model bundle are present. The Hush benchmark is `#[ignore]` and was not run.

## Final (after fixes)

| Command | Result |
|---|---|
| `cargo build --locked` | PASS (exit 0) |
| `cargo fmt --all -- --check` | PASS (exit 0) |
| `cargo clippy --locked --workspace --all-targets` | PASS (exit 0, no warnings) |
| `cargo test --locked --workspace` | PASS (exit 0; 113 tests: 41 core, 5 service, 6 plugin unit, 2 JSFX integration, 1 LSP, 58 UI) |
| `cargo test --release --locked -p otozane-core -p otozane-service` | PASS (41 core, 5 service) |
| Workflow YAML parse (`python3 -c 'yaml.safe_load(...)'`) | PASS |

The JSFX integration tests and the LSP test ran. The LSP test skips internally
when the fixture plugin is absent; on this host it reported success.

## Regression tests added

| Test | Covers | Pre-fix behavior |
|---|---|---|
| `engine::tests::disabling_a_latent_slot_keeps_the_dry_path_aligned` | F-01 | Failed with `dry path dropped out` |
| `engine::tests::restoring_a_chain_recreates_plugins_from_saved_state` | F-07 | Fails if `restore_chain` reuses the plugin |
| `engine::tests::non_finite_input_does_not_poison_a_builtin` | F-16 | Fails with NaN output |
| `engine::tests::non_finite_plugin_output_is_sanitized_before_the_next_slot` | F-16 | Fails with NaN output |
| `engine::tests::huge_finite_plugin_output_does_not_poison_a_builtin` | F-16 | Fails with a poisoned EQ state |
| `config::tests::non_finite_params_are_dropped_before_status_serialization` | F-05 | Fails to round-trip `Status` |
| `config::tests::next_id_stays_unique_and_finite_at_the_maximum` | F-06 | Panics/wraps |
| `tests::a_second_instance_cannot_take_the_lock` | F-03 | Second lock succeeds |
| `service::tests::free_profile_names_avoid_case_and_filesystem_collisions` | F-04 | Case/Unicode variants accepted |
| `service::tests::fade_out_wait_covers_a_full_audio_block` | F-02 | Pins the invariant |
| `store::tests::unreadable_config_and_profile_are_preserved` | F-08/F-09 | File overwritten |
| `app::tests::preset_names_do_not_collide_by_case` | F-04 | Case collision chosen |
| `lib::tests::scanner_output_is_bounded` | F-11 | Unbounded growth |
| `jsfx::tests::oversized_scripts_are_rejected_by_the_loader` | F-19 | Oversized script accepted on load |
| `jsfx::tests::oversized_scripts_are_listed_so_the_scan_can_report_them` | F-19 | Oversized script hidden from errors |
| `vst3::tests::mem_stream_rejects_bad_buffers_and_huge_seeks` | F-23 | Null read accepted / huge seek aborts |
| `engine::tests::non_finite_plugin_parameters_are_sanitized` | F-28 | NaN default reaches `Status`/clamp |
| `param::tests::non_finite_descriptions_are_sanitized` | F-28 | NaN bounds panic `clamp` |

Follow-up tests from independent review: `free_profile_names...` now covers
ASCII and macOS Unicode case collisions; `unreadable_config_and_profile...`
checks the backup bytes; `restoring_a_chain...` asserts the restored state
reaches the factory. The UI import sanitizer and the startup `active_profile`
fallback have no direct test (no fixture for the file-dialog path or startup).

The PipeWire reconnect, tar layout, and update_linux changes are Linux-only and
were parsed by rustfmt but not compiled or executed here. The JSFX loader guard
and discovery test ran.

Fixes without a deterministic test: F-02 timing (unit invariant only), F-10
(I/O failure injection not available), F-12..F-15 (Linux runtime), F-17/F-18
(CI/privileged execution not run), F-21/F-22/F-24/F-25/F-26/F-27 (device/plugin
or platform paths not exercised). These are reasons, not passes.

## Static analysis notes

- Clippy disproved one delegated candidate: the `vst3` binding returns an
  unsigned latency, so the proposed negative-latency fix was unnecessary and was
  reverted.
- `cargo audit` and `cargo deny` are not installed on this host; dependency
  advisories were not scanned.
- The workflow YAML was parsed after the permission change; the `on:` key is
  parsed as a boolean by YAML 1.1, which is expected.

## Not executed (environment limitations)

- `scripts/e2e.sh` (Linux PipeWire/WirePlumber): requires a Linux session.
- `scripts/e2e-macos.sh`: records the real microphone and touches audio
  devices; not run to avoid changing live system state.
- Driver build/install/remove, `coreaudiod` restart, launchd/TCC changes: not
  run. The driver source changes were not recompiled.
- Live update install, download, package manager, polkit, DMG mount/swap: not
  run.
- Miri: cannot execute FFI/platform paths.
- ASan/TSan/Loom/fuzzing: not run. The unsafe/FFI paths were reviewed
  statically; the PipeWire UAF was confirmed against the dependency's C source
  but not with a sanitizer.
- Linux-only modules (`otozane-pipewire`, `update_linux`) are `cfg`-disabled on
  macOS. They were parsed by rustfmt but **not type-checked** by a compiler on
  this host. This is the largest residual validation gap.
- Denormal performance on x86 Linux was not measured.
- RNNoise/Hush model latency was not measured with real signals.
