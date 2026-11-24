use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Version of this build, e.g. `0.5.0`.
pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Where verified downloads are staged, next to settings and profiles.
pub fn staging_dir() -> std::path::PathBuf {
    otozane_core::config::config_dir().join("updates")
}

/// Seconds since the Unix epoch, 0 before 1970.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// How often automatic checks run. Stored as a short string in `ui.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Interval {
    #[serde(rename = "6h")]
    SixHours,
    #[serde(rename = "12h")]
    TwelveHours,
    #[default]
    #[serde(rename = "1d")]
    OneDay,
    #[serde(rename = "1w")]
    OneWeek,
}

impl Interval {
    pub const ALL: [Interval; 4] = [
        Interval::SixHours,
        Interval::TwelveHours,
        Interval::OneDay,
        Interval::OneWeek,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Interval::SixHours => "6 hours",
            Interval::TwelveHours => "12 hours",
            Interval::OneDay => "1 day",
            Interval::OneWeek => "1 week",
        }
    }

    pub fn duration(self) -> Duration {
        Duration::from_secs(match self {
            Interval::SixHours => 6 * 3600,
            Interval::TwelveHours => 12 * 3600,
            Interval::OneDay => 24 * 3600,
            Interval::OneWeek => 7 * 24 * 3600,
        })
    }
}

/// Automatic check preferences, stored under `[updates]` in `ui.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Check while Otozane runs. Off until the user turns it on.
    pub check_automatically: bool,
    pub interval: Interval,
    /// Unix seconds of the last attempted check.
    pub last_check: Option<i64>,
}

impl Prefs {
    /// Whether an automatic check is due. A clock that jumped backwards does
    /// not force a check.
    pub fn due(&self, now: i64, last_check: Option<i64>) -> bool {
        let interval = self.interval.duration().as_secs() as i64;
        self.check_automatically
            && last_check.is_none_or(|last| now.saturating_sub(last) >= interval)
    }
}

/// What the backend can do with a newer release.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// The backend downloads, verifies and installs it in place.
    Update,
    /// Not supported from inside the app: open the release page instead.
    /// `hint` tells the user how this install is updated.
    ReleasePage { hint: String },
}

/// A release newer than this build.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub id: u64,
    /// Version without the leading `v`, e.g. `0.6.0`.
    pub version: String,
    /// Release notes, generated from `CHANGELOG.md`.
    pub notes: String,
    /// GitHub release page.
    pub page_url: String,
    pub action: Action,
}

/// Update operations run by the worker.
pub trait Backend: Send + 'static {
    fn current_version(&self) -> String {
        app_version().to_string()
    }

    /// The newest release, or `None` when this build is current.
    fn check(&mut self) -> Result<Option<Release>, String>;

    /// Downloads and verifies the release without touching the running app.
    fn stage(&mut self, release: &Release) -> Result<(), String>;

    /// Installs the staged update. Called only after the user confirms the
    /// restart. The app keeps running: [`Backend::restart`] does the handoff.
    fn apply(&mut self) -> Result<(), String>;

    fn discard(&mut self);

    /// Starts the replacement or arranges for it to start after this process exits.
    fn restart(&mut self) -> Result<(), String>;
}

/// Where the update process stands.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    UpToDate,
    /// A newer release was found. Nothing is downloaded yet.
    Available,
    /// Downloading and verifying, the app keeps running.
    Staging,
    /// Staged and verified, waiting for the user to confirm the restart.
    Ready,
    Applying,
    Restarting,
    RestartFailed(String),
    /// An actionable message for the Updates section.
    Failed(String),
}

/// Latest update state, published by the update thread.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub current_version: String,
    pub phase: Phase,
    /// Newer release known from the last successful check.
    pub release: Option<Release>,
    /// Unix seconds when the last check started, mirrored into `ui.toml`.
    pub last_check: Option<i64>,
}

impl State {
    /// A newer version is known, so menus and the window can point at it.
    pub fn has_update(&self) -> bool {
        self.release.is_some()
    }

