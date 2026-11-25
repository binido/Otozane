//! `otozaned`: runs the audio engine and audio backend, and serves the
//! IPC socket used by the desktop UI and other clients.

mod service;
mod store;

#[cfg(target_os = "macos")]
use otozane_coreaudio as backend;
#[cfg(target_os = "linux")]
use otozane_pipewire as backend;

use otozane_core::config::Profile;
use otozane_core::protocol::{self, Request, Response};
use otozane_core::{SAMPLE_RATE, engine};
use service::Service;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Requests larger than this are rejected (a full chain is a few KiB).
const MAX_LINE: u64 = 1 << 20;

enum State {
    Active(Box<Service>),
    #[cfg(target_os = "linux")]
    Holding,
    Exiting,
}

struct Session {
    state: Mutex<State>,
    changed: Condvar,
}

impl State {
    fn tick(&mut self) -> Option<Duration> {
        match self {
            Self::Active(svc) => {
                svc.tick();
                Some(Duration::from_millis(100))
            }
            #[cfg(target_os = "linux")]
            Self::Holding => Some(Duration::from_secs(1)),
            Self::Exiting => None,
        }
    }
}

fn main() {
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "--version" => println!("otozaned {}", env!("CARGO_PKG_VERSION")),
            // Plugin scan helper, started by the service once per bundle.
            "--scan" => {
                let path = std::env::args_os()
                    .nth(2)
                    .map(std::path::PathBuf::from)
                    .unwrap_or_default();
                match otozane_plugins::scan_bundle(&path) {
                    Ok(list) => println!("{}", serde_json::to_string(&list).unwrap_or_default()),
                    Err(e) => {
                        println!("error: {e}");
                        std::process::exit(1);
                    }
                }
            }
            _ => println!(
                "Usage: otozaned [--version | --scan BUNDLE]\n\nRuns the Otozane audio service."
            ),
        }
        return;
    }
    if let Err(e) = run() {
        eprintln!("otozaned: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let path = protocol::socket_path();
    // The connect check alone is racy: two starting processes can both remove
    // and bind the socket, leaving one unreachable. The lock is authoritative.
    let _instance = instance_lock(&path.with_extension("lock"))?;
    if UnixStream::connect(&path).is_ok() {
        return Err(format!("already running ({})", path.display()).into());
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;

    #[cfg(target_os = "linux")]
    let executable = std::env::current_exe()?;
    #[cfg(target_os = "linux")]
    backend::start_virtual_microphone();

    let session = Arc::new(Session {
        state: Mutex::new(State::Active(Box::new(start_processing()?))),
        changed: Condvar::new(),
    });
    eprintln!(
        "otozaned {} listening on {}",
        env!("CARGO_PKG_VERSION"),
        path.display()
    );

    let accept_session = session.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let session = accept_session.clone();
            std::thread::spawn(move || {
                if let Err(e) = serve(conn, &session) {
                    eprintln!("client error: {e}");
                }
            });
        }
    });

    #[cfg(target_os = "linux")]
    let mut checked = std::time::Instant::now();
    let mut state = session.state.lock().unwrap();
    while let Some(wait) = state.tick() {
        #[cfg(target_os = "linux")]
        if checked.elapsed() >= Duration::from_secs(1) {
            // The microphone follows this process when a package removes its binary.
            if !executable.exists() {
                *state = State::Exiting;
                break;
            }
            checked = std::time::Instant::now();
        }
        state = session.changed.wait_timeout(state, wait).unwrap().0;
    }
    let _ = std::fs::remove_file(&path);
    Ok(())
}

/// Holds an exclusive lock for the process lifetime. The OS releases it on
/// exit, so a crashed service never leaves a stale lock behind.
fn instance_lock(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "another instance is running",
        )),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

