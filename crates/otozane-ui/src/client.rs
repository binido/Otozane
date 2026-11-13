//! Background connection to `otozaned`. The UI thread never blocks on IPC:
//! it queues requests and reads the latest published state.

use otozane_core::config::Profile;
use otozane_core::protocol::{Client, PluginList, Request, Response, Status};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(33);

/// Master processing mode, shown as one three-way choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    On,
    /// Effects bypassed, microphone passes through unchanged.
    NoEffects,
    /// Capture stopped, the virtual microphone outputs silence.
    Muted,
}

impl Mode {
    pub const ALL: [(Mode, &str); 3] = [
        (Mode::On, "On"),
        (Mode::NoEffects, "No effects"),
        (Mode::Muted, "Muted"),
    ];

    pub fn of(st: &Status) -> Self {
        match (st.muted, st.processing_enabled) {
            (true, _) => Mode::Muted,
            (false, false) => Mode::NoEffects,
            (false, true) => Mode::On,
        }
    }

    /// Requests that switch the service from `from` to this mode. Muted
    /// keeps the effects setting as it was. `SetMuted` is only sent when
    /// muting or unmuting, so On and No effects also work with older services.
    pub fn requests(self, from: Mode) -> Vec<Request> {
        let mut reqs = Vec::new();
        if self != Mode::Muted {
            reqs.push(Request::SetProcessing {
                enabled: self == Mode::On,
            });
        }
        if (self == Mode::Muted) != (from == Mode::Muted) {
            reqs.push(Request::SetMuted {
                muted: self == Mode::Muted,
            });
        }
        reqs
    }
}

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
    /// Messages for the user, newest last (`true` = error). Drained by the UI.
    pub messages: Vec<(String, bool)>,
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
            messages: Vec::new(),
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

    /// Asks for a file and imports it as a new profile named after the file.
    pub fn import_profile(&self) {
        let (tx, state) = (self.tx.clone(), self.state.clone());
        std::thread::spawn(move || {
            let Some(path) = rfd::FileDialog::new()
                .set_title("Import profile")
                .add_filter("Otozane profile", &["toml"])
                .pick_file()
            else {
                return;
            };
            let result = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|s| toml::from_str::<Profile>(&s).map_err(|e| e.message().to_string()));
            match result {
                Ok(profile) => {
                    let name = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Imported".into());
                    let _ = tx.send(Request::ImportProfile { name, profile });
                }
                Err(e) => state
                    .lock()
                    .unwrap()
                    .messages
                    .push((format!("{} is not a profile: {e}", path.display()), true)),
            }
        });
    }
}

/// Asks where to save an exported profile and writes it as TOML.
fn export_profile(name: String, profile: Profile, state: Arc<Mutex<State>>) {
    std::thread::spawn(move || {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export profile")
            .set_file_name(format!("{name}.toml"))
            .add_filter("Otozane profile", &["toml"])
            .save_file()
        else {
            return;
        };
        let msg = toml::to_string_pretty(&profile)
            .map_err(|e| e.to_string())
            .and_then(|s| std::fs::write(&path, s).map_err(|e| e.to_string()));
        state.lock().unwrap().messages.push(match msg {
            Ok(()) => (format!("Exported to {}", path.display()), false),
            Err(e) => (format!("Export failed: {e}"), true),
        });
    });
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
            }
            Err(e) => state.lock().unwrap().connection = Connection::Disconnected(e.to_string()),
        }
        // Requests queued while disconnected are stale. Stops once the window
        // (and its sender) is gone.
        loop {
            match rx.try_recv() {
                Ok(_) => {}
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
        }
        ctx.request_repaint();
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn session(
    mut client: Client,
    rx: &Receiver<Request>,
    state: &Arc<Mutex<State>>,
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
            Response::Error { message } => s.messages.push((message, true)),
            Response::Profile { name, profile } => export_profile(name, profile, state.clone()),
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
                            | Request::ImportProfile { .. }
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

const UNIT: &str = include_str!("../../../packaging/otozane.service");
const UNIT_EXEC: &str = "%h/.local/bin/otozaned";

/// `otozaned` next to the current executable, if present.
fn local_service() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("otozaned")))
        .filter(|p| p.exists())
}

fn config_home() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config")))
}

