//! Real-time engine and its non-real-time controller.

use crate::config::EffectConfig;
use crate::effects::{EffectKind, Processor};
use crate::param::{AtomicF32, ParamDesc};
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

/// An effect instance built outside the core, e.g. a hosted plugin.
pub struct External {
    pub processor: Box<dyn Processor>,
    pub params: Vec<ParamDesc>,
    /// Current parameter values reported by the instance (e.g. after restoring
    /// its state), used where the configuration has no value.
    pub values: Vec<f32>,
}

/// Builds [`External`] instances for chain entries the core cannot create itself.
pub type Factory<'a> = dyn FnMut(&EffectConfig) -> Result<External, String> + 'a;

/// Controller-side bookkeeping for one chain slot.
struct SlotInfo {
    shared: Arc<SlotShared>,
    plugin: Option<crate::config::PluginRef>,
    params: Arc<[ParamDesc]>,
    latency: usize,
    error: Option<String>,
}

/// Non-real-time half of the engine. Builds effects and sends them to [`Engine`].
pub struct Controller {
    sample_rate: f32,
    channels: usize,
    outgoing: rtrb::Producer<Box<Chain>>,
    garbage: rtrb::Consumer<Box<Chain>>,
    slots: Vec<SlotInfo>,
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

    /// Replaces the chain. Effects whose id, kind and plugin match an existing
    /// slot keep their processor and internal state. New plugin entries are
    /// created by `factory`; a failed plugin becomes a pass-through slot and
    /// its error is reported by [`Controller::effect_error`].
    pub fn set_chain(
        &mut self,
        chain: &[EffectConfig],
        factory: &mut Factory<'_>,
    ) -> Result<(), ControlError> {
        self.collect_garbage();
        if self.outgoing.slots() == 0 {
            return Err(ControlError::Busy);
        }
        let mut next = Chain {
            ids: Vec::with_capacity(chain.len()),
            slots: Vec::with_capacity(chain.len()),
        };
        let mut infos = Vec::with_capacity(chain.len());
        for cfg in chain {
            let existing = self.slots.iter().position(|s| {
                s.shared.id == cfg.id && s.shared.kind == cfg.kind && s.plugin == cfg.plugin
            });
            let info = match existing {
                Some(i) => {
                    next.slots.push(None);
                    let old = &self.slots[i];
                    SlotInfo {
                        shared: old.shared.clone(),
                        plugin: old.plugin.clone(),
                        params: old.params.clone(),
                        latency: old.latency,
                        error: old.error.clone(),
                    }
                }
                None => {
                    let (processor, params, fallback, error) = if cfg.kind == EffectKind::Plugin {
                        match factory(cfg) {
                            Ok(e) => (e.processor, e.params, e.values, None),
                            Err(e) => (
                                Box::new(crate::effects::Passthrough) as Box<dyn Processor>,
                                Vec::new(),
                                Vec::new(),
                                Some(e),
                            ),
                        }
                    } else {
                        let params: Vec<ParamDesc> =
                            cfg.kind.params().iter().map(ParamDesc::from).collect();
                        let p = cfg.kind.create(self.sample_rate, self.channels, MAX_BLOCK);
                        let defaults = params.iter().map(|d| d.default).collect();
                        (p, params, defaults, None)
                    };
                    let values = values_for(cfg, &params, &fallback);
                    let shared = Arc::new(SlotShared {
                        id: cfg.id,
                        kind: cfg.kind,
                        enabled: AtomicBool::new(cfg.enabled),
                        params: values.into_iter().map(AtomicF32::new).collect(),
                        meter: AtomicF32::default(),
                    });
                    let latency = processor.latency();
                    next.slots.push(Some(Slot::new(shared.clone(), processor)));
                    SlotInfo {
                        shared,
                        plugin: cfg.plugin.clone(),
                        params: params.into(),
                        latency,
                        error,
                    }
                }
            };
            info.shared.enabled.store(cfg.enabled, Ordering::Relaxed);
            let current: Vec<f32> = info.shared.params.iter().map(|a| a.load()).collect();
            for (a, v) in info
                .shared
                .params
                .iter()
                .zip(values_for(cfg, &info.params, &current))
            {
                a.store(v);
            }
            next.ids.push(cfg.id);
            infos.push(info);
        }
        self.outgoing
            .push(Box::new(next))
            .map_err(|_| ControlError::Busy)?;
        self.slots = infos;
        self.shared.latency_frames.store(
            self.slots.iter().map(|s| s.latency).sum::<usize>() as u32,
            Ordering::Relaxed,
        );
        Ok(())
    }

    fn slot(&self, id: u64) -> Result<&SlotInfo, ControlError> {
        self.slots
            .iter()
            .find(|s| s.shared.id == id)
            .ok_or(ControlError::UnknownEffect(id))
    }

    pub fn set_param(&self, id: u64, key: &str, value: f32) -> Result<f32, ControlError> {
        let slot = self.slot(id)?;
        let (i, desc) = slot
            .params
            .iter()
            .enumerate()
            .find(|(_, p)| p.key == key)
            .ok_or_else(|| ControlError::UnknownParam(key.into()))?;
        let v = desc.clamp(value);
        slot.shared.params[i].store(v);
        Ok(v)
    }

    /// Parameter descriptions of an effect.
    pub fn params(&self, id: u64) -> Arc<[ParamDesc]> {
        self.slot(id)
            .map_or_else(|_| Arc::from([]), |s| s.params.clone())
    }

    /// Current parameter values of an effect, parallel to [`Controller::params`].
    pub fn values(&self, id: u64) -> Vec<f32> {
        self.slot(id)
            .map(|s| s.shared.params.iter().map(|a| a.load()).collect())
            .unwrap_or_default()
    }

    /// Why an effect is inactive, e.g. a plugin that failed to load.
    pub fn effect_error(&self, id: u64) -> Option<String> {
        self.slot(id).ok().and_then(|s| s.error.clone())
    }

    pub fn set_enabled(&self, id: u64, enabled: bool) -> Result<(), ControlError> {
        self.slot(id)?
            .shared
            .enabled
            .store(enabled, Ordering::Relaxed);
        Ok(())
    }

    pub fn meter(&self, id: u64) -> f32 {
        self.slot(id).map_or(0.0, |s| s.shared.meter.load())
    }
}

/// Values for `descs` taken from the configuration, else `fallback`, else defaults.
fn values_for(cfg: &EffectConfig, descs: &[ParamDesc], fallback: &[f32]) -> Vec<f32> {
    descs
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let v = cfg
                .params
                .get(&d.key)
                .copied()
                .or_else(|| fallback.get(i).copied())
                .unwrap_or(d.default);
            d.clamp(v)
        })
        .collect()
}

/// Factory for chains without plugins.
pub fn no_plugins(cfg: &EffectConfig) -> Result<External, String> {
    Err(format!(
        "plugin hosting unavailable for '{}'",
        cfg.display_name()
    ))
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
        ctl.set_chain(&[gain_cfg(1, 6.0)], &mut no_plugins).unwrap();
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
        ctl.set_chain(&[gain_cfg(1, 0.0), gain_cfg(2, 0.0)], &mut no_plugins)
            .unwrap();
        eng.process_interleaved(&mut [0.0; 64]);
        ctl.set_chain(&[gain_cfg(2, 0.0), gain_cfg(1, 0.0)], &mut no_plugins)
            .unwrap();
        eng.process_interleaved(&mut [0.0; 64]);
        assert!(eng.chain.slots.iter().all(|s| s.is_some()));
        assert_eq!(eng.chain.ids, vec![2, 1]);
        ctl.collect_garbage();
    }
}
