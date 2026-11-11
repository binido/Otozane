# Development

## Setup

Arch/CachyOS packages: `rust`, `clang`, `pkgconf`, `pipewire`, `rnnoise`.
Optional for scripts: `python`, `python-pillow` (screenshots), `python-fonttools` (icons).

```sh
cargo build
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all
```

## Running against a separate configuration

Use a throwaway config directory so development never touches your real profiles:

```sh
XDG_CONFIG_HOME=/tmp/otozane-dev ./target/debug/otozaned
./target/debug/otozane
```

Only one service can run per user (the socket is `$XDG_RUNTIME_DIR/otozane.sock`).
To run a second, test service next to a real one, set `OTOZANE_SOCKET` (socket
path, also used by the UI and `scripts/ipc.py`) and `OTOZANE_NODE_PREFIX`
(prefix for its PipeWire node names). `scripts/e2e.sh` does both.

## Scripts

| Script | Purpose |
|--------|---------|
| `scripts/e2e.sh [input.wav] [chain-json]` | End-to-end audio check. Creates a temporary test source with `pw-loopback`, starts `otozaned` with an isolated config, sets the chain, plays a file into the test source and records **Otozane Microphone** with `pw-record`, then prints peak/RMS per 100 ms. Does not touch real devices. |
| `scripts/tone.py OUT.wav SECONDS AMP [noise]` | Writes a 1 kHz sine or white noise test file. |
| `scripts/analyze.py FILE.wav` | Peak and RMS per 100 ms block. |
| `scripts/ipc.py JSON...` | Sends raw requests to the running service. |
| `scripts/screenshot.sh OUT.png [page] [dark\|light]` | Renders the UI and saves a screenshot (uses `OTOZANE_SCREENSHOT`, `OTOZANE_PAGE`, `OTOZANE_THEME`). |
| `scripts/icons.py` | Regenerates the Lucide font subset and `src/icons.rs`. |

Example checks used during development:

```sh
# +6 dB gain: a 0.25 sine should arrive at ~0.5.
scripts/e2e.sh "" '[{"id":1,"kind":"gain","params":{"gain":6}}]'

# RNNoise on white noise: output should fall to silence after ~0.5 s.
python3 scripts/tone.py /tmp/noise.wav 4 0.1 noise
scripts/e2e.sh /tmp/noise.wav '[{"id":1,"kind":"rnnoise"}]'

# Service requests
python3 scripts/ipc.py '{"cmd":"status"}'
python3 scripts/ipc.py '{"cmd":"set_param","id":2,"key":"gain","value":3}'
```

Note: when EasyEffects processes all inputs/outputs, test streams must opt out
of being moved (`node.dont-move = true`), which the scripts do.

## Plugins

```sh
# List the effects in a bundle (loads it in this process)
cargo run -p otozane-plugins --example scan -- /usr/lib/vst3/lsp-plugins.vst3
# Load one, print its parameters, process a sine, round-trip its state
cargo run -p otozane-plugins --example probe -- /usr/lib/vst3/lsp-plugins.vst3 "Limiter Mono" 5=0.3
# End to end with a plugin in the chain (output gain about -38 dB -> ~0.003)
scripts/e2e.sh "" '[{"id":1,"kind":"plugin","params":{"409613204":0.3},"plugin":{"format":"vst3","path":"/usr/lib/vst3/lsp-plugins.vst3","uid":"6473702062316D20202020206A7A357A","name":"Limiter Mono"}}]'
```

`tests/lsp.rs` runs against LSP plugins when `lsp-plugins-vst3` is installed
and skips otherwise.

## Adding an effect

1. Implement `Processor` in `crates/otozane-core/src/effects/`.
2. Declare its `ParamSpec`s and add a variant to `EffectKind` (name, params,
   meter, create, `ALL`).
3. The robustness test in `effects/mod.rs` covers it automatically; add a
   behavioral test next to the implementation.

The UI picks up parameters from the specs.

## Commit style

[Conventional Commits](https://www.conventionalcommits.org/): `type(scope): description`.
