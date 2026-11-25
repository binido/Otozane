# Otozane big picture

<!-- open-steps:begin -->
_Measured 2026-11-25. Stages are dated where they stand; anything undated in this file is not measured._

## What this is

Otozane processes microphone audio and reduces background noise.
For calls, recordings, or streams, select **Otozane Microphone** in the app you use.
Otozane runs on Linux with PipeWire and on macOS 26 or later on Apple Silicon.
The `otozaned` background service processes the audio. [README.md](README.md)

GitHub published `v0.5.0` on 2026-11-22. [Release](https://github.com/binido/Otozane/releases/tag/v0.5.0)

## What is in it

The latest Open Steps report (2026-11-25) covers the audit; stages are dated where they stand.
Rows group files that serve the same purpose and have the same Git measurements.

The project is 13 days old. Nothing here can be quiet for six months yet, so the Signal column starts to mean something on 2027-05-10.

| Project area | What it does | Stage | Last worked on | Signal |
| --- | --- | --- | --- | --- |
| Audio processing and controls | Microphone capture, effects, profiles, plugins, the background service, and the settings window. VST3 runs on Linux. JSFX requires a build with ysfx. [crates/](crates/) | built - not shipped (25 Nov) | 2026-11-25 | `active` |
| Research and screenshots | Hush measurements and screenshots of the interface. [docs/](docs/) | not checked | 2026-11-23 | `active` |
| Installation | Files for Linux packages, the macOS app, and the macOS virtual microphone driver. [packaging/](packaging/) | built - not shipped (25 Nov) | 2026-11-25 | `active` |
| Build and checks | Scripts to build packages and check audio on Linux and macOS. [scripts/](scripts/) | not checked | 2026-11-22 | `active` |
| Build inputs | Lists workspace members and pins dependency versions. [Cargo.toml](Cargo.toml), [Cargo.lock](Cargo.lock) | not checked | 2026-11-24 | `active` |
| Architecture and development | Explains how audio flows and how to build and test the app. [ARCHITECTURE.md](ARCHITECTURE.md), [DEVELOPMENT.md](DEVELOPMENT.md) | not checked | 2026-11-25 | `active` |
| Project plan | Tracks completed milestones and remaining work. [PLAN.md](PLAN.md) | not checked | 2026-11-22 | `active` |
| Third-party components | Lists third-party components and their licenses. [THIRD_PARTY.md](THIRD_PARTY.md) | not checked | 2026-11-22 | `active` |
| Audit record | Validated findings, fixes, coverage, and validation results from the 2026-11-25 audit. [audit/](audit/) | built - not shipped (25 Nov) | 2026-11-25 | `active` |

## Worth retiring

Nothing found.

## What is next

### From the audit report (2026-11-25)

- Review and publish the audit branch, or decide what to keep. [audit/BUG_HUNT_REPORT.md](audit/BUG_HUNT_REPORT.md)
- Close the macOS virtual-microphone writer and microphone-permission trust gaps. [audit/REMEDIATION.md](audit/REMEDIATION.md), C-01 and C-02
- Pin update signing and verify the signer identity before shipping updates. [audit/REMEDIATION.md](audit/REMEDIATION.md), C-22
- Verify the audio fixes on real Linux and macOS hardware, including the driver and permission checks. [audit/VALIDATION.md](audit/VALIDATION.md)
- Work the remaining 36 recorded risks. [audit/REMEDIATION.md](audit/REMEDIATION.md)

### Open GitHub issues filed by the project author

- Measure why Hush takes longer per frame when processing runs every 10 ms. Reduce energy use without increasing the number of audio blocks that miss their deadline. [#20, 2026-11-23](https://github.com/binido/Otozane/issues/20)
- Add in-app updates and a setting for automatic updates. [#23, 2026-11-23](https://github.com/binido/Otozane/issues/23)
- Add an About section with the version, license, creator’s name, and a link to the source code. [#24, 2026-11-24](https://github.com/binido/Otozane/issues/24)

### Tasks from the project plan

- Test and tune the `Voice` preset on a speech recording. [PLAN.md, 2026-11-11](PLAN.md#next-steps)
- Consider using resampling to correct clock drift between devices instead of dropping frames. [PLAN.md, 2026-11-11](PLAN.md#next-steps)
- Check recovery after suspend and a PipeWire restart in a real desktop session. Check that the tray icon appears after logging out and back in. [PLAN.md, 2026-11-12 and 2026-11-22](PLAN.md#phases)
- Support latency changes while plugins are running and add plugin editor windows. JSFX `@gfx` windows are low priority. [Phases](PLAN.md#phases), [next steps](PLAN.md#next-steps), 2026-11-22.
- Add CLAP support and bring VST3 support to macOS. Submit the macOS app to Apple for notarization before distribution. [PLAN.md, 2026-11-14](PLAN.md#phases)

_Age and wiring measured 2026-11-25, fresh this pass. Stage comes from session reports and is only as current as the date beside it._
<!-- open-steps:end -->
