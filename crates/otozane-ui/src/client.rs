//! Background connection to `otozaned`. The UI thread never blocks on IPC:
//! it queues requests and reads the latest published state.

use otozane_core::protocol::{Client, PluginList, Request, Response, Status};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(33);

#[derive(Debug, Clone, PartialEq)]
pub enum Connection {
    Connecting,
    Connected,
    Disconnected(String),
}

pub struct State {
    pub connection: Connection,
    pub status: Option<Status>,
    pub profiles: Vec<String>,
    pub plugins: PluginList,
    /// Errors returned by the service, newest last. Drained by the UI.
    pub errors: Vec<String>,
}

pub struct ServiceClient {
    tx: Sender<Request>,
    pub state: Arc<Mutex<State>>,
}

impl ServiceClient {
    pub fn spawn(ctx: egui::Context) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let state = Arc::new(Mutex::new(State {
            connection: Connection::Connecting,
            status: None,
            profiles: Vec::new(),
            plugins: PluginList::default(),
            errors: Vec::new(),
        }));
        let st = state.clone();
        std::thread::Builder::new()
            .name("otozane-ipc".into())
            .spawn(move || run(rx, st, ctx))
            .expect("failed to spawn IPC thread");
        Self { tx, state }
    }

    pub fn send(&self, req: Request) {
        let _ = self.tx.send(req);
    }
}

fn run(rx: Receiver<Request>, state: Arc<Mutex<State>>, ctx: egui::Context) {
    loop {
        match Client::connect() {
            Ok(client) => {
                state.lock().unwrap().connection = Connection::Connected;
                let err = session(client, &rx, &state, &ctx);
                let mut s = state.lock().unwrap();
                s.connection = Connection::Disconnected(err.to_string());
                s.status = None;
                // Requests queued while disconnected are stale.
                while rx.try_recv().is_ok() {}
            }
            Err(e) => {
                state.lock().unwrap().connection = Connection::Disconnected(e.to_string());
                while rx.try_recv().is_ok() {}
            }
        }
        ctx.request_repaint();
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn session(
    mut client: Client,
    rx: &Receiver<Request>,
    state: &Mutex<State>,
    ctx: &egui::Context,
) -> std::io::Error {
    let mut last_profiles = Instant::now() - Duration::from_secs(10);
    let handle = |client: &mut Client, req: &Request| -> std::io::Result<()> {
        let resp = client.request(req)?;
        let mut s = state.lock().unwrap();
        match resp {
            Response::Status(st) => s.status = Some(*st),
            Response::Profiles { names } => s.profiles = names,
            Response::Plugins(list) => s.plugins = *list,
            Response::Error { message } => s.errors.push(message),
            Response::Ok => {}
        }
        Ok(())
    };
    loop {
        let deadline = Instant::now() + POLL;
        let mut profiles_changed = false;
        loop {
            let timeout = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(timeout) {
                Ok(req) => {
                    profiles_changed |= matches!(
                        req,
                        Request::LoadProfile { .. }
                            | Request::SaveProfileAs { .. }
                            | Request::DeleteProfile { .. }
                            | Request::RenameProfile { .. }
                    );
                    if let Err(e) = handle(&mut client, &req) {
                        return e;
                    }
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    return std::io::Error::other("UI closed");
                }
            }
        }
        if profiles_changed || last_profiles.elapsed() > Duration::from_secs(2) {
            for req in [Request::ListProfiles, Request::ListPlugins] {
                if let Err(e) = handle(&mut client, &req) {
                    return e;
                }
            }
            last_profiles = Instant::now();
        }
        if let Err(e) = handle(&mut client, &Request::Status) {
            return e;
        }
        ctx.request_repaint();
    }
}

/// Starts `otozaned` detached from this process: from next to the current
/// executable if present, otherwise from `PATH`.
pub fn start_service() -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let local = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("otozaned")))
        .filter(|p| p.exists());
    let mut cmd = std::process::Command::new(
        local
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("otozaned")),
    );
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // Own process group, so closing the UI's terminal does not stop audio.
        .process_group(0);
    cmd.spawn().map(|_| ())
}
