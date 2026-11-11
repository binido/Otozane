//! Level-dependent effects: noise gate, compressor, limiter.
//!
//! All three use linked detection (the loudest channel drives one gain for
//! every channel) so the stereo image does not shift.

use super::Processor;
use crate::dsp::{Delay, db_to_lin, lin_to_db, time_coef};
use crate::param::ParamSpec;

pub const GATE_PARAMS: &[ParamSpec] = &[
    ParamSpec::new("threshold", "Threshold", -80.0, 0.0, -45.0, "dB"),
    ParamSpec::new("range", "Range", -80.0, 0.0, -60.0, "dB"),
    ParamSpec::new("attack", "Attack", 0.1, 50.0, 1.0, "ms").log(),
    ParamSpec::new("hold", "Hold", 0.0, 500.0, 50.0, "ms"),
    ParamSpec::new("release", "Release", 5.0, 1000.0, 150.0, "ms").log(),
];

/// Hysteresis between opening and closing, so the gate does not chatter.
const GATE_HYSTERESIS_DB: f32 = 3.0;

pub struct Gate {
    sr: f32,
    env: f32,
    open: bool,
    hold_left: usize,
    gain: f32,
}

impl Gate {
    pub fn new(sr: f32) -> Self {
        Self {
            sr,
            env: 0.0,
            open: false,
            hold_left: 0,
            gain: 0.0,
        }
    }
}

impl Processor for Gate {
    fn process(&mut self, ch: &mut [&mut [f32]], p: &[f32]) {
        let open_lin = db_to_lin(p[0]);
        let close_lin = db_to_lin(p[0] - GATE_HYSTERESIS_DB);
        let floor = db_to_lin(p[1]);
        let att = time_coef(p[2], self.sr);
        let hold = (p[3] * 0.001 * self.sr) as usize;
        let rel = time_coef(p[4], self.sr);
        // Fast peak detector release, independent of the gate release.
        let det = time_coef(10.0, self.sr);
        for i in 0..ch[0].len() {
            let x = ch.iter().fold(0.0f32, |m, c| m.max(c[i].abs()));
            self.env = if x > self.env {
                x
            } else {
                x + (self.env - x) * det
            };
            if self.env >= open_lin {
                self.open = true;
                self.hold_left = hold;
            } else if self.env < close_lin {
                if self.hold_left > 0 {
                    self.hold_left -= 1;
                } else {
                    self.open = false;
                }
            }
            let target = if self.open { 1.0 } else { floor };
            let c = if target > self.gain { att } else { rel };
            self.gain = target + (self.gain - target) * c;
            for c in ch.iter_mut() {
                c[i] *= self.gain;
            }
        }
    }

    fn meter(&self) -> f32 {
        lin_to_db(self.gain).max(-80.0)
    }
}

pub const COMPRESSOR_PARAMS: &[ParamSpec] = &[
    ParamSpec::new("threshold", "Threshold", -60.0, 0.0, -18.0, "dB"),
    ParamSpec::new("ratio", "Ratio", 1.0, 20.0, 3.0, ":1").log(),
    ParamSpec::new("attack", "Attack", 0.1, 100.0, 5.0, "ms").log(),
    ParamSpec::new("release", "Release", 10.0, 1000.0, 100.0, "ms").log(),
    ParamSpec::new("knee", "Knee", 0.0, 24.0, 6.0, "dB"),
    ParamSpec::new("makeup", "Makeup gain", -12.0, 24.0, 0.0, "dB"),
];

