//! Background connection to `otozaned`. The UI thread never blocks on IPC:
//! it queues requests and reads the latest published state.

use otozane_core::config::{Profile, Settings, config_dir};
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
    pub stop_on_close: bool,
    pub setting_stop_on_close: Option<bool>,
    pub stop_on_close_unconfirmed: bool,
    pub login_start: Option<(bool, bool)>,
    /// Messages for the user, newest last (`true` = error). Drained by the UI.
    pub messages: Vec<(String, bool)>,
}

pub struct ServiceClient {
    tx: Sender<Command>,
    pub state: Arc<Mutex<State>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

enum Command {
    Request(Request),
    SetStopOnClose(bool),
    Close,
}

impl ServiceClient {
    pub fn spawn(ctx: egui::Context, stop_on_close: bool) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let state = Arc::new(Mutex::new(State {
            connection: Connection::Connecting,
            status: None,
            profiles: Vec::new(),
            plugins: PluginList::default(),
            stop_on_close,
            setting_stop_on_close: None,
            stop_on_close_unconfirmed: false,
            login_start: None,
            messages: Vec::new(),
        }));
        let st = state.clone();
        let start_on_open = stop_on_close && std::env::var_os("OTOZANE_SCREENSHOT").is_none();
        let thread = std::thread::Builder::new()
            .name("otozane-ipc".into())
            .spawn(move || run(rx, st, ctx, start_on_open))
            .expect("failed to spawn IPC thread");
        Self {
            tx,
            state,
            thread: Some(thread),
        }
    }

    pub fn send(&self, req: Request) {
        let _ = self.tx.send(Command::Request(req));
    }

    pub fn set_stop_on_close(&self, enabled: bool) {
        self.state.lock().unwrap().setting_stop_on_close = Some(enabled);
        let _ = self.tx.send(Command::SetStopOnClose(enabled));
    }

