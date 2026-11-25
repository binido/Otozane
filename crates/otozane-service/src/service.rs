//! Request handling and persistence for the running service.

use crate::backend::{Backend, CaptureState};
use crate::store::{Store, validate_name};
use otozane_core::config::{EffectConfig, Profile, Settings};
use otozane_core::effects::EffectKind;
use otozane_core::engine::{ControlError, Controller, External};
use otozane_core::protocol::{self, Diagnostics, EffectInfo, PluginList, Request, Response};
use otozane_plugins::PluginControl;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Time to let the audio thread run a removed effect's 10 ms fade-out before
/// swapping the chain. A capture callback may carry a full 1024-frame block
/// (21.3 ms at 48 kHz), so the previous 15 ms could swap before the fade ran.
const FADE_OUT_WAIT: Duration = Duration::from_millis(25);

pub struct Service {
    pub ctl: Controller,
    pub backend: Backend,
    pub store: Store,
    pub settings: Settings,
    pub chain: Vec<EffectConfig>,
    /// Control halves of loaded plugins, by effect id.
    plugins: HashMap<u64, Box<dyn PluginControl>>,
    scan: Arc<Mutex<PluginList>>,
    scan_thread: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
    dirty_settings: bool,
    /// Capture stopped on request. Not saved: a restart always unmutes.
    muted: bool,
    /// Monitor playback on. Not saved, so speakers never start feeding back.
    monitoring: bool,
    dirty_profile: bool,
}

fn err(e: impl std::fmt::Display) -> Response {
    Response::Error {
        message: e.to_string(),
    }
}

/// A free profile name: the base name, else ` 2`, ` 3`... Checks both the
/// listing (ASCII case) and the filesystem (macOS folds case and Unicode).
fn free_profile_name(store: &Store, base: &str) -> String {
    let taken = store.list_profiles();
    (1..)
        .map(|i| {
            if i == 1 {
                base.to_string()
            } else {
                format!("{base} {i}")
            }
        })
        .find(|n| !taken.iter().any(|t| t.eq_ignore_ascii_case(n)) && !store.profile_exists(n))
        .unwrap()
}

