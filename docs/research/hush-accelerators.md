# Hush on GPU/NPU: research notes

Research for issue #18. Machine: Apple M5 (10 cores), macOS 27.2.1, Rust 1.96.
Measured 2026-11-23. Prototype code and raw logs stayed outside the repo.

## Recommendation

- **GPU (tract `metal`): no-go.** On tract 0.23 Metal, a frame takes 10x
  longer than on CPU, uses 4.6x the CPU time and shows 3.7x the energy impact.
  All five GRU layers fall back to CPU.
- **NPU: no-go for now.** No usable converted model exists for the Hush
  weights. Getting one means replacing `libweya_nc` and its DSP. The cost it
  would save is mostly wake-up overhead, which an NPU also pays.
- **Where the CPU actually goes:** most of the paced cost is the core waking
  from idle every 10 ms, not the model. The same frame takes 0.18 ms
  back-to-back and 0.9 ms when paced. `top` CPU % therefore overstates the
  energy cost. A cheaper fix, if one is needed, is on the CPU side (see
  Follow-up).

## 1. Where the service CPU goes

Live service (`/Applications/Otozane.app`, mono, chain with RNNoise and Hush).
Per-thread CPU from `proc_pidinfo` over 60 s. On-CPU split of the audio thread
from `sample` (10 s, leaf frames that are not waits).

| Part | % of one core |
|---|---|
| Whole process (`ps` TIME delta, 60 s) | 7.6 (`top` fluctuates 6 to 11) |
| `hush` worker thread (inference) | 4.1 to 4.6 |
| Audio IO thread, total | 2.7 |
| of which RNNoise | ~1.9 |
| of which Hush 48/16 kHz resampling and queueing | ~0.4 |
| `otozane-coreaudio` (device poll every 1 s) | 0.14 |

The repo bench (`hush::tests::bench`, back-to-back, P-core) gives 2.2% total
(audio thread 0.06%) for mono and 5.1% for stereo, with a worst 10 ms block of
1.4 ms and 3.1 ms. Resampling and thread wake-ups on the audio side are small.
Inference dominates.

Inference cost depends on pacing. Throwaway ctypes bench calling
`weya_nc_process_frame` (tract 0.19.16, 160-sample frames, noise input, 2000
frames):

| Mode | mean ms | p99 ms | max ms | > budget (21.6 ms) | thread CPU % |
|---|---|---|---|---|---|
| back-to-back | 0.180 | 0.390 | 1.13 | 0 | 1.8 |
| paced 10 ms, default QoS | 0.896 | 1.567 | 3.19 | 0 | 9.1 |
| paced, user-interactive QoS | 0.921 | 1.525 | 1.84 | 0 | 9.4 |
| paced, utility QoS | 0.812 | 1.463 | 3.65 | 0 | 8.2 |
| paced, background QoS | 0.883 | 2.646 | 33.9 | 1 | 8.1 |
| paced with spin-wait instead of sleep (app dylib) | 0.283 | 0.693 | 1.00 | 0 | (spinning) |
| paced with sleep (app dylib) | 1.125 | 1.680 | 2.91 | 0 | 11.4 |

QoS does not matter. Keeping the core busy (spin-wait) brings the frame back to
0.28 ms, so the 4 to 5x slowdown comes from idle-state and clock ramp-up plus
cold caches after each 10 ms sleep. The live worker (4.3%) sits between the
back-to-back and the isolated paced numbers. The likely reason is other system
activity keeping clocks up, but that was not verified.

## 2. tract 0.23 bump and Metal

**Bumping Hush's vendored `deep_filter` to tract 0.23.10 does not build.** The
sources were fetched as `build-hush.sh` does (commit `9f6414e`), then the four
`tract-*` deps were changed from `^0.19.4` to `=0.23.10`. The build fails with
28 errors in `deep_filter`, all API renames, none of them blocking in
principle:

```
   5 error[E0107]: type alias takes 0 generic arguments                                     (TypedSimpleState/Plan)
   3 error[E0609]: no field `symbol_table` on type `Graph<InferenceFact, ...>`               (now `symbols`)
   3 error[E0599]: no method named `with_output_names` found for struct `Graph`              (now `with_outputs_by_name`)
   3 error[E0433]: cannot find type `SimpleState` in this scope
  12 error[E0599]: no method named `to_array_view` / `to_array_view_mut` / `into_array`      (now `to_plain_array_view`, ...)
   2 error[E0599]: no associated function `TValue::Var` / `TValue::Const`
```

**Standalone prototype instead.** A small crate on tract 0.23.10
(`tract-onnx`, `tract-pulse`, `tract-metal`) loads the three Hush sub-models
(`enc`, `erb_dec`, `df_dec` from `advanced_dfnet16k`) the same way
`deep_filter` does, with an `S` symbol pulsed to 1, and runs one streaming
step per frame. It does not run the STFT, ERB features or deep filtering.
Those are CPU DSP either way. It loads and runs on both runtimes.

**Ops on Metal vs CPU** (per-node placement after `MetalTransform`):

