//! `otozaned`: runs the audio engine and PipeWire backend, and serves the
//! IPC socket used by the desktop UI and other clients.

mod service;
mod store;

use otozane_core::config::Profile;
use otozane_core::protocol::{self, Request, Response};
use otozane_core::{SAMPLE_RATE, engine};
use service::Service;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Requests larger than this are rejected (a full chain is a few KiB).
const MAX_LINE: u64 = 1 << 20;

fn main() {
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "--version" => println!("otozaned {}", env!("CARGO_PKG_VERSION")),
            // Plugin scan helper, started by the service once per bundle.
            "--scan" => {
                let path = std::env::args().nth(2).unwrap_or_default();
                match otozane_plugins::scan_bundle(std::path::Path::new(&path)) {
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
    if UnixStream::connect(&path).is_ok() {
        return Err(format!("already running ({})", path.display()).into());
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;

    let store = store::Store::new(store::config_dir());
    let mut settings = store.load_settings();
    settings.channels = settings.channels.clamp(1, 2);
    let profile = store
        .load_profile(&settings.active_profile)
        .unwrap_or_else(|e| {
            eprintln!(
                "profile '{}' not loaded ({e}); using defaults",
                settings.active_profile
            );
            Profile::default()
        });

    let (ctl, eng) = engine::new(SAMPLE_RATE as f32, settings.channels as usize);
    ctl.shared
        .processing_enabled
        .store(settings.processing_enabled, Ordering::Relaxed);
    let backend = otozane_pipewire::Backend::start(eng, settings.input_device.clone());
    let mut svc = Service::new(ctl, backend, store, settings);
    // Also makes sure the active profile exists on disk.
    svc.apply_chain(profile.chain)?;
    if !svc.store.dir().join("plugins.json").exists() {
        svc.start_scan();
    }
    let svc = Arc::new(Mutex::new(svc));
    eprintln!(
        "otozaned {} listening on {}",
        env!("CARGO_PKG_VERSION"),
        path.display()
    );

    let accept_svc = svc.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let svc = accept_svc.clone();
            std::thread::spawn(move || {
                if let Err(e) = serve(conn, &svc) {
                    eprintln!("client error: {e}");
                }
            });
        }
    });

    loop {
        std::thread::sleep(Duration::from_millis(100));
        let mut s = svc.lock().unwrap();
        s.tick();
        if s.quit {
            break;
        }
    }
    let _ = std::fs::remove_file(&path);
    // Dropping the service drops the backend, which destroys our PipeWire objects.
    let svc = Arc::try_unwrap(svc).ok();
    drop(svc);
    Ok(())
}

fn serve(conn: UnixStream, svc: &Mutex<Service>) -> std::io::Result<()> {
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
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => svc.lock().unwrap().handle(req),
            Err(e) => Response::Error {
                message: format!("invalid request: {e}"),
            },
        };
        let mut out = serde_json::to_string(&resp)?;
        out.push('\n');
        writer.write_all(out.as_bytes())?;
    }
}
