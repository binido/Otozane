# Coverage matrix

Baseline: `d69c9cd14427bbe11d82d4ae03b16eeb8b37ba05`
Status values: complete / partial / static-only / not started.

| Subsystem | Task | Agent | Files inspected | Methods | Skills | Tests | Findings | Limitations | Status |
|---|---|---|---|---|---|---|---|---|---|
| Engine, chain swap, RT invariants | BH-01 | Primary | `otozane-core/src/engine.rs` | Full read; regression tests | bb-methodology, triage-validation | 12 engine tests | F-01, F-02, F-16, F-28 fixed | No callback timing measurement | complete |
| Built-in DSP | BH-05 | Primary + subagent | `dsp.rs`, `param.rs`, `effects/*.rs` | Full read; numpy re-implementation | triage-validation, evidence-hygiene | workspace tests | F-16 fixed; C-14/C-15/C-17/C-18 open | Real libraries not measured | partial |
| Hush | BH-05/06 | Primary + subagent | `effects/hush.rs` | Full read; model tests | triage-validation | Hush model tests pass | C-13, C-16 open | No load/deadline measurement | partial |
| RNNoise | BH-05/06 | Primary | `effects/rnnoise.rs` | Full read | — | unit tests pass | C-16 open | Vendor internals not audited | partial |
| Unsafe/FFI inventory | BH-02 | Primary + subagents | All `unsafe` sites; coreaudio, hush, rnnoise, vst3, jsfx | Inventory + targeted reads | triage-validation | F-12/F-16/F-23 tests | F-12, F-16, F-23 fixed | No Miri/sanitizers | partial |
| CoreAudio backend | BH-03 | Primary + subagent | `otozane-coreaudio/src/lib.rs` | Full read | triage-validation, evidence-hygiene, bb-methodology | build only | F-26 fixed; C-03/C-04/C-06 open | No live device/TCC tests | static-only |
| HAL driver | BH-03 | Primary + subagent | `Driver.cpp`; pinned libASPL source; built binary | Full read; read-only binary analysis | same | none | F-27 fixed; C-01/C-02/C-05/C-07 open | Driver not rebuilt | static-only |
| PipeWire backend | BH-04 | Primary + subagent | `otozane-pipewire/src/lib.rs` | Full read; dependency source | same | unit test only | F-12..F-15 fixed | Linux code not type-checked/runtime-tested | static-only |
| IPC/protocol/service | BH-08 | Primary + subagent | `protocol.rs`, `service/main.rs`, `service.rs`, `store.rs`, `client.rs` | Full read | triage-validation, evidence-hygiene, bb-methodology | service tests | F-03, F-05, F-08..F-10, F-24, F-25 fixed | No adversarial socket test | partial |
| Configuration/profiles | BH-08 | Primary + subagent | `config.rs`, `store.rs` | Full read | same | store/profile tests | F-04..F-06, F-08..F-10 fixed | No fault injection | partial |
| Plugins VST3/JSFX | BH-07 | Primary + subagent | `otozane-plugins/src/*`, tests, vst3/ysfx FFI | Full read | triage-validation, evidence-hygiene, bb-methodology | JSFX + MemStream tests | F-11, F-19..F-25 fixed; C-19..C-21, C-29 open | VST3 not loaded on macOS | partial |
| Updater/installers | BH-10 | Primary + subagent | `update*.rs`, workflow, packaging, scripts | Targeted reads | triage-validation, supply-chain-attack-recon | updater tests | F-17, F-18 fixed; C-22..C-28 open | No install execution | partial |
| UI async/state | BH-10/12 | Primary + subagent | `client.rs`, `app.rs`, `effects.rs`, tray/menubar | Targeted reads | triage-validation | 58 UI tests | C-24/C-25 open | No interactive UI run | partial |
| Concurrency/lifecycle | BH-11 | Primary | Engine, service, backends | Code review | triage-validation | F-03/F-12 tests | F-03, F-12 fixed | No Loom/TSan | partial |
| Performance/stability | BH-15 | Primary | Engine hot path, Hush timing | Static review | — | Hush bench not run | C-15 open | No profiling | partial |
| Dependencies/CI | BH-13 | Primary + BH-10 | Manifests, lockfile, workflow, scripts | Manifest/workflow review | supply-chain-attack-recon | YAML parse | F-17 fixed; C-28 open | `cargo audit`/`deny` unavailable | partial |
| Privacy/audio isolation | BH-14 | Primary + BH-03/04 | Backends, driver, README claims | Code review | triage-validation | none | C-01/C-02/C-03 open; F-14/F-15 fixed | No recording test | static-only |

## Coverage notes

- All six delegated assignments completed (BH-03, BH-04, BH-05, BH-07, BH-08,
  BH-10). None failed or was blocked.
- Two fresh independent regression reviews completed (BH-R1 core/service,
  BH-R2 platform). Their accepted residuals and fixes are in `REMEDIATION.md`.
- The primary agent independently re-read every accepted finding and the code
  behind each fix. Delegated agreement was not used as evidence.
- Linux-only modules (`otozane-pipewire`, `update_linux`) are `cfg`-disabled on
  this macOS host. They were parsed by rustfmt but not type-checked by a
  compiler; this is a real coverage gap for F-12..F-15 and F-18/F-27.
- The HAL driver was not rebuilt or installed. F-27 is source-level.
- The final workspace suite, clippy, and format check pass; see `VALIDATION.md`.
- Source-of-truth docs read: `AGENTS.md`, `README.md`, `ARCHITECTURE.md`,
  `DEVELOPMENT.md`, `BIG-PICTURE.md`, `PLAN.md`, `THIRD_PARTY.md`, the manifests,
  the lockfile, and the release workflow.
- `THIRD_PARTY.md` licenses match the bundled/material inputs; no license
  inconsistency was found in the reviewed components.
