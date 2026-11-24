//! Linux update backend: install detection, release asset choice and installing
//! an asset the release code has verified.
//!
//! `install` only accepts a path that was downloaded and checked against the
//! release's `SHA256SUMS` elsewhere; nothing here downloads or verifies.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Latest release page, shown for installs the app cannot update itself.
pub const RELEASES_URL: &str = "https://github.com/binido/Otozane/releases/latest";

const ARCH_HINT: &str = "Installed from the AUR as otozane-bin. Update with your AUR \
helper, for example: yay -S otozane-bin, or paru -S otozane-bin. Then restart Otozane. \
Releases:";
const UNKNOWN_HINT: &str =
    "This installation method cannot be updated from the app. Download the latest release:";

/// How this copy of Otozane was installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    /// `/usr/bin` from the release `.deb`, updated with apt through polkit.
    Deb,
    /// `/usr/bin` from the release `.rpm`, updated with dnf or zypper through polkit.
    Rpm,
    /// pacman's `otozane-bin`; updating belongs to the user's AUR helper.
    Arch,
    /// `scripts/install.sh` into a prefix, `~/.local` by default.
    User {
        prefix: PathBuf,
    },
    Unknown,
}

/// What Settings, Updates offers for a newer release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Download and verify this exact asset, then pass it to `install`.
    Install { asset: String },
    /// Show the hint and a link to the release page instead of an Update button.
    Notify { hint: String },
}

/// How to bring the service back after an install.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    /// `restart` runs `systemctl --user restart otozane.service`.
    Systemd,
    /// Starts the daemon directly when no user unit is installed.
    Detached,
}

/// Relaunch paths captured before installation replaces the running binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restart {
    service: Service,
    app: PathBuf,
}

impl Restart {
    pub fn current() -> Result<Self, Error> {
        let app = std::env::current_exe()?;
        if !app.is_file() {
            return Err(io::Error::other(
                "The running app was replaced. Reopen Otozane and check again.",
            )
            .into());
        }
        Ok(Self {
            service: if user_unit_exists() {
                Service::Systemd
            } else {
                Service::Detached
            },
            app,
        })
    }

