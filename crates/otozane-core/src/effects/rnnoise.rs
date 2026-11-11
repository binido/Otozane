//! RNNoise noise suppression through FFI to the C library.

use super::Processor;
use crate::param::ParamSpec;
use std::ptr::NonNull;

mod ffi {
    use std::ffi::c_void;
    #[repr(C)]
    pub struct DenoiseState(c_void);
    unsafe extern "C" {
        pub fn rnnoise_get_frame_size() -> i32;
        pub fn rnnoise_create(model: *mut c_void) -> *mut DenoiseState;
        pub fn rnnoise_destroy(st: *mut DenoiseState);
        pub fn rnnoise_process_frame(
            st: *mut DenoiseState,
            out: *mut f32,
            input: *const f32,
        ) -> f32;
    }
}

/// RNNoise operates on 10 ms frames at 48 kHz.
pub const FRAME: usize = 480;
/// RNNoise expects samples scaled to the 16-bit integer range.
const SCALE: f32 = 32768.0;

pub const PARAMS: &[ParamSpec] = &[
    ParamSpec::new("mix", "Mix", 0.0, 100.0, 100.0, "%"),
    ParamSpec::new("vad_threshold", "Voice threshold", 0.0, 95.0, 0.0, "%"),
    ParamSpec::new("vad_grace", "Voice grace period", 0.0, 1000.0, 200.0, "ms"),
];

/// Owned RNNoise state. Created and destroyed off the audio thread.
struct State(NonNull<ffi::DenoiseState>);

// SAFETY: a DenoiseState is plain heap memory with no thread affinity; it is
// only accessed through `&mut self`.
unsafe impl Send for State {}

impl Drop for State {
    fn drop(&mut self) {
        // SAFETY: pointer came from rnnoise_create and is destroyed once.
        unsafe { ffi::rnnoise_destroy(self.0.as_ptr()) }
    }
}

struct Channel {
    state: State,
    input: [f32; FRAME],
    /// Input of the previous frame, aligned with `output` for the dry mix.
    dry: [f32; FRAME],
    output: [f32; FRAME],
}

/// Buffers input into 480-sample frames and returns the previous frame's
/// output, giving exactly one frame (10 ms) of latency for any block size.
pub struct Rnnoise {
    channels: Vec<Channel>,
    pos: usize,
    vad: f32,
    /// Remaining samples of the grace period after the last voiced frame.
    hold: usize,
    gate: f32,
}

impl Rnnoise {
    pub fn new(channels: usize) -> Self {
        // SAFETY: plain query with no preconditions.
        assert_eq!(unsafe { ffi::rnnoise_get_frame_size() } as usize, FRAME);
        let channels = (0..channels)
            .map(|_| {
                // SAFETY: a null model selects the built-in default model.
                let ptr = unsafe { ffi::rnnoise_create(std::ptr::null_mut()) };
                Channel {
                    state: State(NonNull::new(ptr).expect("rnnoise_create failed")),
                    input: [0.0; FRAME],
                    dry: [0.0; FRAME],
                    output: [0.0; FRAME],
                }
            })
            .collect();
        Self {
            channels,
            pos: 0,
            vad: 0.0,
            hold: 0,
            gate: 1.0,
        }
    }
}

impl Processor for Rnnoise {
    fn process(&mut self, channels: &mut [&mut [f32]], p: &[f32]) {
        let mix = p[0] / 100.0;
        let threshold = p[1] / 100.0;
        let grace = (p[2] * 48.0) as usize;
        // ~5 ms gate ramp.
        let gate_step = 1.0 / 240.0;
        let n = channels[0].len();
        for i in 0..n {
            let target = if threshold <= 0.0 || self.hold > 0 {
                1.0
            } else {
                0.0
            };
            self.gate = if target > self.gate {
                (self.gate + gate_step).min(target)
            } else {
                (self.gate - gate_step).max(target)
            };
            for (ch, st) in channels.iter_mut().zip(self.channels.iter_mut()) {
                let x = ch[i];
                st.input[self.pos] = x * SCALE;
                let wet = st.output[self.pos] / SCALE;
                let dry = st.dry[self.pos] / SCALE;
                ch[i] = (dry + (wet - dry) * mix) * self.gate;
            }
            self.hold = self.hold.saturating_sub(1);
            self.pos += 1;
            if self.pos == FRAME {
                self.pos = 0;
                let mut vad = 0.0f32;
                for st in self.channels.iter_mut() {
                    st.dry = st.input;
                    // SAFETY: both buffers hold exactly FRAME floats and do not alias.
                    let v = unsafe {
                        ffi::rnnoise_process_frame(
                            st.state.0.as_ptr(),
                            st.output.as_mut_ptr(),
                            st.input.as_ptr(),
                        )
                    };
                    vad = vad.max(v);
                }
                self.vad = vad;
                if vad >= threshold {
                    self.hold = grace.max(FRAME);
                }
            }
        }
    }

    fn latency(&self) -> usize {
        FRAME
    }

    /// Voice activity probability of the last frame, in percent.
    fn meter(&self) -> f32 {
        self.vad * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, amp: f32) -> Vec<f32> {
        let mut seed = 0x1234_5678u32;
        (0..n)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn suppresses_white_noise() {
        let mut r = Rnnoise::new(1);
        let input = noise(48_000, 0.1);
        let out = super::super::run(&mut r, &input, &[100.0, 0.0, 200.0]);
        let (a, b) = (rms(&input[24_000..]), rms(&out[24_000..]));
        assert!(b < a * 0.25, "noise rms {a} -> {b}");
    }

    #[test]
    fn zero_mix_is_dry_signal_delayed_by_one_frame() {
        let mut r = Rnnoise::new(1);
        let input = noise(4_800, 0.1);
        let out = super::super::run(&mut r, &input, &[0.0, 0.0, 200.0]);
        for i in FRAME..input.len() {
            assert!(
                (out[i] - input[i - FRAME]).abs() < 1e-6,
                "{i}: {} vs {}",
                out[i],
                input[i - FRAME]
            );
        }
    }

    #[test]
    fn voice_gate_closes_on_noise() {
        let mut r = Rnnoise::new(1);
        let input = noise(48_000, 0.1);
        let out = super::super::run(&mut r, &input, &[0.0, 95.0, 0.0]);
        assert!(out[24_000..].iter().all(|s| s.abs() < 1e-6));
    }
}