    pub fn close(&mut self) {
        let state = self.state.lock().unwrap();
        if !state.stop_on_close && state.setting_stop_on_close.is_none() {
            return;
        }
        drop(state);
        let _ = self.tx.send(Command::Close);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
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
                    let _ = tx.send(Command::Request(Request::ImportProfile { name, profile }));
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

fn run(rx: Receiver<Command>, state: Arc<Mutex<State>>, ctx: egui::Context, start_on_open: bool) {
    let startup = start_on_open.then(|| {
        if let Err(e) = start_window_service() {
            state
                .lock()
                .unwrap()
                .messages
                .push((format!("Could not start audio service: {e}"), true));
        }
        Instant::now()
    });
    loop {
        match Client::connect() {
            Ok(client) => {
                state.lock().unwrap().connection = Connection::Connected;
                let Err(err) = session(client, &rx, &state, &ctx) else {
                    return;
                };
                let mut s = state.lock().unwrap();
                s.connection = Connection::Disconnected(err.to_string());
                s.status = None;
            }
            Err(e) => state.lock().unwrap().connection = Connection::Disconnected(e.to_string()),
        }
        // Ordinary edits queued while disconnected are stale.
        ctx.request_repaint();
        loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(Command::Close) => {
                    let remaining = startup.map_or(Duration::ZERO, |start| {
                        Duration::from_secs(4).saturating_sub(start.elapsed())
                    });
                    close_service(remaining);
                    return;
                }
                Ok(Command::SetStopOnClose(enabled)) => {
                    apply_stop_on_close(&state, enabled);
                    break;
                }
                Ok(Command::Request(_)) => {}
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

fn session(
    mut client: Client,
    rx: &Receiver<Command>,
    state: &Arc<Mutex<State>>,
    ctx: &egui::Context,
) -> std::io::Result<()> {
    let mut last_profiles = Instant::now() - Duration::from_secs(10);
    let handle = |client: &mut Client, req: &Request| -> std::io::Result<()> {
        let resp = client.request(req)?;
        let mut s = state.lock().unwrap();
        match resp {
            Response::Status(st) => {
                s.stop_on_close = st.stop_on_close;
                if st.stop_on_close {
                    s.stop_on_close_unconfirmed = false;
                }
                s.status = Some(*st);
            }
            Response::Profiles { names } => s.profiles = names,
            Response::Plugins(list) => s.plugins = *list,
            Response::Error { message } => s.messages.push((message, true)),
            Response::Profile { name, profile } => export_profile(name, profile, state.clone()),
            Response::Ok => {}
            Response::Stopped => return Err(std::io::Error::other("Audio service stopped")),
        }
        Ok(())
    };
    loop {
        let deadline = Instant::now() + POLL;
        let mut profiles_changed = false;
        loop {
            let timeout = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(timeout) {
                Ok(Command::Request(req)) => {
                    profiles_changed |= matches!(
                        req,
                        Request::LoadProfile { .. }
                            | Request::SaveProfileAs { .. }
                            | Request::DeleteProfile { .. }
                            | Request::RenameProfile { .. }
                            | Request::ImportProfile { .. }
                    );
                    handle(&mut client, &req)?;
                }
                Ok(Command::SetStopOnClose(enabled)) => apply_stop_on_close(state, enabled),
                Ok(Command::Close) => {
                    close_service(Duration::ZERO);
                    return Ok(());
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    return Ok(());
                }
            }
        }
        if profiles_changed || last_profiles.elapsed() > Duration::from_secs(2) {
            for req in [Request::ListProfiles, Request::ListPlugins] {
                handle(&mut client, &req)?;
            }
            last_profiles = Instant::now();
        }
        handle(&mut client, &Request::Status)?;
        ctx.request_repaint();
    }
}

fn connect_within(timeout: Duration) -> std::io::Result<Client> {
    let start = Instant::now();
    loop {
        match Client::connect() {
            Err(_) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(50)),
            result => return result,
        }
    }
}

fn apply_stop_on_close(state: &Mutex<State>, enabled: bool) {
    let mut unconfirmed = false;
    let result = (|| {
        start_window_service()?;
        let mut client = connect_within(Duration::from_secs(4))?;
        let login = enabled.then(LoginStart::read).transpose()?;
        if let Some(login) = &login {
            login.disable().map_err(|e| login.restore_after(e))?;
        }
        match client.request(&Request::SetStopOnClose { enabled }) {
            Ok(Response::Ok) => Ok(()),
            Ok(response) => {
                let error = match response {
                    Response::Error { message } => std::io::Error::other(message),
                    _ => std::io::Error::other("Audio service stopped"),
                };
                Err(match &login {
                    Some(login) => login.restore_after(error),
                    None => error,
                })
            }
            // An unanswered request may still save the setting.
            Err(_) if enabled && stop_on_close_enabled() => Ok(()),
            Err(e) if enabled => {
                unconfirmed = true;
                Err(std::io::Error::other(format!(
                    "Save unconfirmed. Login startup stays off pending confirmation. {e}"
                )))
            }
            Err(e) => Err(e),
        }
    })();
    let login_start = (autostart_enabled(), login_window_enabled());
    let mut state = state.lock().unwrap();
    state.setting_stop_on_close = None;
    state.login_start = Some(login_start);
    state.stop_on_close_unconfirmed |= unconfirmed;
    match result {
        Ok(()) => {
            state.stop_on_close = enabled;
            if enabled {
                state.stop_on_close_unconfirmed = false;
            }
        }
        Err(e) => state
            .messages
            .push((format!("Could not change stop on close: {e}"), true)),
    }
}

fn close_service(timeout: Duration) {
    if !stop_on_close_enabled() {
        return;
    }
    if let Ok(mut client) = connect_within(timeout) {
        let _ = client.request(&Request::Shutdown);
        // The next window must see a stopped service when it opens.
        #[cfg(target_os = "macos")]
        let _ = wait_for_service_exit(
            &otozane_core::protocol::socket_path(),
            Duration::from_secs(2),
        );
    }
}

fn update_connection(path: &std::path::Path) -> std::io::Result<Option<Client>> {
    match Client::connect_to(path) {
        Ok(client) => Ok(Some(client)),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

pub fn service_is_running() -> std::io::Result<bool> {
    let Some(mut client) = update_connection(&otozane_core::protocol::socket_path())? else {
        return Ok(false);
    };
    match client.request(&Request::Status)? {
        Response::Status(_) => Ok(true),
        Response::Stopped => Ok(false),
        response => Err(std::io::Error::other(format!(
            "Unexpected service status: {response:?}"
        ))),
    }
}

pub fn stop_for_update(
    stop_supervisor: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    stop_for_update_at(&otozane_core::protocol::socket_path(), stop_supervisor)
}

fn stop_for_update_at(
    path: &std::path::Path,
    stop_supervisor: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    let Some(mut client) = update_connection(path)? else {
        stop_supervisor()?;
        return wait_for_service_exit(path, Duration::from_secs(2));
    };
    #[cfg(target_os = "linux")]
    let peer = service_pidfd(&client);
    let response = client.request(&Request::Shutdown)?;
    if !matches!(response, Response::Ok | Response::Stopped) {
        return Err(std::io::Error::other(format!(
            "Service refused shutdown: {response:?}"
        )));
    }
    stop_supervisor()?;
    let exited = wait_for_service_exit(path, Duration::from_secs(2));
    #[cfg(target_os = "linux")]
    if exited
        .as_ref()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::TimedOut)
        && matches!(client.request(&Request::Status), Ok(Response::Stopped))
    {
        // Stop-on-close can leave the daemon holding the socket after saving.
        terminate_held_service(&peer?)?;
        return wait_for_service_exit(path, Duration::from_secs(2));
    }
    exited
}

pub fn wait_for_service_start() -> std::io::Result<()> {
    let mut client = connect_within(Duration::from_secs(4))?;
    match client.request(&Request::Status)? {
        Response::Status(_) => Ok(()),
        response => Err(std::io::Error::other(format!(
            "The audio service did not start: {response:?}"
        ))),
    }
}

fn wait_for_service_exit(path: &std::path::Path, timeout: Duration) -> std::io::Result<()> {
    let start = Instant::now();
    while update_connection(path)?.is_some() {
        if start.elapsed() >= timeout {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "The audio service did not exit in time",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn service_pidfd(client: &Client) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{AsFd, AsRawFd, FromRawFd};

    let mut peer = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut size = std::mem::size_of_val(&peer) as libc::socklen_t;
    // SAFETY: The socket is live and peer and size are writable with the declared length.
    let result = unsafe {
        libc::getsockopt(
            client.as_fd().as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::from_mut(&mut peer).cast(),
            &mut size,
        )
    };
    if result == -1 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if size as usize != std::mem::size_of_val(&peer) || peer.pid <= 0 || peer.uid != uid {
        return Err(std::io::Error::other(
            "The audio service is owned by another user",
        ));
    }
    // SAFETY: pidfd_open takes a positive PID and zero flags.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, peer.pid, 0u32) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: A successful pidfd_open returns a new owned descriptor.
    Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) })
}

#[cfg(target_os = "linux")]
fn terminate_held_service(peer: &std::os::fd::OwnedFd) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    // SAFETY: The PID descriptor identifies the peer and null requests default signal info.
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            peer.as_raw_fd(),
            libc::SIGTERM,
            std::ptr::null::<libc::siginfo_t>(),
            0u32,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

pub fn restore_after_update_error(was_running: bool, error: String) -> String {
    if was_running
        && !matches!(service_is_running(), Ok(true))
        && let Err(restore) = start_service().and_then(|()| wait_for_service_start())
    {
        return format!(
            "{error} Could not restore the audio service: {restore}. Use Start service after resolving the error."
        );
    }
    error
}

pub fn stop_on_close_enabled() -> bool {
    std::fs::read_to_string(config_dir().join("config.toml"))
        .ok()
        .and_then(|s| toml::from_str::<Settings>(&s).ok())
        .is_some_and(|settings| settings.stop_on_close)
}

#[cfg(target_os = "linux")]
const UNIT: &str = include_str!("../../../packaging/otozane.service");
#[cfg(target_os = "linux")]
const UNIT_EXEC: &str = "%h/.local/bin/otozaned";

/// `otozaned` next to the current executable, if present.
pub(crate) fn local_service() -> Option<std::path::PathBuf> {
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

struct LoginFile {
    path: std::path::PathBuf,
    contents: Vec<u8>,
    permissions: std::fs::Permissions,
}

impl LoginFile {
    fn read(path: Option<std::path::PathBuf>) -> std::io::Result<Option<Self>> {
        let Some(path) = path else {
            return Ok(None);
        };
        match std::fs::read(&path) {
            Ok(contents) => Ok(Some(Self {
                permissions: std::fs::metadata(&path)?.permissions(),
                path,
                contents,
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn disable(entry: Option<&Self>) -> std::io::Result<()> {
        if let Some(entry) = entry {
            std::fs::remove_file(&entry.path)?;
        }
        Ok(())
    }

    fn restore(entry: Option<&Self>) -> std::io::Result<()> {
        use std::io::Write;

        if let Some(entry) = entry {
            match std::fs::File::create_new(&entry.path) {
                Ok(mut file) => {
                    file.write_all(&entry.contents)?;
                    file.set_permissions(entry.permissions.clone())?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

struct LoginStart {
    window: Option<LoginFile>,
    #[cfg(target_os = "macos")]
    agent: Option<LoginFile>,
    #[cfg(target_os = "linux")]
    unit_enabled: bool,
}

impl LoginStart {
    fn read() -> std::io::Result<Self> {
        Ok(Self {
            window: LoginFile::read(login_entry_path())?,
            #[cfg(target_os = "macos")]
            agent: LoginFile::read(crate::launchd::plist_path().ok())?,
            #[cfg(target_os = "linux")]
            unit_enabled: autostart_enabled(),
        })
    }

    fn disable(&self) -> std::io::Result<()> {
        LoginFile::disable(self.window.as_ref())?;
        // Removing the plist lets the loaded launch agent finish normally.
        #[cfg(target_os = "macos")]
        LoginFile::disable(self.agent.as_ref())?;
        #[cfg(target_os = "linux")]
        if self.unit_enabled {
            set_autostart(false, false)?;
        }
        Ok(())
    }

    fn restore(&self) -> std::io::Result<()> {
        let window = LoginFile::restore(self.window.as_ref());
        #[cfg(target_os = "macos")]
        let service = LoginFile::restore(self.agent.as_ref());
        #[cfg(target_os = "linux")]
        let service = if self.unit_enabled && !autostart_enabled() {
            // The existing unit may contain user edits and should keep its contents.
            systemctl(&["enable", "otozane.service"])
        } else {
            Ok(())
        };
        window.and(service)
    }

    fn restore_after(&self, error: std::io::Error) -> std::io::Error {
        match self.restore() {
            Ok(()) => error,
            Err(restore) => {
                std::io::Error::other(format!("{error}. Could not restore login start: {restore}"))
            }
        }
    }
}

#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
fn unit_text(service: Option<&std::path::Path>) -> String {
    match service {
        Some(p) => {
            let exec = format!("\"{}\"", p.display().to_string().replace('%', "%%"));
            UNIT.replace(UNIT_EXEC, &exec)
        }
        None => UNIT.to_string(),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn systemctl(args: &[&str]) -> std::io::Result<()> {
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
#[cfg(target_os = "linux")]
pub fn autostart_enabled() -> bool {
    systemctl(&["is-enabled", "--quiet", "otozane.service"]).is_ok()
}

/// Installs and enables the user unit, or disables it. `start_now` also
/// starts it; pass false while another instance is running, since the unit
/// would fail on the single-instance lock.
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
pub fn start_service() -> std::io::Result<()> {
    if resume_service()? {
        return Ok(());
    }
    if unit_path().is_some_and(|p| p.exists()) {
        return systemctl(&["start", "otozane.service"]);
    }
    start_detached()
}

#[cfg(target_os = "linux")]
fn resume_service() -> std::io::Result<bool> {
    let Ok(mut client) = Client::connect() else {
        return Ok(false);
    };
    match client.request(&Request::Start)? {
        Response::Error { message } => Err(std::io::Error::other(message)),
        _ => Ok(true),
    }
}

pub fn start_window_service() -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    if resume_service()? {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    if Client::connect().is_ok() {
        return Ok(());
    }
    start_detached()
}

pub(crate) fn start_detached() -> std::io::Result<()> {
    start_detached_from(
        local_service()
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("otozaned")),
    )
}

pub(crate) fn start_detached_from(service: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new(service);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // Own process group, so closing the UI's terminal does not stop audio.
        .process_group(0);
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(target_os = "macos")]
pub use crate::launchd::{autostart_enabled, set_autostart, start_service};

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn update_service_fixture() {
        use std::io::{BufRead, Write};
        use std::os::unix::net::UnixListener;

        let Some(path) = std::env::var_os("OTOZANE_TEST_UPDATE_SOCKET") else {
            return;
        };
        let holds = std::env::var_os("OTOZANE_TEST_UPDATE_HOLD").is_some();
        let listener = UnixListener::bind(path).unwrap();
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            std::thread::spawn(move || {
                let reader = std::io::BufReader::new(stream.try_clone().unwrap());
                for line in reader.lines().map_while(Result::ok) {
                    let request: super::Request = serde_json::from_str(&line).unwrap();
                    let response = if holds && request == super::Request::Status {
                        super::Response::Stopped
                    } else {
                        super::Response::Ok
                    };
                    writeln!(stream, "{}", serde_json::to_string(&response).unwrap()).unwrap();
                }
            });
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn update_stop_exits_a_held_daemon_but_rejects_a_shutdown_timeout() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("service.sock");
        assert!(super::stop_for_update_at(&socket, || Ok(())).is_ok());
        for holds in [true, false] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", "client::tests::update_service_fixture"])
                .env("OTOZANE_TEST_UPDATE_SOCKET", &socket)
                .stdout(std::process::Stdio::null());
            if holds {
                command.env("OTOZANE_TEST_UPDATE_HOLD", "1");
            }
            let mut child = command.spawn().unwrap();
            for _ in 0..200 {
                if socket.exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let bound = socket.exists();
            let result = super::stop_for_update_at(&socket, || Ok(()));
            if holds && result.is_ok() {
                child.wait().unwrap();
            }
            let stopped = child.try_wait().unwrap().is_some();
            if !stopped {
                let _ = child.kill();
                child.wait().unwrap();
            }
            assert!(bound, "the child never bound the fixture socket");
            std::fs::remove_file(&socket).unwrap();
            if holds {
                result.unwrap();
                assert!(stopped);
            } else {
                assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
                assert!(!stopped);
            }
        }
    }

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

    #[cfg(target_os = "linux")]
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
