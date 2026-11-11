//! Serializable chain, profile, and service settings.

use crate::effects::EffectKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectConfig {
    /// Stable identifier, unique within a chain. Used to keep effect state
    /// across chain edits.
    pub id: u64,
    pub kind: EffectKind,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Parameter values by key. Missing keys use the default; unknown keys are ignored.
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
}

fn yes() -> bool {
    true
}

impl EffectConfig {
    pub fn new(id: u64, kind: EffectKind) -> Self {
        let params = kind
            .params()
            .iter()
            .map(|p| (p.key.to_string(), p.default))
            .collect();
        Self {
            id,
            kind,
            enabled: true,
            params,
        }
    }

    /// Parameter values in declaration order, clamped to their ranges.
    pub fn values(&self) -> Vec<f32> {
        self.kind
            .params()
            .iter()
            .map(|p| p.clamp(self.params.get(p.key).copied().unwrap_or(p.default)))
            .collect()
    }
}

/// Returns `$XDG_CONFIG_HOME/otozane`, falling back to `~/.config/otozane`.
pub fn config_dir() -> std::path::PathBuf {
    use std::path::PathBuf;
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("otozane")
}

/// Returns an id not used by any effect in `chain`.
pub fn next_id(chain: &[EffectConfig]) -> u64 {
    chain.iter().map(|e| e.id).max().map_or(1, |m| m + 1)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub chain: Vec<EffectConfig>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            chain: vec![
                EffectConfig::new(1, EffectKind::Rnnoise),
                EffectConfig::new(2, EffectKind::Gain),
            ],
        }
    }
}

/// Persistent service settings (`config.toml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// PipeWire `node.name` of the selected microphone. `None` picks the
    /// first available physical microphone.
    pub input_device: Option<String>,
    pub processing_enabled: bool,
    pub active_profile: String,
    /// Capture channel count: 1 (mono) or 2 (stereo).
    pub channels: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            input_device: None,
            processing_enabled: true,
            active_profile: "Default".into(),
            channels: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_roundtrips_and_tolerates_missing_params() {
        let p = Profile::default();
        let s = toml::to_string(&p).unwrap();
        assert_eq!(toml::from_str::<Profile>(&s).unwrap(), p);

        let e: EffectConfig = toml::from_str("id = 3\nkind = \"gain\"\n").unwrap();
        assert!(e.enabled);
        assert_eq!(e.values(), vec![0.0]);
    }
}
