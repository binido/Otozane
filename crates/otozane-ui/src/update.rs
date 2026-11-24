//! Update checks and installation, off the UI thread.
//!
//! The UI only queues commands and reads the latest published state, so a
//! network request, a checksum check or a package manager prompt never blocks
//! a frame. The real GitHub release and platform installer code implements
//! [`Backend`] in its own module. [`platform_backend`] is the single place this
//! build selects one. The UI never restarts the app on its own: a check only
//! finds a release, staging only downloads and verifies it, and applying (with
//! its restart) happens after the user confirms.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Version of this build, e.g. `0.5.0`.
pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
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
    pub fn due(&self, now: i64) -> bool {
        let interval = self.interval.duration().as_secs() as i64;
        self.check_automatically
            && self
                .last_check
                .is_none_or(|last| now.saturating_sub(last) >= interval)
    }
}

/// What the backend can do with a newer release.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// The backend downloads, verifies and installs it in place.
    Update,
    /// Not supported from inside the app: open the release page instead.
    /// `hint` tells the user how this install is updated.
    // Constructed by the platform backends, which live in their own modules.
    #[allow(dead_code)]
    ReleasePage { hint: String },
}

/// A release newer than this build.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// Version without the leading `v`, e.g. `0.6.0`.
    pub version: String,
    /// Release notes, generated from `CHANGELOG.md`.
    pub notes: String,
    /// GitHub release page.
    pub page_url: String,
    pub action: Action,
}

/// Installs Otozane updates. Implemented by the release and platform backends.
/// Every method runs on the update thread and may block.
///
/// An error is a sentence for the user that says what to do next. Nothing may
/// be left half-installed: a failed stage or apply keeps the running version.
pub trait Backend: Send + 'static {
    fn current_version(&self) -> String {
        app_version().to_string()
    }

    /// The newest release, or `None` when this build is current.
    fn check(&mut self) -> Result<Option<Release>, String>;

    /// Downloads and verifies the release without touching the running app.
    fn stage(&mut self, release: &Release) -> Result<(), String>;

    /// Installs the staged update and restarts Otozane. Called only after the
    /// user confirms the restart.
    fn apply(&mut self) -> Result<(), String>;
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
    /// Applied. The backend is restarting the app.
    Restarting,
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
    SetPrefs(Prefs),
    Stage,
    Apply,
}

/// Handle to the update thread.
#[derive(Clone)]
pub struct Updater {
    tx: Sender<Command>,
    state: Arc<Mutex<State>>,
}

impl Updater {
    pub fn spawn(backend: Box<dyn Backend>, prefs: Prefs) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let state = Arc::new(Mutex::new(State {
            current_version: backend.current_version(),
            last_check: prefs.last_check,
            ..Default::default()
        }));
        let st = state.clone();
        std::thread::Builder::new()
            .name("otozane-updates".into())
            .spawn(move || run(backend, prefs, rx, st))
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

    /// Downloads and verifies the release found by the last check.
    pub fn stage(&self) {
        let _ = self.tx.send(Command::Stage);
    }

    /// Installs the staged release and restarts. Confirmation is the UI's job.
    pub fn apply(&self) {
        let _ = self.tx.send(Command::Apply);
    }

    pub fn set_prefs(&self, prefs: Prefs) {
        let _ = self.tx.send(Command::SetPrefs(prefs));
    }
}

/// How often the update thread re-reads the clock while idle. Intervals are at
/// least six hours, so a minute is fine and keeps the thread quiet.
const IDLE_POLL: Duration = Duration::from_secs(60);

