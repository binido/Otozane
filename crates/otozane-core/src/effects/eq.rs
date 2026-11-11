//! Five-band parametric equalizer.

use super::Processor;
use crate::dsp::{Biquad, Coefs, FilterType};
use crate::param::ParamSpec;

pub const BANDS: usize = 5;
pub const TYPES: &[&str] = &[
    "Off",
    "Peak",
    "Low shelf",
    "High shelf",
    "Low pass",
    "High pass",
];
const PER_BAND: usize = 4;

macro_rules! band {
    ($n:literal, $ty:expr, $freq:expr) => {
        [
            ParamSpec::new(
                concat!("b", $n, "_type"),
                concat!("Band ", $n, " type"),
                0.0,
                5.0,
                $ty,
                "",
            )
            .choice(TYPES),
            ParamSpec::new(
                concat!("b", $n, "_freq"),
                concat!("Band ", $n, " frequency"),
                20.0,
                20000.0,
                $freq,
                "Hz",
            )
            .log(),
            ParamSpec::new(
                concat!("b", $n, "_gain"),
                concat!("Band ", $n, " gain"),
                -24.0,
                24.0,
                0.0,
                "dB",
            ),
            ParamSpec::new(
                concat!("b", $n, "_q"),
                concat!("Band ", $n, " Q"),
                0.1,
                10.0,
                0.71,
                "",
            )
            .log(),
        ]
    };
}

const fn flatten(b: [[ParamSpec; PER_BAND]; BANDS]) -> [ParamSpec; BANDS * PER_BAND] {
    let mut out = [b[0][0]; BANDS * PER_BAND];
    let mut i = 0;
    while i < BANDS * PER_BAND {
        out[i] = b[i / PER_BAND][i % PER_BAND];
        i += 1;
    }
    out
}

static SPECS: [ParamSpec; BANDS * PER_BAND] = flatten([
    band!("1", 2.0, 100.0),
    band!("2", 1.0, 250.0),
    band!("3", 1.0, 1000.0),
    band!("4", 1.0, 4000.0),
    band!("5", 3.0, 8000.0),
]);
pub const PARAMS: &[ParamSpec] = &SPECS;

/// Coefficients for one band from its four parameter values, or `None` when off.
pub fn band_coefs(v: &[f32], sample_rate: f32) -> Option<Coefs> {
    let ty = match v[0].round() as i32 {
        1 => FilterType::Peak,
        2 => FilterType::LowShelf,
        3 => FilterType::HighShelf,
        4 => FilterType::LowPass,
        5 => FilterType::HighPass,
        _ => return None,
    };
    Some(Coefs::new(ty, v[1], v[3], v[2], sample_rate))
}

/// Combined magnitude response in dB at `freq` for a full parameter set.
pub fn response_db(params: &[f32], freq: f32, sample_rate: f32) -> f32 {
    params
        .chunks_exact(PER_BAND)
        .filter_map(|b| band_coefs(b, sample_rate))
        .map(|c| c.response_db(freq, sample_rate))
        .sum()
}

pub struct Equalizer {
    sr: f32,
    last: [f32; BANDS * PER_BAND],
    coefs: [Option<Coefs>; BANDS],
    state: Vec<[Biquad; BANDS]>,
}

impl Equalizer {
    pub fn new(sr: f32, channels: usize) -> Self {
        Self {
            sr,
            last: [f32::NAN; BANDS * PER_BAND],
            coefs: [None; BANDS],
            state: vec![[Biquad::default(); BANDS]; channels],
        }
    }
}

impl Processor for Equalizer {
    fn process(&mut self, ch: &mut [&mut [f32]], p: &[f32]) {
        // Coefficients are recomputed only when a parameter changed.
        if self.last[..] != p[..] {
            self.last.copy_from_slice(p);
            for (b, c) in p.chunks_exact(PER_BAND).zip(self.coefs.iter_mut()) {
                *c = band_coefs(b, self.sr);
            }
        }
        for (c, st) in ch.iter_mut().zip(self.state.iter_mut()) {
            for (coef, bq) in self.coefs.iter().zip(st.iter_mut()) {
                if let Some(coef) = coef {
                    for s in c.iter_mut() {
                        *s = bq.tick(coef, *s);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{EffectKind, defaults, run};

    #[test]
    fn defaults_are_flat_and_boost_applies() {
        let p = defaults(EffectKind::Equalizer);
        for f in [50.0, 1000.0, 15000.0] {
            assert!(response_db(&p, f, 48000.0).abs() < 0.01);
        }
        let mut p = p;
        p[10] = 6.0; // band 3 gain (peak @ 1 kHz)
        assert!((response_db(&p, 1000.0, 48000.0) - 6.0).abs() < 0.05);

        let sine: Vec<f32> = (0..48000)
            .map(|i| 0.1 * (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48000.0).sin())
            .collect();
        let out = run(&mut Equalizer::new(48000.0, 1), &sine, &p);
        let peak = out[24000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.1 * 1.995).abs() < 0.005, "{peak}");
    }
}