/// Static gain computer with a soft knee; returns gain change in dB (<= 0).
pub fn compressor_curve(level_db: f32, threshold: f32, ratio: f32, knee: f32) -> f32 {
    let over = level_db - threshold;
    let slope = 1.0 / ratio - 1.0;
    if knee > 0.0 && over.abs() * 2.0 <= knee {
        slope * (over + knee / 2.0).powi(2) / (2.0 * knee)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

/// Feed-forward compressor with log-domain smoothing.
pub struct Compressor {
    sr: f32,
    gr_db: f32,
}

impl Compressor {
    pub fn new(sr: f32) -> Self {
        Self { sr, gr_db: 0.0 }
    }
}

impl Processor for Compressor {
    fn process(&mut self, ch: &mut [&mut [f32]], p: &[f32]) {
        let (thr, ratio, knee) = (p[0], p[1].max(1.0), p[4]);
        let att = time_coef(p[2], self.sr);
        let rel = time_coef(p[3], self.sr);
        let makeup = p[5];
        for i in 0..ch[0].len() {
            let x = ch.iter().fold(0.0f32, |m, c| m.max(c[i].abs()));
            let target = compressor_curve(lin_to_db(x), thr, ratio, knee);
            let c = if target < self.gr_db { att } else { rel };
            self.gr_db = target + (self.gr_db - target) * c;
            let g = db_to_lin(self.gr_db + makeup);
            for c in ch.iter_mut() {
                c[i] *= g;
            }
        }
    }

    fn meter(&self) -> f32 {
        self.gr_db
    }
}

pub const LIMITER_PARAMS: &[ParamSpec] = &[
    ParamSpec::new("ceiling", "Ceiling", -24.0, 0.0, -1.0, "dB"),
    ParamSpec::new("release", "Release", 10.0, 1000.0, 60.0, "ms").log(),
];

const LOOKAHEAD_MS: f32 = 5.0;

/// Lookahead peak limiter. The gain is computed from the undelayed signal
/// and applied to a copy delayed by the lookahead, so gain reduction is in
/// place before a peak arrives. A final clamp guarantees the ceiling.
pub struct Limiter {
    sr: f32,
    delay: Vec<Delay>,
    lookahead: usize,
    gain: f32,
    /// Remaining samples to hold the current reduction (covers the lookahead).
    hold_left: usize,
}

impl Limiter {
    pub fn new(sr: f32, channels: usize) -> Self {
        let lookahead = (LOOKAHEAD_MS * 0.001 * sr) as usize;
        Self {
            sr,
            delay: (0..channels).map(|_| Delay::new(lookahead)).collect(),
            lookahead,
            gain: 1.0,
            hold_left: 0,
        }
    }
}

impl Processor for Limiter {
    fn process(&mut self, ch: &mut [&mut [f32]], p: &[f32]) {
        let ceil = db_to_lin(p[0]);
        let rel = time_coef(p[1], self.sr);
        // Attack reaches ~99% within the lookahead window.
        let att = time_coef(LOOKAHEAD_MS / 4.6, self.sr);
        for i in 0..ch[0].len() {
            let x = ch.iter().fold(0.0f32, |m, c| m.max(c[i].abs()));
            let target = if x > ceil { ceil / x } else { 1.0 };
            if target < self.gain {
                self.gain = target + (self.gain - target) * att;
                self.hold_left = self.lookahead;
            } else if self.hold_left > 0 {
                self.hold_left -= 1;
            } else {
                self.gain = target + (self.gain - target) * rel;
            }
            for (c, d) in ch.iter_mut().zip(self.delay.iter_mut()) {
                c[i] = (d.tick(c[i]) * self.gain).clamp(-ceil, ceil);
            }
        }
    }

    fn latency(&self) -> usize {
        self.lookahead
    }

    fn meter(&self) -> f32 {
        lin_to_db(self.gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{EffectKind, defaults, run};

    fn sine(n: usize, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 48000.0).sin())
            .collect()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn gate_closes_below_threshold_and_opens_above() {
        let p = defaults(EffectKind::Gate);
        let quiet = run(&mut Gate::new(48000.0), &sine(48000, 0.001), &p);
        assert!(peak(&quiet[24000..]) < 0.001 * 0.01);
        let loud = run(&mut Gate::new(48000.0), &sine(48000, 0.5), &p);
        assert!((peak(&loud[24000..]) - 0.5).abs() < 0.01);
    }

    #[test]
    fn compressor_curve_and_steady_state() {
        assert_eq!(compressor_curve(-30.0, -20.0, 4.0, 0.0), 0.0);
        assert!((compressor_curve(-10.0, -20.0, 4.0, 0.0) + 7.5).abs() < 1e-4);
        // Soft knee is continuous at its edges.
        let edge = compressor_curve(-17.0, -20.0, 4.0, 6.0);
        assert!((edge - compressor_curve(-17.0, -20.0, 4.0, 0.0)).abs() < 1e-4);

        // 0 dBFS sine, threshold -20, ratio 4, no knee: about -15 dB of reduction.
        let p = [-20.0, 4.0, 1.0, 50.0, 0.0, 0.0];
        let out = run(&mut Compressor::new(48000.0), &sine(48000, 1.0), &p);
        let db = lin_to_db(peak(&out[40000..]));
        assert!((-16.5..-13.5).contains(&db), "{db}");
    }

    #[test]
    fn limiter_never_exceeds_ceiling() {
        let mut input = sine(48000, 0.3);
        for v in &mut input[20000..20100] {
            *v *= 4.0;
        }
        let mut l = Limiter::new(48000.0, 1);
        let out = run(&mut l, &input, &[-6.0, 60.0]);
        assert!(peak(&out) <= db_to_lin(-6.0) + 1e-6);
        // Below the ceiling the signal is untouched (apart from the delay).
        let lat = l.latency();
        assert!((out[5000 + lat] - input[5000]).abs() < 1e-5);
    }
}