fn unit_path() -> Option<std::path::PathBuf> {
    Some(config_home()?.join("systemd/user/otozane.service"))
}

const DESKTOP: &str = include_str!("../../../packaging/otozane.desktop");

fn login_entry_path() -> Option<std::path::PathBuf> {
    Some(config_home()?.join("autostart/otozane.desktop"))
}

/// The desktop entry with `Exec` starting `exe` hidden in the tray.
fn login_entry_text(exe: &std::path::Path) -> String {
    let mut quoted = String::from("\"");
    for c in exe.display().to_string().chars() {
        match c {
            // Escaped for the quoted argument, then again as a string value.
            '"' | '`' | '$' => quoted.extend(['\\', '\\', c]),
            '\\' => quoted.push_str(r"\\\\"),
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    DESKTOP.replace("Exec=otozane\n", &format!("Exec={quoted} --hidden\n"))
}

/// Whether the window starts hidden in the tray at login.
pub fn login_window_enabled() -> bool {
    login_entry_path().is_some_and(|p| p.exists())
}

/// Writes or removes the XDG autostart entry for this executable.
pub fn set_login_window(on: bool) -> std::io::Result<()> {
    let path = login_entry_path().ok_or_else(|| std::io::Error::other("HOME is not set"))?;
    if !on {
        return std::fs::remove_file(&path).or_else(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(e),
        });
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, login_entry_text(&std::env::current_exe()?))
}

/// The unit file with `ExecStart` pointing at `service`.
fn unit_text(service: Option<&std::path::Path>) -> String {
    match service {
        Some(p) => {
            let exec = format!("\"{}\"", p.display().to_string().replace('%', "%%"));
            UNIT.replace(UNIT_EXEC, &exec)
        }
        None => UNIT.to_string(),
    }
}

fn systemctl(args: &[&str]) -> std::io::Result<()> {
    let out = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

/// Whether the systemd user unit starts the service with the session.
pub fn autostart_enabled() -> bool {
    systemctl(&["is-enabled", "--quiet", "otozane.service"]).is_ok()
}

/// Installs and enables the user unit, or disables it. `start_now` also
/// starts it; pass false while another instance is running, since the unit
/// would fail on the single-instance lock.
pub fn set_autostart(on: bool, start_now: bool) -> std::io::Result<()> {
    if !on {
        return systemctl(&["disable", "otozane.service"]);
    }
    let path = unit_path().ok_or_else(|| std::io::Error::other("HOME is not set"))?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, unit_text(local_service().as_deref()))?;
    systemctl(&["daemon-reload"])?;
    let mut args = vec!["enable", "otozane.service"];
    if start_now {
        args.push("--now");
    }
    systemctl(&args)
}

/// Starts `otozaned`: through systemd when the user unit is installed, so
/// systemd owns the process, otherwise detached from this process, from next
/// to the current executable if present, else from `PATH`.
pub fn start_service() -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    if unit_path().is_some_and(|p| p.exists()) {
        return systemctl(&["start", "otozane.service"]);
    }
    let local = local_service();
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

#[cfg(test)]
mod tests {
    #[test]
    fn mode_sends_set_muted_only_on_mute_change() {
        use super::Mode::*;
        use otozane_core::protocol::Request::*;
        assert_eq!(NoEffects.requests(On), [SetProcessing { enabled: false }]);
        assert_eq!(Muted.requests(On), [SetMuted { muted: true }]);
        assert_eq!(
            On.requests(Muted),
            [SetProcessing { enabled: true }, SetMuted { muted: false }]
        );
    }

    #[test]
    fn unit_points_at_local_service() {
        let text = super::unit_text(Some(std::path::Path::new("/opt/my dir/50%/otozaned")));
        assert!(
            text.contains("ExecStart=\"/opt/my dir/50%%/otozaned\"\n"),
            "{text}"
        );
        assert!(!text.contains(super::UNIT_EXEC));
    }

    #[test]
    fn login_entry_starts_hidden() {
        let text = super::login_entry_text(std::path::Path::new("/opt/a \"b\"/50%/otozane"));
        assert!(
            text.contains("\nExec=\"/opt/a \\\\\"b\\\\\"/50%%/otozane\" --hidden\n"),
            "{text}"
        );
    }
}