    pub fn stop(&self) -> io::Result<()> {
        crate::client::stop_for_update(|| {
            if self.service == Service::Systemd {
                crate::client::systemctl(&["stop", "otozane.service"])?;
            }
            Ok(())
        })
    }
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Version(String),
    Arch(String),
    NotInstallable(String),
    WrongAsset {
        expected: String,
        found: String,
    },
    Staged(String),
    Layout(String),
    Command {
        command: String,
        detail: String,
    },
    Package {
        command: String,
        detail: String,
        state: String,
        recovery: &'static str,
    },
    Rollback {
        install: io::Error,
        restore: io::Error,
        work: PathBuf,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Version(v) => write!(f, "not a release version: {v}"),
            Error::Arch(a) => write!(f, "Otozane has no Linux build for this CPU ({a})"),
            Error::NotInstallable(what) => write!(f, "{what} cannot be updated from the app"),
            Error::WrongAsset { expected, found } => {
                write!(f, "expected the asset {expected}, got {found}")
            }
            Error::Staged(path) => write!(f, "the downloaded update is missing: {path}"),
            Error::Layout(detail) => write!(f, "the release archive is incomplete: {detail}"),
            Error::Command { command, detail } => write!(f, "{command} failed: {detail}"),
            Error::Package {
                command,
                detail,
                state,
                recovery,
            } => write!(
                f,
                "{command} failed: {detail}. Package state: {state}. {recovery}"
            ),
            Error::Rollback {
                install,
                restore,
                work,
            } => write!(
                f,
                "Replacing files failed: {install}. Rollback was incomplete: {restore}. Recovery files are in {}.",
                work.display()
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl Install {
    /// Detects how this binary was installed.
    pub fn detect() -> Install {
        let Ok(exe) = std::env::current_exe() else {
            return Install::Unknown;
        };
        Install::detect_at(Path::new("/"), &exe)
    }

    /// Detects the install for `exe` inside `root`, so tests can use a temporary root.
    pub fn detect_at(root: &Path, exe: &Path) -> Install {
        let in_system_bin = exe.parent() == Some(root.join("usr/bin").as_path());
        if in_system_bin {
            // A package database entry is the only proof that a system-wide
            // install belongs to a package rather than a hand-copied payload.
            if pacman_owns(root) {
                return Install::Arch;
            }
            if dpkg_owns(root, exe) {
                return Install::Deb;
            }
            if root == Path::new("/") && rpm_owns(exe) {
                return Install::Rpm;
            }
        }
        let is_bin = exe.parent().and_then(Path::file_name) == Some(OsStr::new("bin"));
        if let Some(prefix) = exe.parent().filter(|_| is_bin).and_then(Path::parent) {
            // `/usr` without a package behind it stays Unknown: writing there
            // as the user would fail halfway.
            if prefix != root && prefix != root.join("usr") && prefix.join("lib/otozane").is_dir() {
                return Install::User {
                    prefix: prefix.to_path_buf(),
                };
            }
        }
        Install::Unknown
    }

    /// Short label for Settings, Updates.
    pub fn label(&self) -> String {
        match self {
            Install::Deb => "Debian package (apt)".to_owned(),
            Install::Rpm => "Fedora/openSUSE package (dnf or zypper)".to_owned(),
            Install::Arch => "Arch package (otozane-bin)".to_owned(),
            Install::User { prefix } => format!("User install in {}", prefix.display()),
            Install::Unknown => "Unrecognised install".to_owned(),
        }
    }

    /// Chooses the release asset for `version` and how to present it.
    pub fn action(&self, version: &str) -> Result<Action, Error> {
        let version = release_version(version)?;
        let (arch, deb_arch) = match self {
            Install::Deb | Install::Rpm | Install::User { .. } => host_arch()?,
            Install::Arch => {
                return Ok(Action::Notify {
                    hint: format!("{ARCH_HINT} {RELEASES_URL}"),
                });
            }
            Install::Unknown => {
                return Ok(Action::Notify {
                    hint: format!("{UNKNOWN_HINT} {RELEASES_URL}"),
                });
            }
        };
        let asset = match self {
            Install::Deb => format!("otozane_{version}-1_{deb_arch}.deb"),
            Install::Rpm => format!("otozane-{version}-1.{arch}.rpm"),
            Install::User { .. } => format!("otozane-v{version}-linux-{arch}.tar.gz"),
            Install::Arch | Install::Unknown => unreachable!("handled above"),
        };
        Ok(Action::Install { asset })
    }
}

/// Installs a verified asset.
pub fn install(install: &Install, version: &str, staged: &Path) -> Result<(), Error> {
    let version = release_version(version)?;
    let Action::Install { asset } = install.action(version)? else {
        return Err(Error::NotInstallable(install.label()));
    };
    let staged =
        fs::canonicalize(staged).map_err(|_| Error::Staged(staged.display().to_string()))?;
    if staged.file_name() != Some(OsStr::new(&asset)) {
        let found = staged
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_owned();
        return Err(Error::WrongAsset {
            expected: asset,
            found,
        });
    }
    match install {
        Install::Deb => run_root("apt-get", &["install", "-y"], &staged)?,
        Install::Rpm => install_rpm(&staged)?,
        Install::User { prefix } => install_tarball(prefix, version, &staged)?,
        Install::Arch | Install::Unknown => unreachable!("action() refuses these"),
    }
    Ok(())
}

pub fn restart(plan: &Restart) -> io::Result<()> {
    use crate::client;

    let executable = plan.app.with_file_name("otozaned");
    let running = client::running_service_matches(&executable)?;
    if running == Some(false) {
        plan.stop()?;
    }
    // A retry can reuse the replacement even if Stop left it holding the socket.
    if running != Some(true) || !client::resume_service()? {
        match plan.service {
            Service::Systemd => client::systemctl(&["restart", "otozane.service"])?,
            Service::Detached => client::start_detached_from(&executable)?,
        }
    }
    client::wait_for_service(|peer, _| {
        if client::service_matches_executable(peer, &executable)? {
            Ok(())
        } else {
            Err(io::Error::other(
                "The running service still uses a different executable",
            ))
        }
    })?;
    let mut app = Command::new(&plan.app).spawn()?;
    std::thread::spawn(move || {
        let _ = app.wait();
    });
    Ok(())
}

fn release_version(version: &str) -> Result<&str, Error> {
    let version = version.strip_prefix('v').unwrap_or(version);
    let mut parts = version.split('.');
    let three = (0..3).all(|_| {
        parts
            .next()
            .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    });
    if three && parts.next().is_none() {
        Ok(version)
    } else {
        Err(Error::Version(version.to_owned()))
    }
}

fn host_arch() -> Result<(&'static str, &'static str), Error> {
    match std::env::consts::ARCH {
        "x86_64" => Ok(("x86_64", "amd64")),
        "aarch64" => Ok(("aarch64", "arm64")),
        other => Err(Error::Arch(other.to_owned())),
    }
}

fn dpkg_owns(root: &Path, exe: &Path) -> bool {
    fs::read_to_string(root.join("var/lib/dpkg/info/otozane.list")).is_ok_and(|list| {
        list.lines()
            .any(|line| root.join(line.trim().trim_start_matches('/')) == exe)
    })
}

fn pacman_owns(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root.join("var/lib/pacman/local")) else {
        return false;
    };
    entries
        .filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().starts_with("otozane-"))
}

fn rpm_owns(exe: &Path) -> bool {
    Command::new("rpm")
        .args(["-qf", "--queryformat", "%{NAME}"])
        .arg(exe)
        .output()
        .is_ok_and(|out| {
            out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "otozane"
        })
}

fn install_rpm(staged: &Path) -> Result<(), Error> {
    if find_in_path("dnf").is_some() {
        run_root("dnf", &["install", "-y"], staged)
    } else if find_in_path("zypper").is_some() {
        run_root(
            "zypper",
            &["--non-interactive", "install", "--allow-unsigned-rpm"],
            staged,
        )
    } else {
        Err(Error::Command {
            command: "dnf or zypper".to_owned(),
            detail: "neither is in PATH".to_owned(),
        })
    }
}

fn run_root(program: &str, args: &[&str], staged: &Path) -> Result<(), Error> {
    let command = format!("pkexec {program}");
    let out = Command::new("pkexec")
        .arg(program)
        .args(args)
        .arg(staged)
        .output()
        .map_err(|e| Error::Command {
            command: command.clone(),
            detail: e.to_string(),
        })?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim().to_owned()
    } else {
        stderr.trim().to_owned()
    };
    let detail = format!("exit {}: {detail}", out.status);
    let (state, recovery) = if program == "apt-get" {
        (
            Command::new("dpkg-query")
                .args(["-W", "-f=${Status} ${Version}", "otozane"])
                .output(),
            "If Otozane is unpacked or half-configured, run sudo dpkg --configure otozane, then retry the update.",
        )
    } else {
        (
            Command::new("rpm").args(["-q", "otozane"]).output(),
            "Review the package manager error, then repair or reinstall the Otozane RPM with your package manager.",
        )
    };
    let state = state
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .unwrap_or_else(|| "could not verify the installed package".into());
    Err(Error::Package {
        command,
        detail,
        state,
        recovery,
    })
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|path| {
            fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".config")))
}

