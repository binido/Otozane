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
    /// For plugins, keys are parameter ids and values are normalized (0..1).
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
    /// The hosted plugin, when `kind` is `plugin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<PluginRef>,
    /// Opaque plugin state (base64), restored before `params` are applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginFormat {
    Vst3,
    Jsfx,
}

impl PluginFormat {
    pub fn name(self) -> &'static str {
        match self {
            PluginFormat::Vst3 => "VST3",
            PluginFormat::Jsfx => "JSFX",
        }
    }
}

/// Identifies a plugin class: the file it lives in and its id within it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginRef {
    pub format: PluginFormat,
    /// Bundle or script path.
    pub path: String,
    /// Class id inside the file (VST3: 32 hex digits).
    pub uid: String,
    pub name: String,
    #[serde(default)]
    pub vendor: String,
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
            plugin: None,
            state: None,
        }
    }

    pub fn new_plugin(id: u64, plugin: PluginRef) -> Self {
        Self {
            id,
            kind: EffectKind::Plugin,
            enabled: true,
            params: BTreeMap::new(),
            plugin: Some(plugin),
            state: None,
        }
    }

    /// Display name: the plugin name for plugins, the effect name otherwise.
    pub fn display_name(&self) -> &str {
        match &self.plugin {
            Some(p) => &p.name,
            None => self.kind.name(),
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
    /// Extra plugin search directories, in addition to the standard ones.
    pub plugin_dirs: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            input_device: None,
            processing_enabled: true,
            active_profile: "Default".into(),
            channels: 1,
            plugin_dirs: Vec::new(),
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
