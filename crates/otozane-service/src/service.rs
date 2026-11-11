//! Request handling and persistence for the running service.

use crate::store::{Store, validate_name};
use otozane_core::config::{EffectConfig, Profile, Settings};
use otozane_core::effects::EffectKind;
use otozane_core::engine::{ControlError, Controller, External};
use otozane_core::protocol::{self, Diagnostics, EffectInfo, PluginList, Request, Response};
use otozane_pipewire::{Backend, CaptureState};
use otozane_plugins::PluginControl;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Service {
    pub ctl: Controller,
    pub backend: Backend,
    pub store: Store,
    pub settings: Settings,
    pub chain: Vec<EffectConfig>,
    /// Control halves of loaded plugins, by effect id.
    plugins: HashMap<u64, Box<dyn PluginControl>>,
    scan: Arc<Mutex<PluginList>>,
    dirty_settings: bool,
    dirty_profile: bool,
    pub quit: bool,
}

fn err(e: impl std::fmt::Display) -> Response {
    Response::Error {
        message: e.to_string(),
    }
}

impl Service {
    pub fn new(ctl: Controller, backend: Backend, store: Store, settings: Settings) -> Self {
        let scan = std::fs::read_to_string(store.dir().join("plugins.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            ctl,
            backend,
            store,
            settings,
            chain: Vec::new(),
            plugins: HashMap::new(),
            scan: Arc::new(Mutex::new(scan)),
            dirty_settings: false,
            dirty_profile: false,
            quit: false,
        }
    }

    /// Periodic housekeeping: frees retired chains and writes pending changes.
    pub fn tick(&mut self) {
        self.ctl.collect_garbage();
        self.flush();
    }

    pub fn flush(&mut self) {
        if self.dirty_profile {
            self.capture_plugin_states();
            let p = Profile {
                chain: self.chain.clone(),
            };
            match self.store.save_profile(&self.settings.active_profile, &p) {
                Ok(()) => self.dirty_profile = false,
                Err(e) => eprintln!("failed to save profile: {e}"),
            }
        }
        if self.dirty_settings {
            match self.store.save_settings(&self.settings) {
                Ok(()) => self.dirty_settings = false,
                Err(e) => eprintln!("failed to save settings: {e}"),
            }
        }
    }

    /// Copies each plugin's current state into the chain configuration.
    fn capture_plugin_states(&mut self) {
        for e in &mut self.chain {
            if let Some(c) = self.plugins.get(&e.id) {
                match c.save_state() {
                    Ok(b) => e.state = Some(otozane_plugins::encode_state(&b)),
                    Err(err) => eprintln!("{}: {err}", e.display_name()),
                }
            }
        }
    }

    pub fn apply_chain(&mut self, chain: Vec<EffectConfig>) -> Result<(), String> {
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
            std::thread::sleep(Duration::from_millis(15));
        }
        // The audio thread takes updates once per cycle (or per 100 ms pump
        // when idle), so a burst of edits may briefly find the queue full.
        let channels = self.settings.channels as usize;
        let plugins = &mut self.plugins;
        let mut factory = |cfg: &EffectConfig| -> Result<External, String> {
            let plugin = cfg.plugin.as_ref().ok_or("plugin entry without a plugin")?;
            let state = cfg.state.as_deref().and_then(otozane_plugins::decode_state);
            let inst = otozane_plugins::instantiate(
                plugin,
                state.as_deref(),
                otozane_core::SAMPLE_RATE as f32,
                channels,
                otozane_core::engine::MAX_BLOCK,
            )
            .map_err(|e| format!("{}: {e}", plugin.name))?;
            plugins.insert(cfg.id, inst.control);
            Ok(External {
                processor: inst.processor,
                params: inst.params,
                values: inst.values,
            })
        };
        let mut tries = 0;
        loop {
            match self.ctl.set_chain(&chain, &mut factory) {
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
        self.flush();
        self.apply_chain(p.chain)?;
        self.settings.active_profile = name.to_string();
        self.dirty_settings = true;
        // The chain came from the file; no need to write it back.
        self.dirty_profile = false;
        Ok(())
    }

    pub fn handle(&mut self, req: Request) -> Response {
        match req {
            Request::Status => Response::Status(Box::new(self.status())),
            Request::SetInput { device } => {
                if device.as_deref() == Some(otozane_pipewire::virtual_node_name()) {
                    return err("Otozane Microphone cannot be its own input");
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
                self.flush();
                self.settings.active_profile = name;
                self.dirty_settings = true;
                self.dirty_profile = true;
                self.flush();
                Response::Ok
            }
            Request::DeleteProfile { name } => {
                if name == self.settings.active_profile {
                    return err("cannot delete the active profile");
                }
                match self.store.delete_profile(&name) {
                    Ok(()) => Response::Ok,
                    Err(e) => err(e),
                }
            }
            Request::RenameProfile { from, to } => {
                self.flush();
                match self.store.rename_profile(&from, &to) {
                    Ok(()) => {
                        if from == self.settings.active_profile {
                            self.settings.active_profile = to;
                            self.dirty_settings = true;
                        }
                        Response::Ok
                    }
                    Err(e) => err(e),
                }
            }
            Request::ListPlugins => Response::Plugins(Box::new(self.scan.lock().unwrap().clone())),
            Request::ScanPlugins => {
                self.start_scan();
                Response::Ok
            }
            Request::Shutdown => {
                self.quit = true;
                Response::Ok
            }
        }
    }

    /// Rescans plugin directories on a background thread. Each bundle is
    /// loaded by a separate `otozaned --scan` process.
    pub fn start_scan(&self) {
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
        std::thread::spawn(move || {
            let exe = std::env::current_exe().unwrap_or_else(|_| "otozaned".into());
            let list = otozane_plugins::scan_dirs(&dirs, &exe);
            if let Ok(json) = serde_json::to_string_pretty(&list) {
                let _ = std::fs::write(&cache, json);
            }
            *scan.lock().unwrap() = list;
        });
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
