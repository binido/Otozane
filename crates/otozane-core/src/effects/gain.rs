use super::Processor;
use crate::dsp::db_to_lin;
use crate::param::ParamSpec;

pub const PARAMS: &[ParamSpec] = &[ParamSpec::new("gain", "Gain", -36.0, 36.0, 0.0, "dB")];

/// Gain with a per-block linear ramp so parameter changes do not click.
#[derive(Default)]
pub struct Gain {
    current: Option<f32>,
}

impl Processor for Gain {
    fn process(&mut self, channels: &mut [&mut [f32]], params: &[f32]) {
        let target = db_to_lin(params[0]);
        let start = *self.current.get_or_insert(target);
        let n = channels.first().map_or(0, |c| c.len());
        if n == 0 {
            return;
        }
        let step = (target - start) / n as f32;
        for ch in channels.iter_mut() {
            for (i, s) in ch.iter_mut().enumerate() {
                *s *= start + step * (i + 1) as f32;
            }
        }
        self.current = Some(target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_gain_and_ramps() {
        let mut g = Gain::default();
        let out = super::super::run(&mut g, &[1.0; 512], &[6.0]);
        assert!((out[511] - 1.995).abs() < 0.01);
        // Changing the target ramps instead of jumping.
        let mut buf = [1.0f32; 256];
        g.process(&mut [&mut buf], &[0.0]);
        assert!(buf[0] > 1.9 && (buf[255] - 1.0).abs() < 1e-4);
    }
}
