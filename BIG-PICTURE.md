# Otozane big picture

<!-- open-steps:begin -->
_Git snapshot from 2026-11-24. The date beside a stage shows when it was confirmed._

## What this is

Otozane processes microphone audio and reduces background noise.
For calls, recordings, or streams, select **Otozane Microphone** in the app you use.
Otozane runs on Linux with PipeWire and on macOS 26 or later on Apple Silicon.
The `otozaned` background service processes the audio. [README.md](README.md)

GitHub published `v0.5.0` on 2026-11-22. [Release](https://github.com/binido/Otozane/releases/tag/v0.5.0)

## What is in it

As of 2026-11-24, Otozane has no Open Steps reports, so the table marks each stage as “not checked.”
Rows group files that serve the same purpose and have the same Git measurements.

The project is 12 days old. No part has existed long enough to go six months without changes. The signal can first identify parts with six months of inactivity on 2027-05-10.

| Project area | What it does | Stage | Last worked on | Signal |
| --- | --- | --- | --- | --- |
| Audio processing and controls | Microphone capture, effects, profiles, plugins, the background service, and the settings window. VST3 runs on Linux. JSFX requires a build with ysfx. [crates/](crates/) | not checked | 2026-11-23 | `active` |
| Research and screenshots | Hush measurements and screenshots of the interface. [docs/](docs/) | not checked | 2026-11-23 | `active` |
| Installation | Files for Linux packages, the macOS app, and the macOS virtual microphone driver. [packaging/](packaging/) | not checked | 2026-11-22 | `active` |
| Build and checks | Scripts to build packages and check audio on Linux and macOS. [scripts/](scripts/) | not checked | 2026-11-22 | `active` |
| Build inputs | Lists workspace members and pins dependency versions. [Cargo.toml](Cargo.toml), [Cargo.lock](Cargo.lock) | not checked | 2026-11-22 | `active` |
| Architecture and development | Explains how audio flows and how to build and test the app. [ARCHITECTURE.md](ARCHITECTURE.md), [DEVELOPMENT.md](DEVELOPMENT.md) | not checked | 2026-11-22 | `active` |
| Project plan | Tracks completed milestones and remaining work. [PLAN.md](PLAN.md) | not checked | 2026-11-22 | `active` |
| Third-party components | Lists third-party components and their licenses. [THIRD_PARTY.md](THIRD_PARTY.md) | not checked | 2026-11-22 | `active` |

## Worth retiring

The Git snapshot shows no candidates for removal.

## What is next

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

_The Git check on 2026-11-24 measured the project’s age and which parts other files mention. Each stage reflects the Open Steps reports as of the date shown._
<!-- open-steps:end -->
