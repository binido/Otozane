//! Real-time engine and its non-real-time controller.

use crate::config::EffectConfig;
use crate::effects::{EffectKind, Processor};
use crate::param::AtomicF32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Maximum channels processed by the engine.
pub const MAX_CHANNELS: usize = 2;
/// Frames processed per internal block; larger host buffers are split.
pub const MAX_BLOCK: usize = 1024;
/// Length of the crossfade used for enable/disable, in frames (10 ms).
const FADE_FRAMES: f32 = 480.0;

/// State shared between the controller and one effect instance.
#[derive(Debug)]
pub struct SlotShared {
    pub id: u64,
    pub kind: EffectKind,
    pub enabled: AtomicBool,
    pub params: Box<[AtomicF32]>,
    pub meter: AtomicF32,
}

/// Engine-wide state readable from any thread.
#[derive(Debug, Default)]
pub struct EngineShared {
    pub processing_enabled: AtomicBool,
    /// Peak absolute sample since the last read (read with `swap(0.0)`).
    pub input_peak: AtomicF32,
    pub output_peak: AtomicF32,
    /// Total latency of the active chain, in frames.
    pub latency_frames: AtomicU32,
    /// Number of `process` calls, useful to see whether audio is flowing.
    pub blocks: AtomicU64,
}

struct Slot {
    shared: Arc<SlotShared>,
    processor: Box<dyn Processor>,
    values: Vec<f32>,
    /// Current wet amount, ramped towards 0 or 1.
    mix: f32,
    dry: [Vec<f32>; MAX_CHANNELS],
    dry_delay: [crate::dsp::Delay; MAX_CHANNELS],
}

impl Slot {
    fn new(shared: Arc<SlotShared>, processor: Box<dyn Processor>) -> Self {
        let latency = processor.latency();
        let enabled = shared.enabled.load(Ordering::Relaxed);
        Self {
            values: vec![0.0; shared.params.len()],
            shared,
            processor,
            mix: if enabled { 1.0 } else { 0.0 },
            dry: std::array::from_fn(|_| vec![0.0; MAX_BLOCK]),
            dry_delay: std::array::from_fn(|_| crate::dsp::Delay::new(latency)),
        }
    }

    fn process(&mut self, channels: &mut [&mut [f32]], master: bool) {
        let target = if master && self.shared.enabled.load(Ordering::Relaxed) {
            1.0
        } else {
            0.0
        };
        for (v, p) in self.values.iter_mut().zip(self.shared.params.iter()) {
            *v = p.load();
        }
        let n = channels[0].len();
        let crossfade = self.mix != 1.0 || target != 1.0;
        if crossfade {
            // The dry path is delayed by the processor latency so toggling an
            // effect does not shift the signal in time.
            for (c, ch) in channels.iter().enumerate() {
                let dry = &mut self.dry[c][..n];
                dry.copy_from_slice(ch);
                self.dry_delay[c].process(dry);
            }
        }
        // The processor always runs so its internal state stays current.
        self.processor.process(channels, &self.values);
        self.shared.meter.store(self.processor.meter());
        if !crossfade {
            return;
        }
        let step = (target - self.mix).signum() / FADE_FRAMES;
        for i in 0..n {
            let m = self.mix;
            for (c, ch) in channels.iter_mut().enumerate() {
                let d = self.dry[c][i];
                ch[i] = d + (ch[i] - d) * m;
            }
            self.mix = if step > 0.0 {
                (m + step).min(target)
            } else {
                (m + step).max(target)
            };
        }
    }
}

/// A chain as handed to the audio thread. `None` entries in a new chain are
/// filled by moving the matching slot out of the previous chain, so effect
/// state survives reordering without allocating on the audio thread.
struct Chain {
    ids: Vec<u64>,
    slots: Vec<Option<Slot>>,
}

/// Real-time half of the engine. Owned by the audio callback.
pub struct Engine {
    channels: usize,
    chain: Box<Chain>,
    incoming: rtrb::Consumer<Box<Chain>>,
    garbage: rtrb::Producer<Box<Chain>>,
    shared: Arc<EngineShared>,
    planes: [Vec<f32>; MAX_CHANNELS],
}

impl Engine {
    pub fn channels(&self) -> usize {
        self.channels
    }

    fn apply_pending(&mut self) {
        // Only take a new chain when the old one can be handed back for
        // deallocation; dropping it here would free memory on the audio thread.
        if self.garbage.slots() == 0 {
            return;
        }
        let Ok(mut next) = self.incoming.pop() else {
            return;
        };
        for i in 0..next.slots.len() {
            if next.slots[i].is_none() {
                let id = next.ids[i];
                if let Some(pos) = self.chain.ids.iter().position(|&x| x == id) {
                    next.slots[i] = self.chain.slots[pos].take();
                }
            }
        }
        std::mem::swap(&mut self.chain, &mut next);
        let _ = self.garbage.push(next);
    }

