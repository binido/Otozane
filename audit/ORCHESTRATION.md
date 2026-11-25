# Orchestration record

## Mechanism

- Primary agent: this Claude Code session in `~/GitCracken/Otozane`.
- Delegation:
  1. **Six Claude Code subagent sessions** through the Agent tool
     (general-purpose, background, read-only assignments): BH-03, BH-04, BH-05,
     BH-07, BH-08, BH-10.
  2. **One nested Claude Code CLI session** (`claude -p`, non-interactive,
     read-only tools) for the integration validation required by the brief.
- The global settings route all model aliases to `deepseek-v4.1-flash` through
  the existing `ANTHROPIC_BASE_URL`. The CLI reported that model identifier.
  Subagents inherit the session's provider configuration; model identity cannot
  be independently attested from inside the process. This record states what the
  runtime reported, no more.
- No credentials were read, printed, or changed. No remote sessions, no pushes,
  no merges, no releases.

## Integration validation

Command (from the repository root; tools limited to Read/Grep/Glob;
`--permission-mode dontAsk`; no session persistence):

```
claude -p "Read crates/otozane-core/src/protocol.rs. Reply with exactly one
line containing the request size limit constant name and value. Do not modify
anything and do not use any other tools." --allowedTools "Read,Grep,Glob" \
--permission-mode dontAsk --output-format text --no-session-persistence
```

Result: executed, read the file, answered that the limit is enforced in the
service rather than `protocol.rs`, and flagged the assumption. A first attempt
with `--max-budget-usd 0.30` was refused by the gateway's cost accounting
("Exceeded USD budget"); retrying without the cap succeeded. That was a
budget-accounting limit, not an auth/capability blocker.

## Delegated assignments

| Task | Scope | Mechanism | Status | Returned | Accepted by primary |
|---|---|---|---|---|---|
| BH-01 | Engine, RT invariants | Primary | complete | — | F-01, F-02, F-16, F-28 fixed |
| BH-02 | Unsafe/FFI inventory | Primary + cross-check | complete | — | F-12, F-16, F-23 fixed |
| BH-03 | CoreAudio + HAL | Subagent (1 session) | complete | 9 candidates | F-26, F-27 fixed; C-01..C-07 open |
| BH-04 | PipeWire | Subagent (1 session) | complete | 9 candidates | F-12..F-15 fixed; C-30..C-32 open |
| BH-05 | DSP correctness | Subagent (1 session) | complete | 10 candidates | F-16 fixed; F-01/F-02 confirmed; C-13..C-18 open |
| BH-07 | VST3/JSFX hosting | Subagent (1 session) | complete | 11 candidates | F-11, F-19..F-25 fixed; C-19..C-21, C-29 open; BH07-5 rejected |
| BH-08 | IPC/config/profiles | Subagent (1 session) | complete | 12 candidates | F-03..F-10 fixed; C-08..C-12 open |
| BH-10 | Updater/CI/UI | Subagent (1 session) | complete | 8 candidates | F-17, F-18 fixed; C-22..C-28 open |
| BH-11 | Concurrency/lifecycle | Primary | partial | — | F-03, F-12 fixed; no Loom/TSan |
| BH-13 | Dependencies/CI | Primary + BH-10 | partial | — | F-17 fixed; C-28 open |
| BH-15 | Performance/stability | Primary | partial | — | Static only; C-15 open |
| BH-R1 | Independent core/service regression review | Subagent (1 fresh session) | complete | 7 findings + fix verdicts | BHR1-1/2/5/6/7 fixed; BHR1-3/4 open (C-34/C-35) |
| BH-R2 | Independent platform regression review | Subagent (1 fresh session) | complete | 4 findings + fix verdicts | BHR2-1/3/4 fixed; BHR2-2 open (C-36) |

**Sessions actually executed:** 6 audit subagent sessions + 2 independent review
subagent sessions + 1 nested CLI integration session = 9.
**Completed:** 9. **Failed:** 0. **Blocked:** 0.

Both reviewers were instructed to load `triage-validation` and
`evidence-hygiene`; neither final report enumerated the skills it loaded, so
this record does not claim more than the instruction and the methodology their
findings follow. BH-R2's API-name checks against the locked crate sources are
recorded in its returned report; no build or runtime was available to it.

## Instructions given to every delegated session

- Unique task ID, repository path, baseline SHA, subsystem, files, objectives,
  invariants.
- Load the installed Claude-BugHunter skills and report which were used.
- Apply the 7-Question Gate adapted to an authorized desktop audit; actively try
  to disprove candidates.
- Return file:line evidence, reachability, expected vs actual, a minimal
  reproduction/regression proposal, a minimal fix, false-positive explanations,
  uncertainties, and coverage limits.
- Read-only: no edits, git changes, builds, network, plugin loading, audio
  device access, or system changes.

## Skills reported by delegated sessions

| Task | Skills reported |
|---|---|
| BH-03 | triage-validation, evidence-hygiene, bb-methodology |
| BH-04 | triage-validation, evidence-hygiene, bb-methodology |
| BH-05 | triage-validation, evidence-hygiene |
| BH-07 | triage-validation, evidence-hygiene, bb-methodology |
| BH-08 | triage-validation, evidence-hygiene, bb-methodology |
| BH-10 | triage-validation, supply-chain-attack-recon (evidence-hygiene/report-writing not used: no live PoC) |

The primary agent loaded `bb-methodology` and `triage-validation`, adapted them,
and independently verified every accepted finding. No delegated result was
treated as authoritative without a source check.

## Rejected or downgraded delegated candidates

- **BH07-5 (VST3 negative latency).** Rejected: the binding returns an unsigned
  latency, so negative values are impossible. Clippy flagged the attempted
  `.max(0)` and it was removed.
- **BH04-1 (listener UAF)** was treated as a hypothesis by the agent; the primary
  fetched PipeWire's `stream.c`/`hook.h` and confirmed the missing hook
  reinitialization, then fixed it.
- **BH04-6/BH04-7** were accepted as low-severity; BH04-8 was applied as
  hardening rather than a confirmed routing bug.
- **BH05-3/BH05-9** duplicated the primary agent's F-01/F-02 and were folded in.
- **BH10-6** was partially fixed (archive root, symlinked binaries,
  `--no-same-owner`); member-type/traversal validation remains C-27.
- **BH08-4** was partially fixed; the independent review then found that
  ASCII-only folding missed macOS Unicode collisions. The final fix uses the
  filesystem and file identity (`799bdb7`).
- **BHR1-3/BHR1-4** (profile-restore latency gap and the fixed fade wait) were
  accepted as design limitations, recorded as C-34 and C-35 rather than
  patched; both need an engine-level crossfade or audio-thread acknowledgement.
- **BHR2-1/BHR2-3/BHR2-4** (reconnect error masked, JSFX load cap incomplete,
  tar layout followed symlinks) were accepted and fixed in `49ea16f`,
  `df1ca48`, and `ebacedd`.
- **BHR2-2** (the `device.id` hardware heuristic) could not be verified without
  a Linux graph; accepted as C-36 for a Linux check, not silently dismissed.

## Findings flow

Delegated candidates were re-read in the source, compared with the documented
behavior, and either fixed with a regression test where feasible or recorded as
an open candidate with severity, confidence, and a recommended fix. The
consolidated record is `BUG_HUNT_REPORT.md`; fixes and backlog are in
`REMEDIATION.md`; coverage and validation are in `COVERAGE.md` and
`VALIDATION.md`.

## Blocked / failed work

None. Environment limits (no Linux runtime, no live macOS audio/driver/TCC, no
installer execution) are recorded as coverage gaps, not delegation failures.
