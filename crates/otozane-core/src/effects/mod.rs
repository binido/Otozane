//! Built-in effects and the [`Processor`] interface every chain entry implements.

mod dynamics;
pub mod eq;
mod gain;
pub mod hush;
pub mod rnnoise;

use crate::param::ParamSpec;
use serde::{Deserialize, Serialize};

/// Processor that leaves audio unchanged (used for plugins that failed to load).
pub struct Passthrough;

impl Processor for Passthrough {
    fn process(&mut self, _: &mut [&mut [f32]], _: &[f32]) {}
}

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
    Hush,
    Gate,
    Equalizer,
    Compressor,
    Limiter,
    /// A hosted third-party plugin; see `EffectConfig::plugin`.
    Plugin,
}

impl EffectKind {
    pub const ALL: &'static [EffectKind] = &[
        EffectKind::Rnnoise,
        EffectKind::Hush,
        EffectKind::Gate,
        EffectKind::Equalizer,
        EffectKind::Compressor,
        EffectKind::Limiter,
        EffectKind::Gain,
    ];

    pub fn name(self) -> &'static str {
        match self {
            EffectKind::Gain => "Gain",
            EffectKind::Rnnoise => "Noise Suppression",
            EffectKind::Hush => "Hush Noise Suppression",
            EffectKind::Gate => "Noise Gate",
            EffectKind::Equalizer => "Equalizer",
            EffectKind::Compressor => "Compressor",
            EffectKind::Limiter => "Limiter",
            EffectKind::Plugin => "Plugin",
        }
    }

    pub fn params(self) -> &'static [ParamSpec] {
        match self {
            EffectKind::Gain => gain::PARAMS,
            EffectKind::Rnnoise => rnnoise::PARAMS,
            EffectKind::Hush => hush::PARAMS,
            EffectKind::Gate => dynamics::GATE_PARAMS,
            EffectKind::Equalizer => eq::PARAMS,
            EffectKind::Compressor => dynamics::COMPRESSOR_PARAMS,
            EffectKind::Limiter => dynamics::LIMITER_PARAMS,
            // Plugin parameters are only known after loading.
            EffectKind::Plugin => &[],
        }
    }

    /// Label and unit of the per-effect meter, if the effect reports one.
    pub fn meter(self) -> Option<(&'static str, &'static str)> {
        match self {
            EffectKind::Gain | EffectKind::Equalizer | EffectKind::Plugin => None,
            EffectKind::Gate => Some(("Gain", "dB")),
            EffectKind::Compressor | EffectKind::Limiter => Some(("Reduction", "dB")),
            EffectKind::Rnnoise => Some(("Voice", "%")),
            EffectKind::Hush => Some(("SNR", "dB")),
        }
    }

    /// Creates a processor. Call this off the audio thread: it allocates.
    /// Fails only for effects that load external files (Hush).
    pub fn create(
        self,
        sample_rate: f32,
        channels: usize,
        max_block: usize,
    ) -> Result<Box<dyn Processor>, String> {
        let _ = max_block;
        Ok(match self {
            EffectKind::Gain => Box::new(gain::Gain::default()),
            EffectKind::Rnnoise => Box::new(rnnoise::Rnnoise::new(channels)),
            EffectKind::Hush => Box::new(hush::Hush::new(channels)?),
            EffectKind::Gate => Box::new(dynamics::Gate::new(sample_rate)),
            EffectKind::Equalizer => Box::new(eq::Equalizer::new(sample_rate, channels)),
            EffectKind::Compressor => Box::new(dynamics::Compressor::new(sample_rate)),
            EffectKind::Limiter => Box::new(dynamics::Limiter::new(sample_rate, channels)),
            // Plugins are created by the host through `engine::Controller::set_chain`.
            EffectKind::Plugin => Box::new(Passthrough),
        })
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

#[cfg(test)]
pub(crate) fn defaults(kind: EffectKind) -> Vec<f32> {
    kind.params().iter().map(|p| p.default).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_effect_handles_any_block_size_and_extreme_params() {
        for &kind in EffectKind::ALL {
            for channels in 1..=2 {
                let specs = kind.params();
                for extreme in [0.0f32, 1.0] {
                    let params: Vec<f32> = specs
                        .iter()
                        .map(|s| s.min + (s.max - s.min) * extreme)
                        .collect();
                    let mut p = match kind.create(48000.0, channels, 1024) {
                        Ok(p) => p,
                        // Hush without its runtime installed; covered in hush.rs.
                        Err(e) => {
                            eprintln!("skipping {kind:?}: {e}");
                            continue;
                        }
                    };
                    let mut seed = 1u32;
                    for n in [0usize, 1, 7, 128, 479, 480, 481, 1024] {
                        let mut bufs: Vec<Vec<f32>> = (0..channels)
                            .map(|_| {
                                (0..n)
                                    .map(|_| {
                                        seed = seed
                                            .wrapping_mul(1_664_525)
                                            .wrapping_add(1_013_904_223);
                                        (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
                                    })
                                    .collect()
                            })
                            .collect();
                        let mut planes: Vec<&mut [f32]> =
                            bufs.iter_mut().map(|b| b.as_mut_slice()).collect();
                        p.process(&mut planes, &params);
                        assert!(
                            bufs.iter().flatten().all(|s| s.is_finite()),
                            "{kind:?} produced invalid output"
                        );
                    }
                    assert!(p.meter().is_finite());
                }
            }
        }
    }
}
