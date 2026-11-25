# Otozane big picture

<!-- open-steps:begin -->
_Measured 2026-11-25. Stages are dated where they stand; anything undated in this file is not measured._

## What this is

Otozane processes microphone audio and reduces background noise.
For calls, recordings, or streams, select **Otozane Microphone** in the app you use.
Otozane runs on Linux with PipeWire and on macOS 26 or later on Apple Silicon.
The `otozaned` background service processes the audio. [README.md](README.md)

GitHub published `v0.6.1` on 2026-11-25. [Release](https://github.com/binido/Otozane/releases/tag/v0.6.1)

## What is in it

The 2026-11-25 release report records the publication of 0.6.1 after the audit.
Rows group files that serve the same purpose and have the same Git measurements.

The project is 14 days old. The first parts can reach six months of inactivity on 2027-05-10.

| Project area | What it does | Stage | Last worked on | Signal |
| --- | --- | --- | --- | --- |
| Audio processing and controls | Microphone capture, effects, profiles, plugins, the background service, and the settings window. VST3 runs on Linux. JSFX requires a build with ysfx. [crates/](crates/) | live (25 Nov) | 2026-11-25 | `active` |
| Research and screenshots | Hush measurements and screenshots of the interface. [docs/](docs/) | not checked | 2026-11-23 | `active` |
| Installation | Files for Linux packages, the macOS app, and the macOS virtual microphone driver. [packaging/](packaging/) | live (25 Nov) | 2026-11-25 | `active` |
| Build and checks | Scripts to build packages and check audio on Linux and macOS. [scripts/](scripts/) | not checked | 2026-11-22 | `active` |
| Build inputs | Lists workspace members and pins dependency versions. [Cargo.toml](Cargo.toml), [Cargo.lock](Cargo.lock) | not checked | 2026-11-24 | `active` |
| Architecture and development | Explains how audio flows and how to build and test the app. [ARCHITECTURE.md](ARCHITECTURE.md), [DEVELOPMENT.md](DEVELOPMENT.md) | not checked | 2026-11-25 | `active` |
| Project plan | Tracks completed milestones and remaining work. [PLAN.md](PLAN.md) | not checked | 2026-11-22 | `active` |
| Third-party components | Lists third-party components and their licenses. [THIRD_PARTY.md](THIRD_PARTY.md) | not checked | 2026-11-22 | `active` |
| Audit record | Validated findings, fixes, coverage, and validation results from the 2026-11-25 audit. [audit/](audit/) | built - not shipped (25 Nov) | 2026-11-25 | `active` |

## Worth retiring

Nothing found.

## What is next

The [1.0 roadmap](PLAN.md#roadmap-to-100) lists the ordered milestones, release
criteria, and deferred work. The first milestone expands plugin support to
macOS VST3, CLAP, AU, LV2, and LADSPA. It also tracks VST2 distribution
feasibility. On 2026-11-25, the maintainer confirmed the
audio recovery checks, two weeks of daily use, a working first-run experience
and `Voice` preset, and a five-hour gaming session with Hush.

The full [audit follow-up list](audit/REMEDIATION.md) tracks remaining findings.
[Hush CPU cost (#20)](https://github.com/binido/Otozane/issues/20) remains open.

_Age and wiring measured 2026-11-25, fresh this pass. Stage comes from session reports and is only as current as the date beside it._
<!-- open-steps:end -->