    /// Processes interleaved audio with `self.channels()` channels in place.
    pub fn process_interleaved(&mut self, buf: &mut [f32]) {
        self.apply_pending();
        if buf.is_empty() {
            return;
        }
        let ch = self.channels;
        let master = self.shared.processing_enabled.load(Ordering::Relaxed);
        let in_peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        for chunk in buf.chunks_mut(MAX_BLOCK * ch) {
            let n = chunk.len() / ch;
            for (i, frame) in chunk.chunks_exact(ch).enumerate() {
                for (plane, &s) in self.planes.iter_mut().zip(frame) {
                    plane[i] = s;
                }
            }
            let (a, b) = self.planes.split_at_mut(1);
            let mut planes: [&mut [f32]; MAX_CHANNELS] = [&mut a[0][..n], &mut b[0][..n]];
            for slot in self.chain.slots.iter_mut().flatten() {
                slot.process(&mut planes[..ch], master);
            }
            for (i, frame) in chunk.chunks_exact_mut(ch).enumerate() {
                for (s, plane) in frame.iter_mut().zip(&self.planes) {
                    *s = plane[i];
                }
            }
        }
        let out_peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let s = &self.shared;
        s.input_peak.store(s.input_peak.load().max(in_peak));
        s.output_peak.store(s.output_peak.load().max(out_peak));
        s.blocks.fetch_add(1, Ordering::Relaxed);
    }
}

/// Non-real-time half of the engine. Builds effects and sends them to [`Engine`].
pub struct Controller {
    sample_rate: f32,
    channels: usize,
    outgoing: rtrb::Producer<Box<Chain>>,
    garbage: rtrb::Consumer<Box<Chain>>,
    slots: Vec<Arc<SlotShared>>,
    latencies: Vec<usize>,
    pub shared: Arc<EngineShared>,
}

/// Creates a connected engine pair for `channels` (1 or 2) at `sample_rate`.
pub fn new(sample_rate: f32, channels: usize) -> (Controller, Engine) {
    assert!((1..=MAX_CHANNELS).contains(&channels));
    let (outgoing, incoming) = rtrb::RingBuffer::new(4);
    let (garbage_tx, garbage) = rtrb::RingBuffer::new(4);
    let shared = Arc::new(EngineShared::default());
    shared.processing_enabled.store(true, Ordering::Relaxed);
    let engine = Engine {
        channels,
        chain: Box::new(Chain {
            ids: Vec::new(),
            slots: Vec::new(),
        }),
        incoming,
        garbage: garbage_tx,
        shared: shared.clone(),
        planes: std::array::from_fn(|_| vec![0.0; MAX_BLOCK]),
    };
    let controller = Controller {
        sample_rate,
        channels,
        outgoing,
        garbage,
        slots: Vec::new(),
        latencies: Vec::new(),
        shared,
    };
    (controller, engine)
}

#[derive(Debug, PartialEq)]
pub enum ControlError {
    UnknownEffect(u64),
    UnknownParam(String),
    /// The audio thread has not consumed previous updates yet.
    Busy,
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlError::UnknownEffect(id) => write!(f, "no effect with id {id}"),
            ControlError::UnknownParam(k) => write!(f, "unknown parameter '{k}'"),
            ControlError::Busy => write!(f, "audio thread is not consuming updates"),
        }
    }
}

impl std::error::Error for ControlError {}

impl Controller {
    /// Frees chains the audio thread has handed back.
    pub fn collect_garbage(&mut self) {
        while self.garbage.pop().is_ok() {}
    }