    /// Menu bar / tray entry for the update state.
    pub fn menu_label(&self) -> String {
        match &self.release {
            Some(release) => format!("Update to {}", release.version),
            None if self.phase == Phase::Checking => "Checking for updates...".to_string(),
            None => "Check for updates".to_string(),
        }
    }
}

enum Command {
    Check,
    SetSchedule(bool, Interval),
    Stage { id: u64, version: String },
    Discard,
    Apply,
    RetryRestart,
    Exit(Sender<()>),
}

/// Handle to the update thread.
#[derive(Clone)]
pub struct Updater {
    tx: Sender<Command>,
    state: Arc<Mutex<State>>,
}

impl Updater {
    pub fn spawn(
        backend: Box<dyn Backend>,
        prefs: Prefs,
        persist_check: impl Fn(i64) + Send + 'static,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let state = Arc::new(Mutex::new(State {
            current_version: backend.current_version(),
            last_check: prefs.last_check,
            ..Default::default()
        }));
        let st = state.clone();
        std::thread::Builder::new()
            .name("otozane-updates".into())
            .spawn(move || run(backend, prefs, rx, st, persist_check))
            .expect("failed to spawn update thread");
        Self { tx, state }
    }

    pub fn snapshot(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    /// Checks now, unless a check or install is running.
    pub fn check(&self) {
        let _ = self.tx.send(Command::Check);
    }

    pub fn stage(&self, release: &Release) {
        let _ = self.tx.send(Command::Stage {
            id: release.id,
            version: release.version.clone(),
        });
    }

    pub fn discard(&self) {
        let _ = self.tx.send(Command::Discard);
    }

    pub fn apply(&self) {
        let mut state = self.state.lock().unwrap();
        if state.phase == Phase::Ready && self.tx.send(Command::Apply).is_ok() {
            state.phase = Phase::Applying;
        }
    }

    pub fn retry_restart(&self) {
        let mut state = self.state.lock().unwrap();
        if matches!(state.phase, Phase::RestartFailed(_))
            && self.tx.send(Command::RetryRestart).is_ok()
        {
            state.phase = Phase::Applying;
        }
    }

    pub fn set_schedule(&self, enabled: bool, interval: Interval) {
        let _ = self.tx.send(Command::SetSchedule(enabled, interval));
    }

    pub fn finish(&self) {
        let (done, finished) = std::sync::mpsc::channel();
        if self.tx.send(Command::Exit(done)).is_ok() {
            let _ = finished.recv_timeout(Duration::from_millis(250));
        }
    }
}

const IDLE_POLL: Duration = Duration::from_secs(60);

fn run(
    mut backend: Box<dyn Backend>,
    mut prefs: Prefs,
    rx: Receiver<Command>,
    state: Arc<Mutex<State>>,
    persist_check: impl Fn(i64),
) {
    let auto_check = |backend: &mut dyn Backend, prefs: &Prefs| {
        let last_check = state.lock().unwrap().last_check;
        if !busy(&state) && prefs.due(now(), last_check) {
            check(backend, &state, &persist_check);
        }
    };
    auto_check(&mut *backend, &prefs);
    loop {
        match rx.recv_timeout(IDLE_POLL) {
            Ok(Command::Check) if !busy(&state) => {
                check(&mut *backend, &state, &persist_check);
            }
            Ok(Command::SetSchedule(enabled, interval)) => {
                prefs.check_automatically = enabled;
                prefs.interval = interval;
            }
            Ok(Command::Stage { id, version }) if !busy(&state) => {
                let release = state.lock().unwrap().release.clone();
                if let Some(release) = release
                    && release.id == id
                    && release.version == version
                    && release.action == Action::Update
                {
                    stage(&mut *backend, &state, &release);
                }
            }
            Ok(Command::Discard) => {
                let mut state = state.lock().unwrap();
                if state.phase == Phase::Ready {
                    backend.discard();
                    state.phase = Phase::Available;
                }
            }
            Ok(Command::Apply) => {
                if let Err(e) = backend.apply() {
                    publish(&state, Phase::Failed(e));
                } else {
                    restart(&mut *backend, &state);
                }
            }
            Ok(Command::RetryRestart) => restart(&mut *backend, &state),
            Ok(Command::Exit(done)) => {
                drop(backend);
                let _ = done.send(());
                return;
            }
            Err(RecvTimeoutError::Disconnected) => return,
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
        }
        auto_check(&mut *backend, &prefs);
    }
}