fn user_unit_exists() -> bool {
    user_unit_exists_in(config_home().as_deref())
}

fn user_unit_exists_in(config_home: Option<&Path>) -> bool {
    config_home.is_some_and(|dir| dir.join("systemd/user/otozane.service").is_file())
}

fn install_tarball(prefix: &Path, version: &str, staged: &Path) -> Result<(), Error> {
    let (arch, _) = host_arch()?;
    if !prefix.is_absolute() || prefix == Path::new("/") {
        return Err(Error::Layout(format!(
            "refusing to install into {}",
            prefix.display()
        )));
    }
    let work = tempfile::Builder::new()
        .prefix(".otozane-update-")
        .tempdir_in(prefix)?;
    let extracted = Command::new("tar")
        .arg("-xzf")
        .arg(staged)
        .arg("-C")
        .arg(work.path())
        .status()?;
    if !extracted.success() {
        return Err(Error::Command {
            command: format!("tar -xzf {}", staged.display()),
            detail: "extraction failed".to_owned(),
        });
    }
    let src = work.path().join(format!("otozane-v{version}-linux-{arch}"));
    let result = install_payload(prefix, &src, work.path());
    if matches!(result, Err(Error::Rollback { .. })) {
        let _ = work.keep();
    }
    result
}

fn install_payload(prefix: &Path, src: &Path, work: &Path) -> Result<(), Error> {
    check_layout(src)?;
    let stage = work.join("stage");
    copy_tree(&src.join("usr"), &stage.join("usr"))?;
    let desktop = stage.join("usr/share/applications/otozane.desktop");
    rewrite_desktop(&desktop, prefix)?;

    let files = [
        ("usr/lib/otozane", "lib/otozane"),
        ("usr/bin/otozane", "bin/otozane"),
        ("usr/bin/otozaned", "bin/otozaned"),
        (
            "usr/share/applications/otozane.desktop",
            "share/applications/otozane.desktop",
        ),
        (
            "usr/share/icons/hicolor/scalable/apps/otozane.svg",
            "share/icons/hicolor/scalable/apps/otozane.svg",
        ),
        ("usr/share/licenses/otozane", "share/licenses/otozane"),
        ("usr/share/doc/otozane", "share/doc/otozane"),
    ];
    let mut swap = Swap::new(work.join("backup"));
    for (from, to) in files {
        if let Err(e) = swap.put(&stage.join(from), &prefix.join(to)) {
            return match swap.rollback() {
                Ok(()) => Err(e.into()),
                Err(restore) => Err(Error::Rollback {
                    install: e,
                    restore,
                    work: work.to_path_buf(),
                }),
            };
        }
    }
    Ok(())
}