    /// Replaces the chain. Effects whose id and kind match an existing slot
    /// keep their processor and internal state.
    pub fn set_chain(&mut self, chain: &[EffectConfig]) -> Result<(), ControlError> {
        self.collect_garbage();
        if self.outgoing.slots() == 0 {
            return Err(ControlError::Busy);
        }
        let mut next = Chain {
            ids: Vec::with_capacity(chain.len()),
            slots: Vec::with_capacity(chain.len()),
        };
        let mut shared = Vec::with_capacity(chain.len());
        let mut latencies = Vec::with_capacity(chain.len());
        for cfg in chain {
            let existing = self
                .slots
                .iter()
                .position(|s| s.id == cfg.id && s.kind == cfg.kind);
            let slot_shared = match existing {
                Some(i) => {
                    next.slots.push(None);
                    latencies.push(self.latencies[i]);
                    self.slots[i].clone()
                }
                None => {
                    let s = Arc::new(SlotShared {
                        id: cfg.id,
                        kind: cfg.kind,
                        enabled: AtomicBool::new(cfg.enabled),
                        params: cfg.values().into_iter().map(AtomicF32::new).collect(),
                        meter: AtomicF32::default(),
                    });
                    let p = cfg.kind.create(self.sample_rate, self.channels, MAX_BLOCK);
                    latencies.push(p.latency());
                    next.slots.push(Some(Slot::new(s.clone(), p)));
                    s
                }
            };
            slot_shared.enabled.store(cfg.enabled, Ordering::Relaxed);
            for (a, v) in slot_shared.params.iter().zip(cfg.values()) {
                a.store(v);
            }
            next.ids.push(cfg.id);
            shared.push(slot_shared);
        }
        self.outgoing
            .push(Box::new(next))
            .map_err(|_| ControlError::Busy)?;
        self.slots = shared;
        self.latencies = latencies;
        self.shared.latency_frames.store(
            self.latencies.iter().sum::<usize>() as u32,
            Ordering::Relaxed,
        );
        Ok(())
    }

    fn slot(&self, id: u64) -> Result<&Arc<SlotShared>, ControlError> {
        self.slots
            .iter()
            .find(|s| s.id == id)
            .ok_or(ControlError::UnknownEffect(id))
    }

    pub fn set_param(&self, id: u64, key: &str, value: f32) -> Result<f32, ControlError> {
        let slot = self.slot(id)?;
        let (i, spec) = slot
            .kind
            .params()
            .iter()
            .enumerate()
            .find(|(_, p)| p.key == key)
            .ok_or_else(|| ControlError::UnknownParam(key.into()))?;
        let v = spec.clamp(value);
        slot.params[i].store(v);
        Ok(v)
    }

    pub fn set_enabled(&self, id: u64, enabled: bool) -> Result<(), ControlError> {
        self.slot(id)?.enabled.store(enabled, Ordering::Relaxed);
        Ok(())
    }

    pub fn meter(&self, id: u64) -> f32 {
        self.slot(id).map_or(0.0, |s| s.meter.load())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain_cfg(id: u64, db: f32) -> EffectConfig {
        let mut c = EffectConfig::new(id, EffectKind::Gain);
        c.params.insert("gain".into(), db);
        c
    }

    #[test]
    fn chain_swap_param_and_bypass() {
        let (mut ctl, mut eng) = new(48000.0, 2);
        ctl.set_chain(&[gain_cfg(1, 6.0)]).unwrap();
        let mut buf = vec![0.5f32; 2048];
        eng.process_interleaved(&mut buf);
        eng.process_interleaved(&mut buf.clone());
        let mut buf = vec![0.5f32; 2048];
        eng.process_interleaved(&mut buf);
        assert!((buf[2047] - 0.5 * 1.995).abs() < 0.01);

        // Parameter change is picked up without a chain swap.
        ctl.set_param(1, "gain", 0.0).unwrap();
        let mut buf = vec![0.5f32; 2048];
        eng.process_interleaved(&mut buf);
        assert!((buf[2047] - 0.5).abs() < 1e-3);

        // Disabling crossfades to dry.
        ctl.set_param(1, "gain", -36.0).unwrap();
        ctl.set_enabled(1, false).unwrap();
        let mut buf = vec![0.5f32; 4096];
        eng.process_interleaved(&mut buf);
        assert!((buf[4095] - 0.5).abs() < 1e-6);

        // Unknown targets are rejected.
        assert_eq!(
            ctl.set_enabled(9, true),
            Err(ControlError::UnknownEffect(9))
        );
        assert!(ctl.set_param(1, "nope", 0.0).is_err());
    }

    #[test]
    fn reordering_keeps_processor_instances() {
        let (mut ctl, mut eng) = new(48000.0, 1);
        ctl.set_chain(&[gain_cfg(1, 0.0), gain_cfg(2, 0.0)])
            .unwrap();
        eng.process_interleaved(&mut [0.0; 64]);
        ctl.set_chain(&[gain_cfg(2, 0.0), gain_cfg(1, 0.0)])
            .unwrap();
        eng.process_interleaved(&mut [0.0; 64]);
        assert!(eng.chain.slots.iter().all(|s| s.is_some()));
        assert_eq!(eng.chain.ids, vec![2, 1]);
        ctl.collect_garbage();
    }
}