impl Phase {
    pub fn blocks_check(&self) -> bool {
        matches!(
            self,
            Self::Checking
                | Self::Staging
                | Self::Ready
                | Self::Applying
                | Self::Restarting
                | Self::RestartFailed(_)
        )
    }

    pub fn is_handoff(&self) -> bool {
        matches!(self, Self::Applying | Self::Restarting)
    }
}

fn busy(state: &Mutex<State>) -> bool {
    state.lock().unwrap().phase.blocks_check()
}

fn publish(state: &Mutex<State>, phase: Phase) {
    state.lock().unwrap().phase = phase;
}

fn check(backend: &mut dyn Backend, state: &Mutex<State>, persist_check: &impl Fn(i64)) {
    let checked_at = now();
    {
        let mut state = state.lock().unwrap();
        state.last_check = Some(checked_at);
        state.phase = Phase::Checking;
    }
    persist_check(checked_at);
    let result = backend.check();
    let mut s = state.lock().unwrap();
    match result {
        Ok(Some(release)) => {
            s.release = Some(release);
            s.phase = Phase::Available;
        }
        Ok(None) => {
            s.release = None;
            s.phase = Phase::UpToDate;
        }
        Err(e) => {
            s.release = None;
            s.phase = Phase::Failed(e);
        }
    }
}

fn stage(backend: &mut dyn Backend, state: &Mutex<State>, release: &Release) {
    publish(state, Phase::Staging);
    publish(
        state,
        match backend.stage(release) {
            Ok(()) => Phase::Ready,
            Err(e) => Phase::Failed(e),
        },
    );
}

fn restart(backend: &mut dyn Backend, state: &Mutex<State>) {
    publish(
        state,
        match backend.restart() {
            Ok(()) => Phase::Restarting,
            Err(e) => Phase::RestartFailed(e),
        },
    );
}

pub(crate) fn installation_lock() -> std::io::Result<std::fs::File> {
    lock_installation_in(&staging_dir())
}

fn lock_installation_in(staging: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::fs::{File, TryLockError};

    std::fs::create_dir_all(staging)?;
    let lock = File::create(staging.join("install.lock"))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(TryLockError::WouldBlock) => Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "Otozane is updating or changing its startup settings. Try again when it finishes.",
        )),
        Err(e) => Err(e.into()),
    }
}

/// Opens the release page in the system browser. Only web addresses from the
/// backend are opened.
pub fn open_page(url: &str) -> std::io::Result<()> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("not a web address: {url}"),
        ));
    }
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = std::process::Command::new(program).arg(url).spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

static OPEN_SETTINGS: AtomicBool = AtomicBool::new(false);

/// Asks the window, when it next draws, to open Settings. Used by the tray and
/// the menu bar, which cannot change the page themselves.
pub fn request_settings() {
    OPEN_SETTINGS.store(true, Ordering::Relaxed);
}

pub fn take_open_settings() -> bool {
    OPEN_SETTINGS.swap(false, Ordering::Relaxed)
}

