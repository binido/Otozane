use crate::update::{self, Action, Backend, Release};
use crate::update_release::{self, HttpTransport, Response, Transport, UpdateError};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

/// Builds the backend for this build's platform.
pub fn backend() -> Box<dyn Backend> {
    Box::new(Github::new(Https::new()))
}

/// An HTTPS transport, or the reason it could not be created.
enum Https {
    Ready(HttpTransport),
    Failed(String),
}

impl Https {
    fn new() -> Self {
        match HttpTransport::new() {
            Ok(transport) => Self::Ready(transport),
            Err(e) => Self::Failed(e.to_string()),
        }
    }
}

impl Transport for Https {
    fn get(&self, url: &str, limit: u64) -> Result<Response, UpdateError> {
        match self {
            Self::Ready(transport) => transport.get(url, limit),
            Self::Failed(e) => Err(UpdateError::Network(e.clone())),
        }
    }

    fn download(&self, url: &str, dest: &Path, limit: u64) -> Result<Response, UpdateError> {
        match self {
            Self::Ready(transport) => transport.download(url, dest, limit),
            Self::Failed(e) => Err(UpdateError::Network(e.clone())),
        }
    }
}

/// GitHub releases plus the platform installer.
pub struct Github<T: Transport> {
    transport: T,
    staging: PathBuf,
    release: Option<update_release::Release>,
    version: Option<String>,
    asset: Option<String>,
    platform: platform::Platform,
}

impl Github<Https> {
    fn new(transport: Https) -> Self {
        let staging = update::staging_dir();
        if let Err(e) = DownloadAttempt::reap_abandoned(&staging)
            && e.kind() != io::ErrorKind::NotFound
        {
            eprintln!("Could not clean up abandoned update downloads: {e}");
        }
        Self::with_staging(transport, staging)
    }
}

impl<T: Transport> Github<T> {
    fn with_staging(transport: T, staging: PathBuf) -> Self {
        Self {
            transport,
            staging,
            release: None,
            version: None,
            asset: None,
            platform: platform::Platform::new(),
        }
    }

    fn reset(&mut self) {
        self.release = None;
        self.version = None;
        self.asset = None;
        self.platform.reset();
    }

    /// The release check and the platform decision, without the worker.
    fn prepared(&mut self, current: &str) -> Result<Option<Release>, String> {
        self.reset();
        let found = update_release::check(&self.transport, current).map_err(check_message)?;
        let Some(found) = found else {
            return Ok(None);
        };
        let version = found.version.to_string();
        let (action, asset) = self.platform.select(&version, &found.release);
        let page_url = format!(
            "https://github.com/{}/releases/tag/{}",
            update_release::REPO,
            found.release.tag_name
        );
        let notes = found.release.notes().to_string();
        let id = found.release.id;
        self.release = Some(found.release);
        self.version = Some(version.clone());
        self.asset = asset;
        Ok(Some(Release {
            id,
            version,
            notes,
            page_url,
            action,
        }))
    }
}

impl<T: Transport + Send + 'static> Backend for Github<T> {
    fn check(&mut self) -> Result<Option<Release>, String> {
        self.prepared(update::app_version())
    }

    fn stage(&mut self, release: &Release) -> Result<(), String> {
        let (Some(raw), Some(asset)) = (self.release.as_ref(), self.asset.as_ref()) else {
            return Err("Check for updates again before downloading.".into());
        };
        if raw.id != release.id || self.version.as_deref() != Some(release.version.as_str()) {
            return Err("Check for updates again before downloading.".into());
        }
        self.platform.discard();
        let archive =
            DownloadAttempt::new(&self.staging).map_err(|e| stage_message(UpdateError::Io(e)))?;
        let path = update_release::download_verified(&self.transport, raw, asset, archive.path())
            .map_err(stage_message)?;
        self.platform.stage(path, &release.version, archive)
    }

    fn discard(&mut self) {
        self.platform.discard();
    }

    fn apply(&mut self) -> Result<(), String> {
        let version = self
            .version
            .clone()
            .ok_or_else(|| "Check for updates again before installing.".to_string())?;
        self.platform.apply(&version)
    }

    fn restart(&mut self) -> Result<(), String> {
        self.platform.restart()
    }
}

struct DownloadAttempt {
    dir: tempfile::TempDir,
    _lease: File,
}

impl DownloadAttempt {
    const PREFIX: &str = "download-attempt-";
    const LEASE: &str = "active.lock";

