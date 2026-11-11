use std::sync::atomic::{AtomicU32, Ordering};

/// Lock-free `f32` cell used to pass parameter values and meter readings
/// between the control thread and the real-time thread.
#[derive(Debug, Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
    pub fn swap(&self, v: f32) -> f32 {
        f32::from_bits(self.0.swap(v.to_bits(), Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamKind {
    Continuous,
    /// Continuous, but best edited on a logarithmic scale (frequencies, times).
    Logarithmic,
    Toggle,
    /// Discrete choice; the value is an index into the labels.
    Choice(&'static [&'static str]),
}

#[derive(Debug, Clone, Copy)]
pub struct ParamSpec {
    pub key: &'static str,
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub unit: &'static str,
    pub kind: ParamKind,
}

impl ParamSpec {
    pub const fn new(
        key: &'static str,
        name: &'static str,
        min: f32,
        max: f32,
        default: f32,
        unit: &'static str,
    ) -> Self {
        Self {
            key,
            name,
            min,
            max,
            default,
            unit,
            kind: ParamKind::Continuous,
        }
    }
    pub const fn log(mut self) -> Self {
        self.kind = ParamKind::Logarithmic;
        self
    }
    pub const fn choice(mut self, labels: &'static [&'static str]) -> Self {
        self.kind = ParamKind::Choice(labels);
        self
    }
    pub fn clamp(&self, v: f32) -> f32 {
        if v.is_nan() {
            self.default
        } else {
            v.clamp(self.min, self.max)
        }
    }
}
