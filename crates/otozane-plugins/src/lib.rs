//! Third-party plugin hosting. Plugins run in the process that loads them;
//! there is no crash isolation for instances in the chain.

pub mod vst3;

use otozane_core::config::{PluginFormat, PluginRef};
use otozane_core::effects::Processor;
use otozane_core::param::ParamDesc;
use otozane_core::protocol::{PluginInfo, PluginList};

/// Non-real-time access to a loaded plugin instance.
pub trait PluginControl: Send {
    /// Display text for a normalized parameter value.
    fn value_text(&self, index: usize, value: f32) -> String;
    /// Mirrors a parameter change to the plugin's controller.
    fn set_value(&self, index: usize, value: f32);
    fn save_state(&self) -> Result<Vec<u8>, String>;
    /// The plugin asked the host to restart it (e.g. its latency changed).
    fn needs_restart(&self) -> bool;
}

/// A loaded plugin: the audio half goes to the engine, the control half stays with the host.
pub struct Instance {
    pub processor: Box<dyn Processor>,
    pub control: Box<dyn PluginControl>,
    pub params: Vec<ParamDesc>,
    pub values: Vec<f32>,
}

pub fn instantiate(
    plugin: &PluginRef,
    state: Option<&[u8]>,
    sample_rate: f32,
    channels: usize,
    max_block: usize,
) -> Result<Instance, String> {
    match plugin.format {
        PluginFormat::Vst3 => vst3::instantiate(plugin, state, sample_rate, channels, max_block),
        PluginFormat::Jsfx => Err("JSFX hosting is not implemented yet".into()),
    }
}

/// Hex encoding for plugin state stored in profiles.
pub fn encode_state(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn decode_state(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Effects in one bundle, as printed by the scan helper. Runs in the helper process.
pub fn scan_bundle(path: &std::path::Path) -> Result<Vec<PluginInfo>, String> {
    Ok(vst3::scan(path)?
        .into_iter()
        .map(|(plugin, category)| PluginInfo { plugin, category })
        .collect())
}

/// Scans every bundle in `dirs`, each in its own `scanner --scan PATH`
/// process, so a plugin that crashes or hangs while loading only fails its
/// own entry.
pub fn scan_dirs(dirs: &[std::path::PathBuf], scanner: &std::path::Path) -> PluginList {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    const TIMEOUT: Duration = Duration::from_secs(20);
    let mut list = PluginList {
        dirs: dirs.iter().map(|d| d.display().to_string()).collect(),
        ..Default::default()
    };
    let bundles: std::collections::BTreeSet<_> =
        dirs.iter().flat_map(|d| vst3::find_bundles(d)).collect();
    for bundle in bundles {
        let name = bundle.display().to_string();
        let child = Command::new(scanner)
            .arg("--scan")
            .arg(&bundle)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                list.errors
                    .push((name, format!("cannot start scanner: {e}")));
                continue;
            }
        };
        // Read stdout on a thread so a large listing cannot block the child.
        let mut out = child.stdout.take().expect("piped");
        let reader = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = std::io::Read::read_to_string(&mut out, &mut s);
            s
        });
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if start.elapsed() < TIMEOUT => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let stdout = reader.join().unwrap_or_default();
        match status {
            None => list.errors.push((name, "timed out while loading".into())),
            Some(s) if !s.success() => {
                let why = stdout
                    .trim()
                    .strip_prefix("error: ")
                    .unwrap_or("crashed while loading");
                list.errors.push((name, format!("{why} ({s})")));
            }
            Some(_) => match serde_json::from_str::<Vec<PluginInfo>>(&stdout) {
                Ok(v) => list.plugins.extend(v),
                Err(e) => list
                    .errors
                    .push((name, format!("invalid scanner output: {e}"))),
            },
        }
    }
    list.plugins.sort_by(|a, b| {
        a.plugin
            .name
            .to_lowercase()
            .cmp(&b.plugin.name.to_lowercase())
    });
    list
}

/// Standard plugin directories plus `extra`.
pub fn search_dirs(extra: &[String]) -> Vec<std::path::PathBuf> {
    let mut v = vst3::default_dirs();
    v.extend(extra.iter().map(std::path::PathBuf::from));
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    #[test]
    fn state_hex_round_trip() {
        let b = vec![0u8, 1, 0xab, 0xff];
        assert_eq!(super::decode_state(&super::encode_state(&b)), Some(b));
        assert_eq!(super::decode_state("abc"), None);
        assert_eq!(super::decode_state("zz"), None);
    }
}