fn check_layout(src: &Path) -> Result<(), Error> {
    let required = [
        "usr/bin/otozane",
        "usr/bin/otozaned",
        "usr/lib/otozane",
        "usr/share/applications/otozane.desktop",
        "usr/share/icons/hicolor/scalable/apps/otozane.svg",
        "usr/share/licenses/otozane",
        "usr/share/doc/otozane",
    ];
    for rel in required {
        if !src.join(rel).exists() {
            return Err(Error::Layout(format!("{rel} is missing from the archive")));
        }
    }
    for rel in ["usr/bin/otozane", "usr/bin/otozaned"] {
        let executable = src
            .join(rel)
            .metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0);
        if !executable {
            return Err(Error::Layout(format!("{rel} is not executable")));
        }
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), Error> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    let command = format!("cp -a {}", from.display());
    let out = Command::new("cp").arg("-a").arg(from).arg(to).output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Error::Command {
            command,
            detail: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}

fn rewrite_desktop(path: &Path, prefix: &Path) -> io::Result<()> {
    let text = fs::read_to_string(path)?;
    let exec = format!("Exec={}/bin/otozane", prefix.display());
    let mut out = String::new();
    for line in text.lines() {
        if line.starts_with("Exec=") {
            out.push_str(&exec);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    fs::write(path, out)
}

/// Renames files into place, remembering the old ones to undo a partial install.
struct Swap {
    backup: PathBuf,
    next: usize,
    installed: Vec<PathBuf>,
    moved: Vec<(PathBuf, PathBuf)>,
}

impl Swap {
    fn new(backup: PathBuf) -> Self {
        Self {
            backup,
            next: 0,
            installed: Vec::new(),
            moved: Vec::new(),
        }
    }

    fn put(&mut self, staged: &Path, live: &Path) -> io::Result<()> {
        if let Some(parent) = live.parent() {
            fs::create_dir_all(parent)?;
        }
        let backup = self.backup.join(self.next.to_string());
        self.next += 1;
        match fs::symlink_metadata(live) {
            Ok(_) => {
                fs::create_dir_all(&self.backup)?;
                fs::rename(live, &backup)?;
                self.moved.push((live.to_path_buf(), backup));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        fs::rename(staged, live)?;
        self.installed.push(live.to_path_buf());
        Ok(())
    }

    fn rollback(&mut self) -> io::Result<()> {
        let mut result = Ok(());
        for live in self.installed.drain(..).rev() {
            result = result.and(remove_any(&live));
        }
        for (live, backup) in self.moved.drain(..).rev() {
            result = result.and(fs::rename(&backup, &live).map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("could not restore {}: {e}", live.display()),
                )
            }));
        }
        result
    }
}

fn remove_any(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "otozane-update-{}-{tag}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn host_asset_arch() -> Option<(&'static str, &'static str)> {
        match std::env::consts::ARCH {
            "x86_64" => Some(("x86_64", "amd64")),
            "aarch64" => Some(("aarch64", "arm64")),
            _ => None,
        }
    }

    fn fake_payload(src: &Path) {
        for dir in [
            "usr/bin",
            "usr/lib/otozane",
            "usr/share/applications",
            "usr/share/icons/hicolor/scalable/apps",
            "usr/share/licenses/otozane",
            "usr/share/doc/otozane",
        ] {
            fs::create_dir_all(src.join(dir)).unwrap();
        }
        fs::write(src.join("usr/bin/otozane"), "new-app").unwrap();
        fs::write(src.join("usr/bin/otozaned"), "new-service").unwrap();
        for bin in ["usr/bin/otozane", "usr/bin/otozaned"] {
            fs::set_permissions(src.join(bin), fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(src.join("usr/lib/otozane/new-marker"), "new").unwrap();
        fs::write(
            src.join("usr/share/applications/otozane.desktop"),
            "[Desktop Entry]\nName=Otozane\nExec=otozane\n",
        )
        .unwrap();
        fs::write(
            src.join("usr/share/icons/hicolor/scalable/apps/otozane.svg"),
            "<svg/>",
        )
        .unwrap();
        fs::write(src.join("usr/share/licenses/otozane/LICENSE"), "MIT").unwrap();
        fs::write(src.join("usr/share/doc/otozane/README.md"), "docs").unwrap();
    }

    fn old_prefix(prefix: &Path) {
        fs::create_dir_all(prefix.join("bin")).unwrap();
        fs::create_dir_all(prefix.join("lib/otozane")).unwrap();
        fs::write(prefix.join("bin/otozane"), "old-app").unwrap();
        fs::write(prefix.join("bin/otozaned"), "old-service").unwrap();
        fs::write(prefix.join("lib/otozane/old-marker"), "old").unwrap();
    }

    fn fake_work(t: &Temp) -> PathBuf {
        let work = t.path().join("work");
        fs::create_dir_all(&work).unwrap();
        work
    }

    #[test]
    fn assets_match_release_artifact_names() {
        let Some((arch, deb_arch)) = host_asset_arch() else {
            return;
        };
        let cases = [
            (Install::Deb, format!("otozane_0.6.0-1_{deb_arch}.deb")),
            (Install::Rpm, format!("otozane-0.6.0-1.{arch}.rpm")),
            (
                Install::User {
                    prefix: PathBuf::from("/tmp/prefix"),
                },
                format!("otozane-v0.6.0-linux-{arch}.tar.gz"),
            ),
        ];
        for (install, asset) in cases {
            assert_eq!(install.action("0.6.0").unwrap(), Action::Install { asset });
            assert_eq!(
                install.action("v0.6.0").unwrap(),
                install.action("0.6.0").unwrap()
            );
        }
        for install in [Install::Arch, Install::Unknown] {
            assert!(matches!(
                install.action("0.6.0").unwrap(),
                Action::Notify { .. }
            ));
        }
    }

    #[test]
    fn rejects_non_release_versions() {
        for bad in [
            "",
            "0.6",
            "0.6.0-rc1",
            "v",
            "../0.6.0",
            "0.6.0/../x",
            "0.6.0\n",
        ] {
            assert!(
                matches!(Install::Deb.action(bad), Err(Error::Version(_))),
                "{bad:?} was accepted"
            );
        }
    }

    #[test]
    fn detects_install_from_a_fake_root() {
        let root = Temp::new("detect");
        let exe = root.path().join("usr/bin/otozane");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        assert_eq!(Install::detect_at(root.path(), &exe), Install::Unknown);

        fs::create_dir_all(root.path().join("var/lib/dpkg/info")).unwrap();
        fs::write(
            root.path().join("var/lib/dpkg/info/otozane.list"),
            "/usr/bin/otozane\n",
        )
        .unwrap();
        assert_eq!(Install::detect_at(root.path(), &exe), Install::Deb);

        fs::create_dir_all(root.path().join("var/lib/pacman/local/otozane-bin-0.5.0-1")).unwrap();
        assert_eq!(Install::detect_at(root.path(), &exe), Install::Arch);
    }

    #[test]
    fn detects_a_user_prefix_install() {
        let root = Temp::new("detect-user");
        let prefix = root.path().join("home/u/.local");
        fs::create_dir_all(prefix.join("bin")).unwrap();
        fs::write(prefix.join("bin/otozane"), "").unwrap();
        fs::create_dir_all(prefix.join("lib/otozane")).unwrap();
        let exe = prefix.join("bin/otozane");
        assert_eq!(
            Install::detect_at(root.path(), &exe),
            Install::User { prefix }
        );
    }

    #[test]
    fn unowned_usr_install_stays_unknown() {
        let root = Temp::new("detect-usr");
        let exe = root.path().join("usr/bin/otozane");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::create_dir_all(root.path().join("usr/lib/otozane")).unwrap();
        assert_eq!(Install::detect_at(root.path(), &exe), Install::Unknown);
    }

    #[test]
    fn tarball_payload_replaces_files_and_points_exec_at_the_prefix() {
        let t = Temp::new("payload");
        let prefix = t.path().join("prefix");
        old_prefix(&prefix);
        let src = t.path().join("src");
        fake_payload(&src);

        install_payload(&prefix, &src, &fake_work(&t)).unwrap();

        assert_eq!(
            fs::read_to_string(prefix.join("bin/otozane")).unwrap(),
            "new-app"
        );
        assert!(prefix.join("lib/otozane/new-marker").exists());
        assert!(!prefix.join("lib/otozane/old-marker").exists());
        let desktop =
            fs::read_to_string(prefix.join("share/applications/otozane.desktop")).unwrap();
        assert!(desktop.contains(&format!("Exec={}/bin/otozane\n", prefix.display())));
    }

    #[test]
    fn tarball_failure_restores_the_previous_install() {
        let t = Temp::new("rollback");
        let prefix = t.path().join("prefix");
        fs::create_dir_all(prefix.join("lib/otozane")).unwrap();
        fs::write(prefix.join("lib/otozane/old-marker"), "old").unwrap();
        // A regular file where the bin directory belongs makes the first
        // binary rename fail after the library has already been swapped.
        fs::write(prefix.join("bin"), "not a directory").unwrap();
        let src = t.path().join("src");
        fake_payload(&src);

        assert!(install_payload(&prefix, &src, &fake_work(&t)).is_err());

        assert_eq!(
            fs::read_to_string(prefix.join("lib/otozane/old-marker")).unwrap(),
            "old"
        );
        assert!(!prefix.join("lib/otozane/new-marker").exists());
    }

    #[test]
    fn a_failed_restore_keeps_the_old_file_in_the_backup() {
        let t = Temp::new("rollback-backup");
        let live = t.path().join("live");
        let backup = t.path().join("backup");
        fs::write(&live, "old").unwrap();
        let mut swap = Swap::new(backup.clone());
        assert!(swap.put(&t.path().join("missing"), &live).is_err());

        fs::create_dir(&live).unwrap();
        assert!(swap.rollback().is_err());
        assert_eq!(fs::read_to_string(backup.join("0")).unwrap(), "old");
    }

    #[test]
    fn installs_a_verified_user_asset() {
        let Some((arch, _)) = host_asset_arch() else {
            return;
        };
        let t = Temp::new("extract");
        let prefix = t.path().join("prefix");
        old_prefix(&prefix);
        let version = "0.6.0";
        let name = format!("otozane-v{version}-linux-{arch}");
        let unpacked = t.path().join("unpacked");
        fake_payload(&unpacked.join(&name));
        let archive = t.path().join(format!("{name}.tar.gz"));
        assert!(
            Command::new("tar")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(&unpacked)
                .arg(&name)
                .status()
                .unwrap()
                .success()
        );

        install(
            &Install::User {
                prefix: prefix.clone(),
            },
            version,
            &archive,
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(prefix.join("bin/otozaned")).unwrap(),
            "new-service"
        );
    }

    #[test]
    fn refuses_an_asset_that_is_not_the_expected_one() {
        if host_asset_arch().is_none() {
            return;
        }
        let t = Temp::new("wrong");
        let staged = t.path().join("otozane-0.6.0-1-x86_64.rpm");
        fs::write(&staged, "not a package").unwrap();
        assert!(matches!(
            install(&Install::Deb, "0.6.0", &staged),
            Err(Error::WrongAsset { .. })
        ));
    }

    #[test]
    fn arch_and_unknown_installs_only_notify() {
        let t = Temp::new("notify");
        let staged = t.path().join("PKGBUILD");
        fs::write(&staged, "").unwrap();
        for install in [Install::Arch, Install::Unknown] {
            assert!(matches!(install.action("0.6.0"), Ok(Action::Notify { .. })));
            assert!(matches!(
                super::install(&install, "0.6.0", &staged),
                Err(Error::NotInstallable(_))
            ));
        }
    }

    #[test]
    fn restart_uses_the_unit_only_when_it_exists() {
        let t = Temp::new("unit");
        assert!(!user_unit_exists_in(Some(t.path())));
        fs::create_dir_all(t.path().join("systemd/user")).unwrap();
        fs::write(t.path().join("systemd/user/otozane.service"), "").unwrap();
        assert!(user_unit_exists_in(Some(t.path())));
    }

    #[test]
    fn a_payload_with_non_executable_binaries_is_refused() {
        let t = Temp::new("exec");
        let prefix = t.path().join("prefix");
        old_prefix(&prefix);
        let src = t.path().join("src");
        fake_payload(&src);
        fs::set_permissions(
            src.join("usr/bin/otozane"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(matches!(
            install_payload(&prefix, &src, &fake_work(&t)),
            Err(Error::Layout(_))
        ));
        assert_eq!(
            fs::read_to_string(prefix.join("bin/otozane")).unwrap(),
            "old-app"
        );
    }
}