    fn new(staging: &Path) -> io::Result<Self> {
        fs::create_dir_all(staging)?;
        let registry = File::open(staging)?;
        registry.lock()?;
        let dir = tempfile::Builder::new()
            .prefix(Self::PREFIX)
            .tempdir_in(staging)?;
        let lease = File::create(dir.path().join(Self::LEASE))?;
        lease.lock()?;
        Ok(Self { dir, _lease: lease })
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn reap_abandoned(staging: &Path) -> io::Result<()> {
        // The registry lock covers the gap between creating a directory and locking its lease.
        let registry = File::open(staging)?;
        registry.lock()?;
        for entry in fs::read_dir(staging)?.filter_map(Result::ok) {
            if !entry
                .file_name()
                .as_encoded_bytes()
                .starts_with(Self::PREFIX.as_bytes())
                || !entry.file_type().is_ok_and(|kind| kind.is_dir())
            {
                continue;
            }
            let path = entry.path();
            match File::open(path.join(Self::LEASE)) {
                Ok(lease) if lease.try_lock().is_ok() => {
                    let _ = fs::remove_dir_all(path);
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    let _ = fs::remove_dir(path);
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn check_message(e: UpdateError) -> String {
    match e {
        UpdateError::Network(_) => {
            "Could not reach GitHub. Check the connection and try again.".into()
        }
        UpdateError::RateLimited { .. } => {
            "GitHub's rate limit is exhausted. Try again later or use the release page.".into()
        }
        UpdateError::Http { status } => {
            format!("GitHub returned HTTP {status}. Try again later or use the release page.")
        }
        e => format!("Could not read the latest release: {e}"),
    }
}

fn stage_message(e: UpdateError) -> String {
    match e {
        UpdateError::ChecksumMismatch { .. } => {
            "The download did not match SHA256SUMS. It was removed and nothing was installed. Try again.".into()
        }
        UpdateError::ChecksumMissing { .. } => {
            "The release has no checksum for this file, so nothing was installed. Use the release page.".into()
        }
        UpdateError::Network(_) => {
            "The download failed. Check the connection and try again; nothing was installed."
                .into()
        }
        UpdateError::AssetNotFound { .. } => {
            "The release has no package for this installation. Use the release page.".into()
        }
        e => format!("The update could not be downloaded: {e} Nothing was installed."),
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use crate::update_macos;

    /// The app bundle found at check time and the app prepared by `stage`.
    pub struct Platform {
        bundle: Option<update_macos::Bundle>,
        staged: Option<update_macos::Staged>,
    }

    impl Platform {
        pub fn new() -> Self {
            Self {
                bundle: None,
                staged: None,
            }
        }

        pub fn reset(&mut self) {
            self.bundle = None;
            self.discard();
        }

        pub fn discard(&mut self) {
            self.staged = None;
        }

        pub fn select(
            &mut self,
            version: &str,
            release: &update_release::Release,
        ) -> (Action, Option<String>) {
            let bundle = match update_macos::detect() {
                Ok(bundle) => bundle,
                Err(e) => return page(&e),
            };
            let asset = match update_macos::asset_name(version) {
                Ok(asset) => asset,
                Err(e) => return page(&e),
            };
            if release.asset(&asset).is_err() {
                return page("The release has no macOS package for this installation.");
            }
            self.bundle = Some(bundle);
            (Action::Update, Some(asset))
        }

        pub fn stage(
            &mut self,
            dmg: PathBuf,
            version: &str,
            _archive: DownloadAttempt,
        ) -> Result<(), String> {
            let bundle = self.bundle.as_ref().ok_or_else(|| {
                "Otozane cannot update itself from here. Download the latest release instead."
                    .to_string()
            })?;
            self.staged = Some(update_macos::stage(&dmg, bundle, version)?);
            Ok(())
        }

        pub fn apply(&mut self, _version: &str) -> Result<(), String> {
            if self.staged.is_none() {
                return Err("The update is not prepared yet.".into());
            }
            Ok(())
        }

        pub fn restart(&mut self) -> Result<(), String> {
            let staged = self
                .staged
                .as_mut()
                .ok_or_else(|| "There is no prepared update to restart into.".to_string())?;
            staged.restart()
        }
    }

    fn page(reason: &str) -> (Action, Option<String>) {
        (
            Action::ReleasePage {
                hint: format!("{reason} Download the latest release:"),
            },
            None,
        )
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use crate::update_linux::{self, Install};

    /// How this copy was installed, the verified download and the restart plan.
    pub struct Platform {
        install: Option<Install>,
        staged: Option<(PathBuf, DownloadAttempt)>,
        restart: Option<(update_linux::Restart, std::fs::File)>,
    }

    impl Platform {
        pub fn new() -> Self {
            Self {
                install: None,
                staged: None,
                restart: None,
            }
        }

        pub fn reset(&mut self) {
            self.install = None;
            self.discard();
            self.restart = None;
        }

        pub fn discard(&mut self) {
            self.staged = None;
        }

        pub fn select(
            &mut self,
            version: &str,
            release: &update_release::Release,
        ) -> (Action, Option<String>) {
            let install = Install::detect();
            match install.action(version) {
                Ok(update_linux::Action::Install { asset }) => {
                    if release.asset(&asset).is_err() {
                        return page("The release has no Linux package for this installation.");
                    }
                    self.install = Some(install);
                    (Action::Update, Some(asset))
                }
                Ok(update_linux::Action::Notify { hint }) => (Action::ReleasePage { hint }, None),
                Err(e) => page(&e.to_string()),
            }
        }

        pub fn stage(
            &mut self,
            asset: PathBuf,
            _version: &str,
            archive: DownloadAttempt,
        ) -> Result<(), String> {
            self.staged = Some((asset, archive));
            Ok(())
        }

        pub fn apply(&mut self, version: &str) -> Result<(), String> {
            let install = self
                .install
                .as_ref()
                .ok_or_else(|| "This installation cannot be updated from the app.".to_string())?;
            let (staged, _) = self
                .staged
                .as_ref()
                .ok_or_else(|| "The update is not downloaded yet.".to_string())?;
            let lock = update::installation_lock()?;
            let plan = update_linux::Restart::current().map_err(|e| e.to_string())?;
            let was_running = crate::client::service_is_running()
                .map_err(|e| format!("Could not check the audio service status: {e}"))?;
            let install_update = || {
                plan.stop()
                    .map_err(|e| format!("Could not stop the audio service: {e}"))?;
                update_linux::install(install, version, staged)
                    .map_err(|e| format!("The update could not be installed: {e}"))
            };
            let result = install_update();
            self.discard();
            result.map_err(|e| crate::client::restore_after_update_error(was_running, e))?;
            self.restart = Some((plan, lock));
            Ok(())
        }

        pub fn restart(&mut self) -> Result<(), String> {
            let (plan, _) = self
                .restart
                .as_ref()
                .ok_or_else(|| "There is no installed update to restart into.".to_string())?;
            update_linux::restart(plan)
                .map_err(|e| format!("The update is installed. Could not restart Otozane: {e}"))
        }
    }

    fn page(reason: &str) -> (Action, Option<String>) {
        (
            Action::ReleasePage {
                hint: format!(
                    "{reason} Download the latest release: {}",
                    update_linux::RELEASES_URL
                ),
            },
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update_release::{LATEST_RELEASE_URL, parse_release};
    use std::fs;

    const RELEASE_JSON: &str = include_str!("../tests/fixtures/release_latest.json");
    const SUMS: &str = include_str!("../tests/fixtures/SHA256SUMS");
    const DMG: &[u8] = b"fake dmg payload\n";
    const ASSET: &str = "otozane-v0.6.0-macos-arm64.dmg";

    struct Fixture {
        offline: bool,
        status: u16,
        rate_limited: bool,
        sums: String,
        download_fails: bool,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                offline: false,
                status: 200,
                rate_limited: false,
                sums: SUMS.to_string(),
                download_fails: false,
            }
        }
    }

    impl Transport for Fixture {
        fn get(&self, url: &str, _limit: u64) -> Result<Response, UpdateError> {
            if self.offline {
                return Err(UpdateError::Network("offline".into()));
            }
            if url == LATEST_RELEASE_URL {
                let body = if self.status == 200 {
                    RELEASE_JSON.as_bytes().to_vec()
                } else {
                    Vec::new()
                };
                Ok(Response {
                    status: self.status,
                    rate_limited: self.rate_limited,
                    reset_at: None,
                    body,
                })
            } else if url.ends_with("/SHA256SUMS") {
                Ok(Response {
                    status: 200,
                    body: self.sums.as_bytes().to_vec(),
                    ..Response::default()
                })
            } else {
                Err(UpdateError::Http { status: 404 })
            }
        }

        fn download(&self, _url: &str, dest: &Path, _limit: u64) -> Result<Response, UpdateError> {
            if self.download_fails {
                fs::write(dest, b"partial").unwrap();
                return Err(UpdateError::Network("offline".into()));
            }
            fs::write(dest, DMG).unwrap();
            Ok(Response {
                status: 200,
                ..Response::default()
            })
        }
    }

    fn release() -> Release {
        Release {
            id: 0,
            version: "0.6.0".into(),
            notes: String::new(),
            page_url: String::new(),
            action: Action::Update,
        }
    }

    /// A backend past its check, so `stage` can run without a real bundle.
    fn checked(fixture: Fixture, staging: &Path) -> Github<Fixture> {
        let mut backend = Github::with_staging(fixture, staging.to_path_buf());
        backend.release = Some(parse_release(RELEASE_JSON.as_bytes()).unwrap());
        backend.version = Some("0.6.0".into());
        backend.asset = Some(ASSET.into());
        backend
    }

    #[test]
    fn newer_release_carries_notes_and_a_page() {
        let dir = tempfile::tempdir().unwrap();
        let mut backend = Github::with_staging(Fixture::new(), dir.path().to_path_buf());
        let found = backend.prepared("0.5.0").unwrap().unwrap();
        assert_eq!(found.version, "0.6.0");
        assert!(found.notes.contains("Update from the app"));
        assert_eq!(
            found.page_url,
            "https://github.com/binido/Otozane/releases/tag/v0.6.0"
        );
        match found.action {
            Action::Update => assert!(backend.asset.is_some()),
            Action::ReleasePage { hint } => assert!(!hint.is_empty()),
        }
        assert_eq!(backend.prepared("0.6.0").unwrap(), None);
        assert_eq!(backend.prepared("0.7.0").unwrap(), None);
    }

    #[test]
    fn installs_the_app_cannot_replace_only_get_the_release_page() {
        let dir = tempfile::tempdir().unwrap();
        let mut backend = Github::with_staging(Fixture::new(), dir.path().to_path_buf());
        let found = backend.prepared("0.5.0").unwrap().unwrap();
        // The test binary does not run from an installed app or package.
        assert!(
            matches!(found.action, Action::ReleasePage { .. }),
            "{:?}",
            found.action
        );
        assert!(backend.asset.is_none(), "no asset to download");
        assert!(backend.stage(&found).is_err(), "no Update button path");
    }

    #[test]
    fn offline_check_says_what_to_do() {
        let dir = tempfile::tempdir().unwrap();
        let mut fixture = Fixture::new();
        fixture.offline = true;
        let mut backend = Github::with_staging(fixture, dir.path().to_path_buf());
        let error = backend.prepared("0.5.0").unwrap_err();
        assert!(error.contains("Could not reach GitHub"), "{error}");
    }

    #[test]
    fn rate_limited_check_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let mut fixture = Fixture::new();
        fixture.status = 403;
        fixture.rate_limited = true;
        let mut backend = Github::with_staging(fixture, dir.path().to_path_buf());
        let error = backend.prepared("0.5.0").unwrap_err();
        assert!(error.contains("rate limit"), "{error}");
    }

    #[test]
    fn checksum_mismatch_leaves_no_file_and_no_install() {
        let dir = tempfile::tempdir().unwrap();
        let mut fixture = Fixture::new();
        fixture.sums = SUMS.replace(
            "4bfeab0aa1c4b426af048ffa79405a3bf2f3d71f4b534f50161ecc6b84585d20",
            &"0".repeat(64),
        );
        let mut backend = checked(fixture, dir.path());
        let error = backend.stage(&release()).unwrap_err();
        assert!(error.contains("SHA256SUMS"), "{error}");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        assert!(backend.apply().is_err(), "nothing was installed");
    }

    #[test]
    fn startup_reaps_partial_downloads_from_exited_workers_only() {
        let staging = tempfile::tempdir().unwrap();
        let active = DownloadAttempt::new(staging.path()).unwrap();
        let abandoned = DownloadAttempt::new(staging.path()).unwrap();
        fs::write(abandoned.path().join(".package.part"), "partial").unwrap();
        let path = abandoned.dir.keep();
        drop(abandoned._lease);

        DownloadAttempt::reap_abandoned(staging.path()).unwrap();
        assert!(active.path().is_dir());
        assert!(!path.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn separate_attempts_own_and_clean_up_their_archives() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = checked(Fixture::new(), dir.path());
        let mut second = checked(Fixture::new(), dir.path());

        first.stage(&release()).unwrap();
        second.stage(&release()).unwrap();
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);

        first.discard();
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        drop(second);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn failed_download_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut fixture = Fixture::new();
        fixture.download_fails = true;
        let mut backend = checked(fixture, dir.path());
        let error = backend.stage(&release()).unwrap_err();
        assert!(error.contains("download failed"), "{error}");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn restart_waits_for_a_prepared_update() {
        let dir = tempfile::tempdir().unwrap();
        let mut backend = checked(Fixture::new(), dir.path());
        assert!(backend.apply().is_err(), "stage comes before apply");
        assert!(backend.restart().is_err(), "nothing staged");
    }
}