fn run(
    mut backend: Box<dyn Backend>,
    mut prefs: Prefs,
    rx: Receiver<Command>,
    state: Arc<Mutex<State>>,
) {
    if prefs.due(now()) {
        check(&mut *backend, &state);
    }
    loop {
        match rx.recv_timeout(IDLE_POLL) {
            Ok(Command::Check) => {
                if !busy(&state) {
                    check(&mut *backend, &state);
                }
            }
            Ok(Command::SetPrefs(p)) => prefs = p,
            Ok(Command::Stage) => {
                let release = state.lock().unwrap().release.clone();
                if let Some(release) = release
                    && release.action == Action::Update
                    && !busy(&state)
                {
                    stage(&mut *backend, &state, &release);
                }
            }
            Ok(Command::Apply) => {
                let ready = state.lock().unwrap().phase == Phase::Ready;
                if ready {
                    apply(&mut *backend, &state);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if !busy(&state) && prefs.due(now()) {
            check(&mut *backend, &state);
        }
    }
}

fn busy(state: &Mutex<State>) -> bool {
    matches!(
        state.lock().unwrap().phase,
        Phase::Checking | Phase::Staging | Phase::Ready | Phase::Applying | Phase::Restarting
    )
}

fn publish(state: &Mutex<State>, phase: Phase) {
    state.lock().unwrap().phase = phase;
}

fn check(backend: &mut dyn Backend, state: &Mutex<State>) {
    state.lock().unwrap().last_check = Some(now());
    publish(state, Phase::Checking);
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

fn apply(backend: &mut dyn Backend, state: &Mutex<State>) {
    publish(state, Phase::Applying);
    publish(
        state,
        match backend.apply() {
            Ok(()) => Phase::Restarting,
            Err(e) => Phase::Failed(e),
        },
    );
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

const NO_BACKEND: &str = "Updates are not available in this build. Download the latest version from https://github.com/binido/Otozane/releases/latest";

/// Selects the update backend for this build.
///
/// The GitHub release backend and the platform installers implement [`Backend`]
/// in their own modules. Replacing this function is the whole integration.
pub fn platform_backend() -> Box<dyn Backend> {
    Box::new(Unavailable)
}

struct Unavailable;

impl Backend for Unavailable {
    fn check(&mut self) -> Result<Option<Release>, String> {
        Err(NO_BACKEND.to_string())
    }

    fn stage(&mut self, _release: &Release) -> Result<(), String> {
        Err(NO_BACKEND.to_string())
    }

    fn apply(&mut self) -> Result<(), String> {
        Err(NO_BACKEND.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn release() -> Release {
        Release {
            version: "0.6.0".into(),
            notes: "What changed.".into(),
            page_url: "https://github.com/binido/Otozane/releases/tag/v0.6.0".into(),
            action: Action::Update,
        }
    }

    struct FakeBackend {
        check: Result<Option<Release>, String>,
        stage_error: Option<String>,
        staged: Arc<AtomicBool>,
        applied: Arc<AtomicBool>,
    }

    impl FakeBackend {
        fn new(check: Result<Option<Release>, String>) -> (Self, Arc<AtomicBool>, Arc<AtomicBool>) {
            let staged = Arc::new(AtomicBool::new(false));
            let applied = Arc::new(AtomicBool::new(false));
            (
                Self {
                    check,
                    stage_error: None,
                    staged: staged.clone(),
                    applied: applied.clone(),
                },
                staged,
                applied,
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
            self.applied.store(true, Ordering::SeqCst);
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
        assert!(prefs.due(0), "never checked");
        prefs.last_check = Some(1_000_000);
        assert!(!prefs.due(1_000_000 + 86_399));
        assert!(prefs.due(1_000_000 + 86_400));
        assert!(!prefs.due(999_990), "a clock that went backwards waits");
        prefs.check_automatically = false;
        prefs.last_check = None;
        assert!(!prefs.due(0), "off means never due");
    }

    #[test]
    fn worker_checks_stages_and_applies() {
        let (fake, staged, applied) = FakeBackend::new(Ok(Some(release())));
        let updater = Updater::spawn(Box::new(fake), Prefs::default());
        updater.check();
        let state = wait(&updater, |s| s.phase == Phase::Available);
        assert_eq!(state.release.unwrap().version, "0.6.0");
        assert!(state.last_check.is_some());

        updater.stage();
        wait(&updater, |s| s.phase == Phase::Ready);
        assert!(staged.load(Ordering::SeqCst));

        updater.apply();
        wait(&updater, |s| s.phase == Phase::Restarting);
        assert!(applied.load(Ordering::SeqCst));
    }

    #[test]
    fn worker_runs_a_due_automatic_check_at_start() {
        let (fake, _, _) = FakeBackend::new(Ok(None));
        let prefs = Prefs {
            check_automatically: true,
            interval: Interval::SixHours,
            last_check: None,
        };
        let updater = Updater::spawn(Box::new(fake), prefs);
        wait(&updater, |s| s.phase == Phase::UpToDate);
        assert!(!updater.snapshot().has_update());
    }

    #[test]
    fn recent_check_is_not_repeated_at_start() {
        let (fake, _, _) = FakeBackend::new(Ok(Some(release())));
        let prefs = Prefs {
            check_automatically: true,
            interval: Interval::OneWeek,
            last_check: Some(now()),
        };
        let updater = Updater::spawn(Box::new(fake), prefs);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(updater.snapshot().phase, Phase::Idle);
    }

    #[test]
    fn check_error_is_actionable_and_keeps_the_version() {
        let message = "Could not reach GitHub. Check your connection and try again.";
        let (fake, _, _) = FakeBackend::new(Err(message.into()));
        let updater = Updater::spawn(Box::new(fake), Prefs::default());
        updater.check();
        let state = wait(&updater, |s| matches!(s.phase, Phase::Failed(_)));
        assert_eq!(state.phase, Phase::Failed(message.into()));
        assert!(state.release.is_none());
        assert!(!state.current_version.is_empty());
    }

    #[test]
    fn stage_error_keeps_the_release_for_a_retry() {
        let (mut fake, staged, _) = FakeBackend::new(Ok(Some(release())));
        fake.stage_error = Some("Checksum mismatch. The download was removed, try again.".into());
        let updater = Updater::spawn(Box::new(fake), Prefs::default());
        updater.check();
        wait(&updater, |s| s.phase == Phase::Available);
        updater.stage();
        let state = wait(&updater, |s| matches!(s.phase, Phase::Failed(_)));
        assert!(state.release.is_some(), "retry needs the release");
        assert!(!staged.load(Ordering::SeqCst));
    }

    #[test]
    fn no_backend_error_says_what_to_do() {
        let updater = Updater::spawn(platform_backend(), Prefs::default());
        updater.check();
        let state = wait(&updater, |s| matches!(s.phase, Phase::Failed(_)));
        let Phase::Failed(message) = state.phase else {
            unreachable!()
        };
        assert!(message.contains("releases/latest"), "{message}");
    }
}
