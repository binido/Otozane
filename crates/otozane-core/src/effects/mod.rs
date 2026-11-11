//! Built-in effects and the [`Processor`] interface every chain entry implements.

mod gain;
pub mod rnnoise;

use crate::param::ParamSpec;
use serde::{Deserialize, Serialize};

/// Real-time audio processor.
///
/// `process` is called on the audio thread. Implementations must not
/// allocate, lock, or perform I/O there; all buffers are sized in the
/// constructor for `max_block` frames.
pub trait Processor: Send {
    /// Processes planar audio in place. `params` holds the current value of
    /// every parameter declared by the effect, in declaration order.
    fn process(&mut self, channels: &mut [&mut [f32]], params: &[f32]);

    /// Constant latency introduced by the processor, in frames.
    fn latency(&self) -> usize {
        0
    }

    /// Optional meter value shown next to the effect (e.g. gain reduction in dB).
    fn meter(&self) -> f32 {
        0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    Gain,
    Rnnoise,
}

impl EffectKind {
    pub const ALL: &'static [EffectKind] = &[EffectKind::Rnnoise, EffectKind::Gain];

    pub fn name(self) -> &'static str {
        match self {
            EffectKind::Gain => "Gain",
            EffectKind::Rnnoise => "Noise Suppression",
        }
    }

    pub fn params(self) -> &'static [ParamSpec] {
        match self {
            EffectKind::Gain => gain::PARAMS,
            EffectKind::Rnnoise => rnnoise::PARAMS,
        }
    }

    /// Label and unit of the per-effect meter, if the effect reports one.
    pub fn meter(self) -> Option<(&'static str, &'static str)> {
        match self {
            EffectKind::Gain => None,
            EffectKind::Rnnoise => Some(("Voice", "%")),
        }
    }

    /// Creates a processor. Call this off the audio thread: it allocates.
    pub fn create(self, sample_rate: f32, channels: usize, max_block: usize) -> Box<dyn Processor> {
        let _ = (sample_rate, max_block);
        match self {
            EffectKind::Gain => Box::new(gain::Gain::default()),
            EffectKind::Rnnoise => Box::new(rnnoise::Rnnoise::new(channels)),
        }
    }
}

#[cfg(test)]
pub(crate) fn run(p: &mut dyn Processor, input: &[f32], params: &[f32]) -> Vec<f32> {
    let mut buf = input.to_vec();
    for chunk in buf.chunks_mut(256) {
        p.process(&mut [chunk], params);
    }
    buf
}