fn start_processing() -> Result<Service, Box<dyn std::error::Error>> {
    let store = store::Store::new(store::config_dir());
    let mut settings = store.load_settings();
    settings.channels = settings.channels.clamp(1, 2);
    if let Err(e) = store::validate_name(&settings.active_profile) {
        // A bad name makes every profile save fail; recover with a known one.
        eprintln!(
            "active profile '{}' is invalid ({e}); using Default",
            settings.active_profile
        );
        settings.active_profile = "Default".into();
    }
    let profile = match store.load_profile(&settings.active_profile) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "profile '{}' not found; using defaults",
                settings.active_profile
            );
            Profile::default()
        }
        Err(e) => {
            // Move the unreadable file aside so the default chain is not saved
            // over it; without a backup, continuing would destroy the original.
            store
                .backup_profile(&settings.active_profile)
                .map_err(|b| {
                    format!(
                        "profile '{}' is unreadable ({e}) and could not be moved aside: {b}",
                        settings.active_profile
                    )
                })?;
            eprintln!(
                "profile '{}' unreadable ({e}); moved aside; using defaults",
                settings.active_profile
            );
            Profile::default()
        }
    };

    let (ctl, eng) = engine::new(SAMPLE_RATE as f32, settings.channels as usize);
    ctl.shared
        .processing_enabled
        .store(settings.processing_enabled, Ordering::Relaxed);
    #[cfg(target_os = "linux")]
    let backend = backend::Backend::start(
        eng,
        settings.input_device.clone(),
        settings.channels as usize,
    );
    #[cfg(target_os = "macos")]
    let backend = backend::Backend::start(eng, settings.input_device.clone());
    let mut svc = Service::new(ctl, backend, store, settings);
    // Also makes sure the active profile exists on disk.
    svc.apply_chain(profile.chain)?;
    if !svc.store.dir().join("plugins.json").exists() {
        svc.start_scan();
    }
    Ok(svc)
}

impl Session {
    fn handle(&self, req: Request, writer: &mut UnixStream) -> std::io::Result<()> {
        let mut state = self.state.lock().unwrap();
        let response = match &mut *state {
            State::Active(svc) => {
                let shutdown = req == Request::Shutdown;
                let response = svc.handle(req);
                if shutdown {
                    #[cfg(target_os = "linux")]
                    let next = if svc.settings.stop_on_close {
                        State::Holding
                    } else {
                        State::Exiting
                    };
                    #[cfg(target_os = "macos")]
                    let next = State::Exiting;
                    *state = next;
                    // Send the reply before the main thread can exit.
                    let result = write_response(writer, &response);
                    self.changed.notify_one();
                    return result;
                }
                response
            }
            #[cfg(target_os = "linux")]
            State::Holding if req == Request::Start => match start_processing() {
                Ok(svc) => {
                    *state = State::Active(Box::new(svc));
                    self.changed.notify_one();
                    Response::Ok
                }
                Err(e) => Response::Error {
                    message: e.to_string(),
                },
            },
            #[cfg(target_os = "linux")]
            State::Holding => Response::Stopped,
            State::Exiting => Response::Stopped,
        };
        drop(state);
        write_response(writer, &response)
    }
}

fn serve(conn: UnixStream, session: &Session) -> std::io::Result<()> {
    let mut writer = conn.try_clone()?;
    let mut reader = BufReader::new(conn);
    loop {
        let mut line = String::new();
        let n = (&mut reader).take(MAX_LINE).read_line(&mut line)?;
        if n == 0 {
            return Ok(());
        }
        if !line.ends_with('\n') {
            return Err(std::io::Error::other("request too long"));
        }
        match serde_json::from_str::<Request>(&line) {
            Ok(req) => session.handle(req, &mut writer)?,
            Err(e) => write_response(
                &mut writer,
                &Response::Error {
                    message: format!("invalid request: {e}"),
                },
            )?,
        }
    }
}

fn write_response(writer: &mut UnixStream, response: &Response) -> std::io::Result<()> {
    let mut out = serde_json::to_string(response)?;
    out.push('\n');
    writer.write_all(out.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_instance_cannot_take_the_lock() {
        let dir = std::env::temp_dir().join(format!("otozane-lock-{}", std::process::id()));
        let path = dir.join("otozane.lock");
        let first = instance_lock(&path).unwrap();
        assert!(instance_lock(&path).is_err());
        drop(first);
        assert!(instance_lock(&path).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }
}