/// Selects the update backend for this build: GitHub releases plus the
/// installer that matches how Otozane was installed.
pub fn platform_backend() -> Box<dyn Backend> {
    crate::update_backend::backend()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn settings_and_updates_can_lock_a_fresh_installation() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("updates");
        let update = lock_installation_in(&staging).unwrap();
        assert_eq!(
            lock_installation_in(&staging).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        drop(update);
        assert!(lock_installation_in(&staging).is_ok());
    }

    fn release() -> Release {
        Release {
            id: 1,
            version: "0.6.0".into(),
            notes: "What changed.".into(),
            page_url: "https://github.com/binido/Otozane/releases/tag/v0.6.0".into(),
            action: Action::Update,
        }
    }

    struct FakeBackend {
        check: Result<Option<Release>, String>,
        stage_error: Option<String>,
        restart_error: Option<String>,
        staged: Arc<AtomicBool>,
        applied: Arc<AtomicBool>,
        restarted: Arc<AtomicBool>,
    }

    impl FakeBackend {
        fn new(
            check: Result<Option<Release>, String>,
        ) -> (Self, Arc<AtomicBool>, Arc<AtomicBool>, Arc<AtomicBool>) {
            let staged = Arc::new(AtomicBool::new(false));
            let applied = Arc::new(AtomicBool::new(false));
            let restarted = Arc::new(AtomicBool::new(false));
            (
                Self {
                    check,
                    stage_error: None,
                    restart_error: None,
                    staged: staged.clone(),
                    applied: applied.clone(),
                    restarted: restarted.clone(),
                },
                staged,
                applied,
                restarted,
            )
        }
    }

    impl Backend for FakeBackend {
        fn check(&mut self) -> Result<Option<Release>, String> {
            self.check.clone()
        }

        fn stage(&mut self, _release: &Release) -> Result<(), String> {
            if let Some(e) = &self.stage_error {
                return Err(e.clone());
            }
            self.staged.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn apply(&mut self) -> Result<(), String> {
            if self.applied.swap(true, Ordering::SeqCst) {
                return Err("The update was already installed.".into());
            }
            Ok(())
        }

        fn discard(&mut self) {
            self.staged.store(false, Ordering::SeqCst);
        }

        fn restart(&mut self) -> Result<(), String> {
            if let Some(error) = self.restart_error.take() {
                return Err(error);
            }
            if !self.applied.load(Ordering::SeqCst) {
                return Err("The update was not installed.".into());
            }
            if self.restarted.swap(true, Ordering::SeqCst) {
                return Err("The update already restarted.".into());
            }
            Ok(())
        }
    }

    fn wait(updater: &Updater, done: impl Fn(&State) -> bool) -> State {
        for _ in 0..400 {
            let state = updater.snapshot();
            if done(&state) {
                return state;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out, state: {:?}", updater.snapshot());
    }

    #[test]
    fn auto_check_is_due_after_the_interval() {
        let mut prefs = Prefs {
            check_automatically: true,
            interval: Interval::OneDay,
            last_check: None,
        };
        assert!(prefs.due(0, prefs.last_check), "never checked");
        prefs.last_check = Some(1_000_000);
        assert!(!prefs.due(1_000_000 + 86_399, prefs.last_check));
        assert!(prefs.due(1_000_000 + 86_400, prefs.last_check));
        assert!(
            !prefs.due(999_990, prefs.last_check),
            "a clock that went backwards waits"
        );
        prefs.check_automatically = false;
        prefs.last_check = None;
        assert!(!prefs.due(0, prefs.last_check), "off means never due");
    }

    #[test]
    fn worker_checks_stages_and_applies() {
        let (fake, staged, applied, _) = FakeBackend::new(Ok(Some(release())));
        let updater = Updater::spawn(Box::new(fake), Prefs::default(), |_| {});
        updater.check();
        let state = wait(&updater, |s| s.phase == Phase::Available);
        assert_eq!(state.release.unwrap().version, "0.6.0");
        assert!(state.last_check.is_some());

        updater.stage(&release());
        wait(&updater, |s| s.phase == Phase::Ready);
        assert!(staged.load(Ordering::SeqCst));

        updater.apply();
        wait(&updater, |s| s.phase == Phase::Restarting);
        assert!(applied.load(Ordering::SeqCst));
    }

    #[test]
    fn worker_persists_and_schedules_from_the_last_attempt_without_a_window() {
        let (fake, _, _, _) = FakeBackend::new(Ok(None));
        let prefs = Prefs {
            check_automatically: true,
            interval: Interval::SixHours,
            last_check: Some(now() - 7 * 3600),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let updater = Updater::spawn(Box::new(fake), prefs, move |time| tx.send(time).unwrap());
        let start_time = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let state = wait(&updater, |s| s.phase == Phase::UpToDate);
        assert_eq!(state.last_check, Some(start_time));

        updater.set_schedule(true, Interval::TwelveHours);
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Err(RecvTimeoutError::Timeout)
        );
        updater.check();
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)),
            Err(RecvTimeoutError::Timeout)
        );
    }

    #[test]
    fn recent_check_is_not_repeated_at_start() {
        let (fake, _, _, _) = FakeBackend::new(Ok(Some(release())));
        let prefs = Prefs {
            check_automatically: true,
            interval: Interval::OneWeek,
            last_check: Some(now()),
        };
        let updater = Updater::spawn(Box::new(fake), prefs, |_| {});
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(updater.snapshot().phase, Phase::Idle);
    }

    #[test]
    fn check_error_is_actionable_and_keeps_the_version() {
        let message = "Could not reach GitHub. Check your connection and try again.";
        let (fake, _, _, _) = FakeBackend::new(Err(message.into()));
        let updater = Updater::spawn(Box::new(fake), Prefs::default(), |_| {});
        updater.check();
        let state = wait(&updater, |s| matches!(s.phase, Phase::Failed(_)));
        assert_eq!(state.phase, Phase::Failed(message.into()));
        assert!(state.release.is_none());
        assert!(!state.current_version.is_empty());
    }

    #[test]
    fn stage_error_keeps_the_release_for_a_retry() {
        let (mut fake, staged, _, _) = FakeBackend::new(Ok(Some(release())));
        fake.stage_error = Some("Checksum mismatch. The download was removed, try again.".into());
        let updater = Updater::spawn(Box::new(fake), Prefs::default(), |_| {});
        updater.check();
        wait(&updater, |s| s.phase == Phase::Available);
        updater.stage(&release());
        let state = wait(&updater, |s| matches!(s.phase, Phase::Failed(_)));
        assert!(state.release.is_some(), "retry needs the release");
        assert!(!staged.load(Ordering::SeqCst));
    }

    #[test]
    fn only_the_selected_release_is_staged_and_discard_allows_new_checks() {
        let (fake, staged, applied, restarted) = FakeBackend::new(Ok(Some(release())));
        let updater = Updater::spawn(Box::new(fake), Prefs::default(), |_| {});
        updater.check();
        wait(&updater, |s| s.phase == Phase::Available);
        let mut stale = release();
        stale.id += 1;
        updater.stage(&stale);
        updater.apply();
        updater.check();
        std::thread::sleep(Duration::from_millis(50));
        assert!(!staged.load(Ordering::SeqCst));
        assert!(!applied.load(Ordering::SeqCst));

        updater.stage(&release());
        wait(&updater, |s| s.phase == Phase::Ready);
        updater.discard();
        wait(&updater, |s| s.phase == Phase::Available);
        assert!(!staged.load(Ordering::SeqCst));
        updater.stage(&release());
        wait(&updater, |s| s.phase == Phase::Ready);
        updater.apply();
        wait(&updater, |s| s.phase == Phase::Restarting);
        assert!(applied.load(Ordering::SeqCst));
        assert!(restarted.load(Ordering::SeqCst));
    }

    #[test]
    fn retrying_a_failed_handoff_skips_installation() {
        let (mut fake, _, _, restarted) = FakeBackend::new(Ok(Some(release())));
        fake.restart_error = Some("Restart failed".into());
        let updater = Updater::spawn(Box::new(fake), Prefs::default(), |_| {});
        updater.check();
        wait(&updater, |s| s.phase == Phase::Available);
        updater.stage(&release());
        wait(&updater, |s| s.phase == Phase::Ready);
        updater.apply();
        wait(&updater, |s| {
            s.phase == Phase::RestartFailed("Restart failed".into())
        });

        updater.check();
        updater.stage(&release());
        updater.retry_restart();
        wait(&updater, |s| s.phase == Phase::Restarting);
        assert!(restarted.load(Ordering::SeqCst));
    }

    #[test]
    fn update_prefs_round_trip_for_ui_toml() {
        let prefs = Prefs {
            check_automatically: true,
            interval: Interval::TwelveHours,
            last_check: Some(42),
        };
        let text = toml::to_string(&prefs).unwrap();
        let back: Prefs = toml::from_str(&text).unwrap();
        assert_eq!(back, prefs);
        assert_eq!(toml::from_str::<Prefs>("").unwrap(), Prefs::default());
    }
}
