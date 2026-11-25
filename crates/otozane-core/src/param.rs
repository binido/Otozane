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

/// Owned parameter description. Built-in effects use static [`ParamSpec`]s;
/// hosted plugins only know their parameters at runtime, so the controller
/// and the IPC protocol work with this type.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParamDesc {
    pub key: String,
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    #[serde(default)]
    pub unit: String,
    /// Number of discrete steps between min and max (0 = continuous).
    #[serde(default)]
    pub steps: u32,
    #[serde(default)]
    pub log: bool,
    /// Labels for each discrete value, when known.
    #[serde(default)]
    pub choices: Vec<String>,
}

impl ParamDesc {
    /// Makes the description safe for `f32::clamp`, JSON, and the UI. A
    /// hosted plugin can report a non-finite default or range.
    pub fn sanitize(&mut self) {
        if !self.min.is_finite() {
            self.min = 0.0;
        }
        if !self.max.is_finite() {
            self.max = 1.0;
        }
        if self.min > self.max {
            std::mem::swap(&mut self.min, &mut self.max);
        }
        if !self.default.is_finite() {
            self.default = self.min;
        }
        self.default = self.default.clamp(self.min, self.max);
    }

    pub fn clamp(&self, v: f32) -> f32 {
        if v.is_nan() {
            self.default
        } else {
            v.clamp(self.min, self.max)
        }
    }
}

impl From<&ParamSpec> for ParamDesc {
    fn from(s: &ParamSpec) -> Self {
        let (steps, log, choices) = match s.kind {
            ParamKind::Continuous => (0, false, Vec::new()),
            ParamKind::Logarithmic => (0, true, Vec::new()),
            ParamKind::Toggle => (1, false, Vec::new()),
            ParamKind::Choice(labels) => (
                labels.len().saturating_sub(1) as u32,
                false,
                labels.iter().map(|l| l.to_string()).collect(),
            ),
        };
        Self {
            key: s.key.into(),
            name: s.name.into(),
            min: s.min,
            max: s.max,
            default: s.default,
            unit: s.unit.into(),
            steps,
            log,
            choices,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_descriptions_are_sanitized() {
        let mut d = ParamDesc {
            key: "p".into(),
            name: "P".into(),
            min: f32::NAN,
            max: f32::INFINITY,
            default: f32::NAN,
            unit: String::new(),
            steps: 0,
            log: false,
            choices: Vec::new(),
        };
        d.sanitize();
        assert!(d.min.is_finite() && d.max.is_finite() && d.default.is_finite());
        assert!(d.min <= d.max);
        assert!(d.default >= d.min && d.default <= d.max);
        assert!(d.clamp(f32::NAN).is_finite());
    }
}
