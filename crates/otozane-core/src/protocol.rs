//! IPC protocol between `otozaned` and clients: one JSON object per line in
//! each direction over a Unix socket. Each request gets exactly one response.

use crate::config::{EffectConfig, PluginRef, Profile};
use crate::param::ParamDesc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Status,
    /// `None` follows the system default source (never a virtual one).
    SetInput {
        device: Option<String>,
    },
    SetProcessing {
        enabled: bool,
    },
    /// Stops capturing; the virtual microphone outputs silence. Not saved.
    SetMuted {
        muted: bool,
    },
    /// Plays the processed microphone to the system default output. Not saved.
    SetMonitor {
        on: bool,
    },
    SetChain {
        chain: Vec<EffectConfig>,
    },
    SetParam {
        id: u64,
        key: String,
        value: f32,
    },
    SetEffectEnabled {
        id: u64,
        enabled: bool,
    },
    ListProfiles,
    /// Switches to a profile. The current chain is saved to the old profile first.
    LoadProfile {
        name: String,
    },
    /// Saves the current chain under `name` and makes it the active profile.
    SaveProfileAs {
        name: String,
    },
    DeleteProfile {
        name: String,
    },
    RenameProfile {
        from: String,
        to: String,
    },
    /// Returns a profile's contents (for export).
    GetProfile {
        name: String,
    },
    /// Stores a profile under `name`, or `name 2`, `name 3`... if taken.
    /// Does not switch to it.
    ImportProfile {
        name: String,
        profile: Profile,
    },
    /// Returns the last plugin scan result.
    ListPlugins,
    /// Rescans the plugin directories in the background.
    ScanPlugins,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Error { message: String },
    Status(Box<Status>),
    Profiles { names: Vec<String> },
    Plugins(Box<PluginList>),
    Profile { name: String, profile: Profile },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    pub plugin: PluginRef,
    /// Category path, e.g. `Fx|Dynamics|Mono`.
    pub category: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginList {
    pub scanning: bool,
    pub dirs: Vec<String>,
    pub plugins: Vec<PluginInfo>,
    /// Files that could not be scanned, with the reason.
    pub errors: Vec<(String, String)>,
}

/// Runtime details of a chain entry that the configuration does not hold.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EffectInfo {
    /// Why the effect is inactive (e.g. the plugin failed to load).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Plugin parameters (empty for built-in effects, which use their static specs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<ParamDesc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<f32>,
    /// Display text for each value, as formatted by the plugin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub description: String,
    pub is_virtual: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum CaptureState {
    Starting,
    WaitingForDevice(Option<String>),
    Connecting(String),
    Running(String),
    /// Capture stopped on request ([`Request::SetMuted`]).
    Muted,
    Error(String),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Diagnostics {
    pub sample_rate: u32,
    pub channels: u32,
    pub chain_latency_frames: u32,
    pub capture_quantum: u32,
    pub playback_quantum: u32,
    pub ring_fill_frames: u32,
    pub underrun_frames: u64,
    pub overrun_frames: u64,
    pub skipped_frames: u64,
    pub processed_blocks: u64,
}

impl Diagnostics {
    /// Estimated input-to-output latency in milliseconds: one capture
    /// quantum, the chain's own latency, and audio queued between streams.
    pub fn estimated_latency_ms(&self) -> f32 {
        let frames = self.capture_quantum + self.chain_latency_frames + self.ring_fill_frames;
        frames as f32 * 1000.0 / self.sample_rate.max(1) as f32
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub capture: CaptureState,
    /// Configured input (`None` = automatic).
    pub input_device: Option<String>,
    pub devices: Vec<DeviceInfo>,
    pub virtual_source_ready: bool,
    pub processing_enabled: bool,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub monitoring: bool,
    /// Peak levels (linear) since the previous status request.
    pub input_peak: f32,
    pub output_peak: f32,
    pub chain: Vec<EffectConfig>,
    /// Per-effect meter values, parallel to `chain`.
    pub meters: Vec<f32>,
    /// Per-effect runtime details, parallel to `chain`.
    #[serde(default)]
    pub effects: Vec<EffectInfo>,
    pub active_profile: String,
    pub diagnostics: Diagnostics,
}

/// Path of the service socket: `$OTOZANE_SOCKET` if set (used by tests),
/// else `$XDG_RUNTIME_DIR/otozane.sock`.
pub fn socket_path() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("OTOZANE_SOCKET") {
        return p.into();
    }
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => std::path::PathBuf::from(dir).join("otozane.sock"),
        // SAFETY: getuid has no preconditions and cannot fail.
        None => std::env::temp_dir().join(format!("otozane-{}.sock", unsafe { getuid() })),
    }
}

unsafe extern "C" {
    fn getuid() -> u32;
}

/// Blocking client for the service socket.
#[cfg(unix)]
pub struct Client {
    reader: std::io::BufReader<std::os::unix::net::UnixStream>,
    writer: std::os::unix::net::UnixStream,
}

#[cfg(unix)]
impl Client {
    pub fn connect() -> std::io::Result<Self> {
        let stream = std::os::unix::net::UnixStream::connect(socket_path())?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        Ok(Self {
            reader: std::io::BufReader::new(stream.try_clone()?),
            writer: stream,
        })
    }

    pub fn request(&mut self, req: &Request) -> std::io::Result<Response> {
        use std::io::{BufRead, Write};
        let mut line = serde_json::to_string(req)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        let mut resp = String::new();
        if self.reader.read_line(&mut resp)? == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        Ok(serde_json::from_str(&resp)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_use_tagged_json() {
        let r = Request::SetParam {
            id: 2,
            key: "gain".into(),
            value: 3.0,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, r#"{"cmd":"set_param","id":2,"key":"gain","value":3.0}"#);
        assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
        let ok = serde_json::to_string(&Response::Ok).unwrap();
        assert_eq!(ok, r#"{"type":"ok"}"#);
    }
}