| Sub-model | On Metal | Left on CPU | Host/device syncs per run |
|---|---|---|---|
| enc | Conv x6, MppGemm x10, Add x9, Max x9, MoveAxis x10, PulsePad x2, Delay x2, Mul, Sigmoid | `Scan` x1 (`/emb_gru/gru/GRU`) | 8 to host, 3 to device |
| erb_dec | Conv x2, Deconv x2, MppGemm x10, Add x12, Max x9, MoveAxis x9, Sigmoid | `Scan` x1 (`/emb_gru/gru/GRU`) | 2 to host, 6 to device |
| df_dec | MppGemm x6, Add x2, Max x2, MoveAxis x2, Tanh | `Scan` x3 (`/df_gru/gru/GRU`, `GRU_1`, `GRU_2`) | 4 to host, 5 to device |

All five GRU layers (tract `Scan`) fall back to CPU, so every frame makes 28
host/device copies.

## 3. CPU vs Metal benchmark

Same prototype, same input (zeros), 2000 frames after 100 warm-up frames.
"Paced" sleeps to a 10 ms cadence like the service. CPU % is process CPU time
(all threads, including Metal driver threads) over audio time. Energy impact is
the `top` POWER column (unitless, no sudo), averaged over 7 samples during a
35 s paced run.

| Runtime | Mode | mean ms | p99 ms | max ms | > 10 ms | > budget | CPU % | Energy impact |
|---|---|---|---|---|---|---|---|---|
| CPU (tract 0.23) | back-to-back | 0.124 | 0.140 | 0.17 | 0 | 0 | 1.2 | |
| Metal | back-to-back | 2.512 | 3.096 | 3.66 | 0 | 0 | 6.2 | |
| CPU (tract 0.23) | paced | 0.768 | 1.098 | 3.91 | 0 | 0 | 7.8 | 10.1 |
| Metal | paced | 7.264 | 9.354 | 17.8 | 11 | 0 | 35.1 | 37.2 |

Metal model preparation takes 3.3 s, compared with 64 ms on CPU. The tract
0.23 CPU numbers above cover the networks only, so they are not directly
comparable with the 0.19 `weya_nc_process_frame` numbers in section 1.

**Not measured:** real power in watts. `powermetrics` needs sudo and was not
run. GPU-side power is only reflected through the `top` energy-impact proxy.
Output parity between Metal and CPU was not checked, since the latency result
already decides the question.

## 4. NPU paths (desk research, not measured)

**Core ML / Apple Neural Engine.** The community conversion
[iky1e/DeepFilterNet3-Streaming-CoreML](https://huggingface.co/iky1e/DeepFilterNet3-Streaming-CoreML)
is the stock 48 kHz DFN3 (hop 480, 30 ms algorithmic delay), not the Hush
16 kHz fine-tune. Recurrent state is passed as explicit inputs and outputs, not
`MLState`. It reports "unpaced steady per-hop Core ML compute was 0.264 ms" on
an unnamed Apple Silicon Mac. It does not say which compute unit ran (ANE, GPU
or CPU), and it reports no paced latency. A conversion script exists in
`kylehowells/DeepFilterNet-mlx` (not reviewed). Using the Hush weights would
take:

- converting `enc`/`erb_dec`/`df_dec` (ONNX in the bundle, or PyTorch if Weya
  publishes a checkpoint) to Core ML at 16 kHz / hop 160, with GRU state as
  explicit I/O;
- reimplementing what `libweya_nc` does around the nets in Otozane (STFT 320/160,
  ERB and complex features with their running normalisation, deep filter of
  order 5, lsnr-based stage skipping), and matching it against the library
  output;
- a Swift or objc2 bridge to Core ML on the worker thread.

The licence is not a blocker (Hush and DFN are Apache-2.0 or MIT).

Latency risk: an unpaced 0.26 ms says little about a 10 ms paced stream. ANE
dispatch goes through a daemon with its own wake-up and scheduling. The DAFx26
paper cited in the issue reports no callback-safe path, and our measurements
show wake-up cost already dominates on CPU. Late frames would play dry, which
is audible.

**ONNX Runtime + OpenVINO EP on Intel NPU (Linux).** This needs a new native
runtime dependency and the model exported with GRU state as explicit I/O (or
OpenVINO's MakeStateful). The NPU plugin wants static shapes, which a pulse of
one frame satisfies. OpenVINO maps ONNX GRU to `GRUSequence`. No public
per-call latency numbers for small recurrent models on the Intel NPU were
found, and no Intel NPU hardware was available to test. Same latency risk as
ANE, plus a second model format to keep in sync with the Hush weights.

## Follow-up

No implementation issue for a GPU/NPU backend. If Hush CPU use is still a
concern, a CPU-side issue would target the wake-up overhead:

> **Hush: reduce per-frame wake-up cost on CPU.** A paced 10 ms
> `weya_nc_process_frame` costs 0.9 ms against 0.18 ms back-to-back, because
> the core idles between frames (docs/research/hush-accelerators.md). Options
> to measure: run the Hush worker for two frames per wake-up (costs 10 ms of
> latency), or port `libweya_nc` to tract 0.23 for its faster CPU kernels (28
> API-rename errors in the vendored `deep_filter`). Measure with energy
> impact, not `top` CPU %. Done when service energy impact drops measurably
> with no change in late frames.