impl Service {
    pub fn new(ctl: Controller, backend: Backend, store: Store, settings: Settings) -> Self {
        let cache = store.dir().join("plugins.json");
        let scan = match std::fs::read_to_string(&cache) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                // A truncated or old-schema cache must not suppress a rescan.
                eprintln!("{}: {e}; rescanning", cache.display());
                let _ = std::fs::remove_file(&cache);
                PluginList::default()
            }),
            Err(_) => PluginList::default(),
        };
        Self {
            ctl,
            backend,
            store,
            settings,
            chain: Vec::new(),
            plugins: HashMap::new(),
            scan: Arc::new(Mutex::new(scan)),
            scan_thread: None,
            dirty_settings: false,
            muted: false,
            monitoring: false,
            dirty_profile: false,
        }
    }

    /// Periodic housekeeping: frees retired chains and writes pending changes.
    pub fn tick(&mut self) {
        self.ctl.collect_garbage();
        let _ = self.flush();
    }

    /// Writes pending changes. Returns the first failure; pending flags stay
    /// set so the tick retries. A plugin-state capture failure is reported even
    /// though the chain itself may be saved.
    pub fn flush(&mut self) -> Result<(), String> {
        let mut result = Ok(());
        if self.dirty_profile {
            if let Err(e) = self.capture_plugin_states() {
                result = Err(e);
            }
            let p = Profile {
                chain: self.chain.clone(),
            };
            match self.store.save_profile(&self.settings.active_profile, &p) {
                Ok(()) => self.dirty_profile = false,
                Err(e) => {
                    let msg = format!("failed to save profile: {e}");
                    eprintln!("{msg}");
                    result = Err(msg);
                }
            }
        }
        if self.dirty_settings {
            match self.store.save_settings(&self.settings) {
                Ok(()) => self.dirty_settings = false,
                Err(e) => {
                    let msg = format!("failed to save settings: {e}");
                    eprintln!("{msg}");
                    result = Err(msg);
                }
            }
        }
        result
    }

    /// Copies each plugin's current state into the chain configuration.
    /// Returns the first failure; a failed plugin keeps its previous state.
    fn capture_plugin_states(&mut self) -> Result<(), String> {
        let mut result = Ok(());
        for e in &mut self.chain {
            if let Some(c) = self.plugins.get(&e.id) {
                match c.save_state() {
                    Ok(b) => e.state = Some(otozane_plugins::encode_state(&b)),
                    Err(err) => {
                        let msg = format!("{}: {err}", e.display_name());
                        eprintln!("{msg}");
                        result = result.and(Err(msg));
                    }
                }
            }
        }
        result
    }

    pub fn apply_chain(&mut self, chain: Vec<EffectConfig>) -> Result<(), String> {
        self.build_chain(chain, self.settings.channels as usize, false)
    }

    /// Rebuilds the chain for a new channel count.
    fn set_channels(&mut self, channels: u32) -> Result<(), String> {
        if !(1..=2).contains(&channels) {
            return Err("channels must be 1 or 2".into());
        }
        if channels == self.settings.channels {
            return Ok(());
        }
        // Plugins are recreated from the state saved into the chain.
        // A failed save would lose the plugin's recent changes, so nothing is switched.
        self.capture_plugin_states()?;
        self.build_chain(self.chain.clone(), channels as usize, false)?;
        #[cfg(target_os = "linux")]
        self.backend.set_channels(channels as usize);
        self.settings.channels = channels;
        self.dirty_settings = true;
        Ok(())
    }

    fn build_chain(
        &mut self,
        mut chain: Vec<EffectConfig>,
        channels: usize,
        fresh_plugins: bool,
    ) -> Result<(), String> {
        // A hand-edited TOML profile can contain nan/inf, which JSON cannot
        // round-trip in Status.
        for e in &mut chain {
            e.sanitize();
        }
        let mut ids = std::collections::HashSet::new();
        if !chain.iter().all(|e| ids.insert(e.id)) {
            return Err("effect ids must be unique".into());
        }
        // Fade out effects that are about to be removed so removal does not click.
        let mut faded = false;
        for old in &self.chain {
            let kept = chain
                .iter()
                .any(|e| e.id == old.id && e.kind == old.kind && e.plugin == old.plugin);
            if old.enabled && !kept {
                let _ = self.ctl.set_enabled(old.id, false);
                faded = true;
            }
        }
        if faded {
            std::thread::sleep(FADE_OUT_WAIT);
        }
        // The audio thread takes updates once per cycle (or per 100 ms pump
        // when idle), so a burst of edits may briefly find the queue full.
        let rebuild = channels != self.settings.channels as usize;
        let plugins = &mut self.plugins;
        let mut factory = |cfg: &EffectConfig| -> Result<External, String> {
            let plugin = cfg.plugin.as_ref().ok_or("plugin entry without a plugin")?;
            let state = cfg.state.as_deref().and_then(otozane_plugins::decode_state);
            let inst = match otozane_plugins::instantiate(
                plugin,
                state.as_deref(),
                otozane_core::SAMPLE_RATE as f32,
                channels,
                otozane_core::engine::MAX_BLOCK,
            ) {
                Ok(inst) => inst,
                Err(e) => {
                    // Do not keep a control from an earlier plugin that used
                    // this id: its state does not belong to this entry.
                    plugins.remove(&cfg.id);
                    return Err(format!("{}: {e}", plugin.name));
                }
            };
            plugins.insert(cfg.id, inst.control);
            Ok(External {
                processor: inst.processor,
                params: inst.params,
                values: inst.values,
            })
        };
        let mut tries = 0;
        loop {
            let r = if rebuild {
                self.ctl.set_channels(channels, &chain, &mut factory)
            } else if fresh_plugins {
                self.ctl.restore_chain(&chain, &mut factory)
            } else {
                self.ctl.set_chain(&chain, &mut factory)
            };
            match r {
                Err(ControlError::Busy) if tries < 60 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(5));
                }
                r => break r.map_err(|e| e.to_string())?,
            }
        }
        self.plugins.retain(|id, _| {
            chain
                .iter()
                .any(|e| e.id == *id && e.kind == EffectKind::Plugin)
        });
        self.chain = chain;
        self.dirty_profile = true;
        Ok(())
    }

    fn switch_profile(&mut self, name: &str) -> Result<(), String> {
        let p = self.store.load_profile(name).map_err(|e| e.to_string())?;
        self.flush()?;
        if self.dirty_profile || self.dirty_settings {
            return Err("could not save the current profile".into());
        }
        // The loaded profile's plugin state must win over the live instances.
        self.build_chain(p.chain, self.settings.channels as usize, true)?;
        self.settings.active_profile = name.to_string();
        self.dirty_settings = true;
        // The chain came from the file; no need to write it back.
        self.dirty_profile = false;
        Ok(())
    }

    pub fn handle(&mut self, req: Request) -> Response {
        match req {
            Request::Status => Response::Status(Box::new(self.status())),
            Request::Start => Response::Ok,
            Request::SetStopOnClose { enabled } => {
                let mut settings = self.settings.clone();
                settings.stop_on_close = enabled;
                match self.store.save_settings(&settings) {
                    Ok(()) => {
                        self.settings = settings;
                        self.dirty_settings = false;
                        Response::Ok
                    }
                    Err(e) => err(e),
                }
            }
            Request::SetInput { device } => {
                if device.as_deref() == Some(crate::backend::virtual_node_name()) {
                    return err(format!(
                        "{} cannot be its own input",
                        otozane_core::VIRTUAL_DEVICE
                    ));
                }
                self.backend.set_input(device.clone());
                self.settings.input_device = device;
                self.dirty_settings = true;
                Response::Ok
            }
            Request::SetProcessing { enabled } => {
                self.ctl
                    .shared
                    .processing_enabled
                    .store(enabled, Ordering::Relaxed);
                self.settings.processing_enabled = enabled;
                self.dirty_settings = true;
                Response::Ok
            }
            Request::SetMuted { muted } => {
                self.backend.set_muted(muted);
                self.muted = muted;
                Response::Ok
            }
            Request::SetMonitor { on } => {
                self.backend.set_monitor(on);
                self.monitoring = on;
                Response::Ok
            }
            Request::SetChannels { channels } => match self.set_channels(channels) {
                Ok(()) => Response::Ok,
                Err(e) => err(e),
            },
            Request::SetChain { chain } => match self.apply_chain(chain) {
                Ok(()) => Response::Ok,
                Err(e) => err(e),
            },
            Request::SetParam { id, key, value } => match self.ctl.set_param(id, &key, value) {
                Ok(v) => {
                    if let Some(c) = self.plugins.get(&id)
                        && let Some(i) = self.ctl.params(id).iter().position(|p| p.key == key)
                    {
                        c.set_value(i, v);
                    }
                    if let Some(e) = self.chain.iter_mut().find(|e| e.id == id) {
                        e.params.insert(key, v);
                    }
                    self.dirty_profile = true;
                    Response::Ok
                }
                Err(e) => err(e),
            },
            Request::SetEffectEnabled { id, enabled } => match self.ctl.set_enabled(id, enabled) {
                Ok(()) => {
                    if let Some(e) = self.chain.iter_mut().find(|e| e.id == id) {
                        e.enabled = enabled;
                    }
                    self.dirty_profile = true;
                    Response::Ok
                }
                Err(e) => err(e),
            },
            Request::ListProfiles => Response::Profiles {
                names: self.store.list_profiles(),
            },
            Request::LoadProfile { name } => match self.switch_profile(&name) {
                Ok(()) => Response::Ok,
                Err(e) => err(e),
            },
            Request::SaveProfileAs { name } => {
                if let Err(e) = validate_name(&name) {
                    return err(e);
                }
                // Re-saving the active profile under its exact name is allowed;
                // any other collision gets a suffix instead of replacing a file.
                let name = if name == self.settings.active_profile {
                    name
                } else {
                    free_profile_name(&self.store, &name)
                };
                if let Err(e) = self.flush() {
                    return err(e);
                }
                if self.dirty_profile {
                    return err("could not save the current profile");
                }
                self.settings.active_profile = name;
                self.dirty_settings = true;
                self.dirty_profile = true;
                if let Err(e) = self.flush() {
                    return err(e);
                }
                if self.dirty_profile || self.dirty_settings {
                    return err("could not save the profile");
                }
                Response::Ok
            }
            Request::DeleteProfile { name } => {
                if self
                    .store
                    .same_profile(&name, &self.settings.active_profile)
                {
                    return err("cannot delete the active profile");
                }
                match self.store.delete_profile(&name) {
                    Ok(()) => Response::Ok,
                    Err(e) => err(e),
                }
            }
            Request::RenameProfile { from, to } => {
                if let Err(e) = self.flush() {
                    return err(e);
                }
                match self.store.rename_profile(&from, &to) {
                    Ok(()) => {
                        if self
                            .store
                            .same_profile(&from, &self.settings.active_profile)
                        {
                            self.settings.active_profile = to;
                            self.dirty_settings = true;
                        }
                        Response::Ok
                    }
                    Err(e) => err(e),
                }
            }
            Request::GetProfile { name } => {
                let profile = if self
                    .store
                    .same_profile(&name, &self.settings.active_profile)
                {
                    let _ = self.capture_plugin_states();
                    Ok(Profile {
                        chain: self.chain.clone(),
                    })
                } else {
                    self.store.load_profile(&name)
                };
                match profile {
                    Ok(profile) => Response::Profile { name, profile },
                    Err(e) => err(format!("cannot read profile '{name}': {e}")),
                }
            }
            Request::ImportProfile { name, profile } => {
                if let Err(e) = validate_name(&name) {
                    return err(e);
                }
                let mut profile = profile;
                for e in &mut profile.chain {
                    e.sanitize();
                }
                let mut ids = std::collections::HashSet::new();
                if !profile.chain.iter().all(|e| ids.insert(e.id)) {
                    return err("effect ids must be unique");
                }
                let name = free_profile_name(&self.store, &name);
                match self.store.save_profile(&name, &profile) {
                    Ok(()) => Response::Ok,
                    Err(e) => err(e),
                }
            }
            Request::ListPlugins => Response::Plugins(Box::new(self.scan.lock().unwrap().clone())),
            Request::ScanPlugins => {
                self.start_scan();
                Response::Ok
            }
            Request::Shutdown => {
                let _ = self.flush();
                Response::Ok
            }
        }
    }

    /// Rescans plugin directories on a background thread. Each bundle is
    /// loaded by a separate `otozaned --scan` process.
    pub fn start_scan(&mut self) {
        {
            let mut s = self.scan.lock().unwrap();
            if s.scanning {
                return;
            }
            s.scanning = true;
        }
        let scan = self.scan.clone();
        let dirs = otozane_plugins::search_dirs(&self.settings.plugin_dirs);
        let cache = self.store.dir().join("plugins.json");
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        let thread = std::thread::spawn(move || {
            let exe = std::env::current_exe().unwrap_or_else(|_| "otozaned".into());
            if let Some(list) =
                otozane_plugins::scan_dirs(&dirs, &exe, || cancelled.load(Ordering::Relaxed))
            {
                if let Ok(json) = serde_json::to_string_pretty(&list) {
                    let tmp = cache.with_extension("json.tmp");
                    if std::fs::write(&tmp, json).is_ok() {
                        let _ = std::fs::rename(&tmp, &cache);
                    }
                }
                *scan.lock().unwrap() = list;
            }
        });
        self.scan_thread = Some((cancel, thread));
    }

    fn effect_info(&self, e: &EffectConfig) -> EffectInfo {
        let mut info = EffectInfo {
            error: self.ctl.effect_error(e.id),
            ..Default::default()
        };
        if e.kind == EffectKind::Plugin {
            info.params = self.ctl.params(e.id).to_vec();
            info.values = self.ctl.values(e.id);
            if let Some(c) = self.plugins.get(&e.id) {
                info.texts = info
                    .values
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| c.value_text(i, v))
                    .collect();
            }
        }
        info
    }

    fn status(&self) -> protocol::Status {
        let st = self.backend.status.lock().unwrap().clone();
        let c = &self.backend.counters;
        let shared = &self.ctl.shared;
        let capture = match st.capture {
            CaptureState::Starting => protocol::CaptureState::Starting,
            CaptureState::WaitingForDevice(d) => protocol::CaptureState::WaitingForDevice(d),
            CaptureState::Connecting(d) => protocol::CaptureState::Connecting(d),
            CaptureState::Running(d) => protocol::CaptureState::Running(d),
            CaptureState::Muted => protocol::CaptureState::Muted,
            CaptureState::Error(e) => protocol::CaptureState::Error(e),
        };
        protocol::Status {
            version: env!("CARGO_PKG_VERSION").into(),
            capture,
            input_device: self.settings.input_device.clone(),
            devices: st
                .devices
                .into_iter()
                .map(|d| protocol::DeviceInfo {
                    name: d.name,
                    description: d.description,
                    is_virtual: d.is_virtual,
                })
                .collect(),
            virtual_source_ready: st.virtual_source_ready,
            processing_enabled: shared.processing_enabled.load(Ordering::Relaxed),
            stop_on_close: self.settings.stop_on_close,
            muted: self.muted,
            monitoring: self.monitoring,
            input_peak: shared.input_peak.swap(0.0),
            output_peak: shared.output_peak.swap(0.0),
            meters: self.chain.iter().map(|e| self.ctl.meter(e.id)).collect(),
            effects: self.chain.iter().map(|e| self.effect_info(e)).collect(),
            chain: self.chain.clone(),
            active_profile: self.settings.active_profile.clone(),
            diagnostics: Diagnostics {
                sample_rate: otozane_core::SAMPLE_RATE,
                channels: self.settings.channels,
                chain_latency_frames: shared.latency_frames.load(Ordering::Relaxed),
                capture_quantum: c.capture_quantum.load(Ordering::Relaxed),
                playback_quantum: c.playback_quantum.load(Ordering::Relaxed),
                ring_fill_frames: c.ring_fill.load(Ordering::Relaxed),
                underrun_frames: c.underrun_frames.load(Ordering::Relaxed),
                overrun_frames: c.overrun_frames.load(Ordering::Relaxed),
                skipped_frames: c.skipped_frames.load(Ordering::Relaxed),
                processed_blocks: shared.blocks.load(Ordering::Relaxed),
            },
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        if let Some((cancel, thread)) = self.scan_thread.take() {
            cancel.store(true, Ordering::Relaxed);
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_out_wait_covers_a_full_audio_block() {
        let block_ms = otozane_core::engine::MAX_BLOCK * 1000 / otozane_core::SAMPLE_RATE as usize;
        assert!(FADE_OUT_WAIT.as_millis() >= block_ms as u128);
    }

    #[test]
    fn free_profile_names_avoid_case_and_filesystem_collisions() {
        let dir = std::env::temp_dir().join(format!("otozane-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::new(dir.clone());
        store.save_profile("Stream", &Profile::default()).unwrap();
        assert_eq!(free_profile_name(&store, "stream"), "stream 2");
        assert_eq!(free_profile_name(&store, "Other"), "Other");
        #[cfg(target_os = "macos")]
        {
            store.save_profile("Ünicode", &Profile::default()).unwrap();
            assert_eq!(free_profile_name(&store, "ünicode"), "ünicode 2");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
