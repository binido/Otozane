//! Third-party plugin hosting. Plugins run in the process that loads them;
//! there is no crash isolation for instances in the chain.

pub mod jsfx;
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
        PluginFormat::Jsfx => jsfx::instantiate(plugin, state, sample_rate, channels, max_block),
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

/// Scans every VST3 bundle in `dirs`, each in its own `scanner --scan PATH`
/// process, so a plugin that crashes or hangs while loading only fails its
/// own entry. JSFX scripts are only parsed, so they are read in this process.
pub fn scan_dirs(
    dirs: &[std::path::PathBuf],
    scanner: &std::path::Path,
    cancelled: impl Fn() -> bool,
) -> Option<PluginList> {
    use std::process::{Command, Stdio};
    let mut list = PluginList {
        dirs: dirs.iter().map(|d| d.display().to_string()).collect(),
        ..Default::default()
    };
    let bundles: std::collections::BTreeSet<_> =
        dirs.iter().flat_map(|d| vst3::find_bundles(d)).collect();
    for bundle in bundles {
        if cancelled() {
            return None;
        }
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
        let outcome = read_scanner_output(&mut child, &cancelled);
        if !matches!(outcome, Ok(Some(_))) {
            let _ = child.kill();
            let _ = child.wait();
        }
        if cancelled() {
            return None;
        }
        match outcome {
            Err(e) => list.errors.push((name, format!("scanner output: {e}"))),
            Ok(None) => list.errors.push((name, "timed out while loading".into())),
            Ok(Some((s, stdout))) if !s.success() => {
                let stdout = String::from_utf8_lossy(&stdout);
                let why = stdout
                    .trim()
                    .strip_prefix("error: ")
                    .unwrap_or("crashed while loading");
                list.errors.push((name, format!("{why} ({s})")));
            }
            Ok(Some((_, stdout))) => match serde_json::from_slice::<Vec<PluginInfo>>(&stdout) {
                Ok(v) => list.plugins.extend(v),
                Err(e) => list
                    .errors
                    .push((name, format!("invalid scanner output: {e}"))),
            },
        }
    }
    let scripts: std::collections::BTreeSet<_> =
        dirs.iter().flat_map(|d| jsfx::find_scripts(d)).collect();
    for script in scripts {
        if cancelled() {
            return None;
        }
        match jsfx::scan(&script) {
            Ok((plugin, category)) => list.plugins.push(PluginInfo { plugin, category }),
            Err(e) => list.errors.push((script.display().to_string(), e)),
        }
    }
    list.plugins.sort_by(|a, b| {
        a.plugin
            .name
            .to_lowercase()
            .cmp(&b.plugin.name.to_lowercase())
    });
    (!cancelled()).then_some(list)
}

fn read_scanner_output(
    child: &mut std::process::Child,
    cancelled: impl Fn() -> bool,
) -> std::io::Result<Option<(std::process::ExitStatus, Vec<u8>)>> {
    use std::io::{Error, ErrorKind, Read};
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    let mut stdout = child.stdout.take().expect("piped");
    // SAFETY: stdout owns the descriptor for both calls.
    let flags = unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL) };
    if flags == -1
        || unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
    {
        return Err(Error::last_os_error());
    }

    let limit = Instant::now() + Duration::from_secs(20);
    let mut drain_limit = None;
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    while !cancelled() {
        let status = child.try_wait()?;
        if status.is_some() {
            drain_limit.get_or_insert_with(|| Instant::now() + Duration::from_millis(250));
        }
        if Instant::now() >= drain_limit.unwrap_or(limit) {
            return Ok(None);
        }
        match stdout.read(&mut buffer) {
            Ok(0) => {}
            Ok(n) => {
                output.extend_from_slice(&buffer[..n]);
                continue;
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
        // Once the scanner exits, a descendant holding stdout open cannot delay the result.
        if let Some(status) = status {
            return Ok(Some((status, output)));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(None)
}

/// Standard plugin directories plus `extra`.
pub fn search_dirs(extra: &[String]) -> Vec<std::path::PathBuf> {
    let mut v = vst3::default_dirs();
    v.extend(jsfx::default_dirs());
    v.extend(extra.iter().map(std::path::PathBuf::from));
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn scanner_finishes_while_descendant_keeps_stdout_open() {
        let dir = tempfile::tempdir().unwrap();
        let mut scanner = Command::new("sh")
            .arg("-c")
            .arg("sleep 6 & echo $! > pid; printf '%131072s[]' ''")
            .current_dir(dir.path())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let start = Instant::now();
        let result = super::read_scanner_output(&mut scanner, || false);
        let elapsed = start.elapsed();
        let pid: libc::pid_t = std::fs::read_to_string(dir.path().join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // SAFETY: the PID belongs to the sleep process started by this test.
        unsafe { libc::kill(pid, libc::SIGTERM) };
        let (status, output) = result.unwrap().unwrap();
        assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
        assert!(status.success());
        assert_eq!(output.len(), 131_074);
        assert!(output.ends_with(b"[]"));
    }

    #[test]
    fn state_hex_round_trip() {
        let b = vec![0u8, 1, 0xab, 0xff];
        assert_eq!(super::decode_state(&super::encode_state(&b)), Some(b));
        assert_eq!(super::decode_state("abc"), None);
        assert_eq!(super::decode_state("zz"), None);
    }
}
