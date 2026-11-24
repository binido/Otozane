//! In-app updates of the macOS app bundle.
//!
//! The updater downloads the release asset and checks it against `SHA256SUMS`
//! before calling [`stage`]; this module trusts that check. It mounts the DMG,
//! validates the app inside (identity, version, architecture, code signature),
//! copies it out and prepares the replacement. [`Staged::restart`] then hands
//! the swap to a small shell helper that waits for this process to exit,
//! replaces the bundle, restores the old app if anything fails, restarts the
//! launchd agent and opens Otozane again.
//!
//! The app is signed ad-hoc and not notarized. After the checksum and the
//! bundle checks the quarantine flag is removed from the staged app, otherwise
//! Gatekeeper would block the first launch. The clean-Mac checklist in
//! `DEVELOPMENT.md` covers what still needs a manual check.
//!
//! The helper's log is `last-update.log` in the updates folder below the
//! config directory, next to the backup it removes after a successful launch.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const BUNDLE_ID: &str = "io.github.binido.otozane";

/// The installed app, as detected from the running executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    app: PathBuf,
    version: String,
}

impl Bundle {
    /// `/Applications/Otozane.app`.
    pub fn app(&self) -> &Path {
        &self.app
    }

    /// `CFBundleShortVersionString` of the installed app.
    pub fn version(&self) -> &str {
        &self.version
    }
}

/// Where this process runs from, when it can replace its own app bundle.
///
/// Unsupported installations get a reason the Updates section can show next
/// to a link to the release page.
pub fn detect() -> Result<Bundle, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Could not find the app: {e}"))?;
    detect_in(&exe)
}

/// [`detect`] for an explicit executable, so tests stay out of `/Applications`.
pub fn detect_in(exe: &Path) -> Result<Bundle, String> {
    let macos = exe
        .parent()
        .ok_or("Otozane is not running from an app bundle.")?;
    let contents = macos
        .parent()
        .ok_or("Otozane is not running from an app bundle.")?;
    let app = contents
        .parent()
        .ok_or("Otozane is not running from an app bundle.")?;
    if macos.file_name() != Some(OsStr::new("MacOS"))
        || contents.file_name() != Some(OsStr::new("Contents"))
        || app.extension() != Some(OsStr::new("app"))
    {
        return Err(
            "Otozane is not running from an app bundle. In-app updates need Otozane.app.".into(),
        );
    }
    // Gatekeeper runs a quarantined app from a read-only, throwaway copy.
    if app.to_string_lossy().contains("/AppTranslocation/") {
        return Err(
            "macOS is running Otozane from a temporary copy. Move Otozane to Applications and open it again."
                .into(),
        );
    }
    if bundle_id(app).as_deref() != Some(BUNDLE_ID) {
        return Err("This is not the Otozane app bundle.".into());
    }
    let version =
        app_version(app).ok_or("Could not read the Otozane version from the app bundle.")?;
    let parent = app
        .parent()
        .ok_or("The app has no folder to update it in.")?;
    if !dir_writable(parent) {
        return Err(format!(
            "{} is not writable. Move Otozane to Applications, or update from the release page.",
            parent.display()
        ));
    }
    Ok(Bundle {
        app: app.to_path_buf(),
        version,
    })
}

/// Release asset for a version, e.g. `otozane-v0.6.0-macos-arm64.dmg`.
///
/// Returns an error on Intel Macs: releases are Apple Silicon only.
pub fn asset_name(version: &str) -> Result<String, String> {
    asset_for(version, std::env::consts::ARCH)
}

fn asset_for(version: &str, arch: &str) -> Result<String, String> {
    let v = version.trim().strip_prefix('v').unwrap_or(version.trim());
    if version_key(v).is_none() {
        return Err(format!("Not a version: {version}"));
    }
    if arch != "aarch64" {
        return Err("In-app updates are only available for Apple Silicon.".into());
    }
    Ok(format!("otozane-v{v}-macos-arm64.dmg"))
}

/// The virtual microphone driver shipped with this app versus the installed one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverState {
    /// `CFBundleVersion` of the driver inside this app bundle.
    pub bundled_version: Option<String>,
    /// `CFBundleVersion` of the driver in `/Library/Audio/Plug-Ins/HAL`.
    pub installed_version: Option<String>,
    /// The installed driver is from a different build; offer a reinstall
    /// through [`crate::driver`] after the update.
    pub needs_reinstall: bool,
}

/// Compares the bundled driver with the installed one. A missing installed
/// driver is not reported here, Settings already offers the first install.
pub fn driver_state() -> DriverState {
    compare_drivers(
        crate::driver::bundled().as_deref(),
        Some(Path::new(crate::driver::INSTALLED)),
    )
}

fn existing(path: Option<&Path>) -> Option<&Path> {
    path.filter(|p| p.is_dir())
}

fn compare_drivers(bundled: Option<&Path>, installed: Option<&Path>) -> DriverState {
    let bundled = existing(bundled);
    let installed = existing(installed);
    let executable = |p: &Path| std::fs::read(p.join("Contents/MacOS/Otozane")).ok();
    let needs_reinstall = match (bundled, installed) {
        (Some(b), Some(i)) => match (executable(b), executable(i)) {
            (Some(b), Some(i)) => b != i,
            _ => false,
        },
        _ => false,
    };
    DriverState {
        bundled_version: bundled.and_then(driver_version),
        installed_version: installed.and_then(driver_version),
        needs_reinstall,
    }
}

fn driver_version(driver: &Path) -> Option<String> {
    let text = std::fs::read_to_string(driver.join("Contents/Info.plist")).ok()?;
    plist_value(&text, "CFBundleVersion")
}

/// A validated app copy waiting for the current process to quit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    version: String,
    app: PathBuf,
    target: PathBuf,
}

impl Staged {
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Starts the helper that swaps the bundle once this process exits.
    ///
    /// The caller must ask the service to shut down (`Request::Shutdown`) and
    /// close the window after this returns; the helper waits up to a minute
    /// for the process to go away, then replaces the app, starts the service
    /// again (through the launchd agent when installed) and opens Otozane.
    /// Nothing is replaced until the UI has exited, and the old app is put
    /// back when a step fails.
    pub fn restart(self) -> Result<(), String> {
        let updates = self
            .app
            .parent()
            .ok_or("The staged app has no folder to update from.")?;
        let pid = std::process::id();
        let backup = updates.join(format!("Otozane-backup-{pid}.app"));
        let log = updates.join("last-update.log");
        let script = updates.join(format!("restart-{pid}.sh"));
        let text = restart_script(
            pid,
            &self.target,
            &self.app,
            &backup,
            &log,
            crate::launchd::plist_path().ok().as_deref(),
        );
        std::fs::write(&script, text)
            .map_err(|e| format!("Could not write the update helper: {e}"))?;
        let mut cmd = Command::new("/bin/sh");
        cmd.arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        use std::os::unix::process::CommandExt;
        // Own process group, so the helper outlives the app without a terminal.
        cmd.process_group(0);
        cmd.spawn()
            .map_err(|e| format!("Could not start the update helper: {e}"))?;
        Ok(())
    }
}

/// Mounts `dmg`, checks the app inside and copies it next to the updates
/// folder. The installed app is not touched; an error leaves it as it was.
pub fn stage(dmg: &Path, bundle: &Bundle, version: &str) -> Result<Staged, String> {
    stage_into(dmg, bundle, version, &update_dir())
}

fn stage_into(
    dmg: &Path,
    bundle: &Bundle,
    version: &str,
    updates: &Path,
) -> Result<Staged, String> {
    let expected = version.trim().strip_prefix('v').unwrap_or(version.trim());
    let new = version_key(expected).ok_or_else(|| format!("Not a version: {version}"))?;
    let current = version_key(&bundle.version)
        .ok_or_else(|| format!("The installed version {} is not readable", bundle.version))?;
    if new <= current {
        return Err(format!(
            "Otozane {expected} is not newer than the installed {}.",
            bundle.version
        ));
    }
    std::fs::create_dir_all(updates)
        .map_err(|e| format!("Could not create {}: {e}", updates.display()))?;
    let app = updates.join(format!("Otozane-{expected}.app"));
    let _ = std::fs::remove_dir_all(&app);

    let result = (|| {
        let mount = TempDir::new("otozane-update-dmg")?;
        {
            let _mounted = Mount::attach(dmg, mount.path())?;
            copy_app(&only_app(mount.path())?, &app)?;
        }
        validate_app(&app, expected)?;
        strip_quarantine(&app);
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&app);
        return Err(e);
    }
    Ok(Staged {
        version: expected.to_string(),
        app,
        target: bundle.app.clone(),
    })
}

fn update_dir() -> PathBuf {
    otozane_core::config::config_dir().join("updates")
}

fn only_app(mount: &Path) -> Result<PathBuf, String> {
    let mut apps = Vec::new();
    for entry in std::fs::read_dir(mount)
        .map_err(|e| format!("Could not read the DMG: {e}"))?
        .flatten()
    {
        let path = entry.path();
        if path.extension() == Some(OsStr::new("app")) {
            apps.push(path);
        }
    }
    match apps.len() {
        1 => Ok(apps.remove(0)),
        0 => Err("The DMG has no app in it.".into()),
        _ => Err("The DMG has more than one app in it.".into()),
    }
}

fn copy_app(src: &Path, dst: &Path) -> Result<(), String> {
    let mut cmd = Command::new("ditto");
    cmd.arg("--norsrc").arg(src).arg(dst);
    run(&mut cmd, "copy the app out of the DMG")?;
    Ok(())
}

fn validate_app(app: &Path, expected: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(app.join("Contents/Info.plist"))
        .map_err(|e| format!("The new app has no Info.plist: {e}"))?;
    if plist_value(&text, "CFBundleIdentifier").as_deref() != Some(BUNDLE_ID) {
        return Err("The DMG contains a different app.".into());
    }
    let version = plist_value(&text, "CFBundleShortVersionString")
        .ok_or("The new app has no version in its Info.plist.")?;
    if version_key(&version) != version_key(expected) {
        return Err(format!(
            "The DMG contains Otozane {version}, expected {expected}."
        ));
    }
    let exe_name = plist_value(&text, "CFBundleExecutable")
        .ok_or("The new app has no executable in its Info.plist.")?;
    let exe = app.join("Contents/MacOS").join(exe_name);
    let meta =
        std::fs::metadata(&exe).map_err(|e| format!("The new app has no executable: {e}"))?;
    if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
        return Err("The new app's executable is not runnable.".into());
    }
    if !is_arm64(&exe) {
        return Err("The new app is not an Apple Silicon build.".into());
    }
    let mut cmd = Command::new("codesign");
    cmd.args(["--verify", "--deep", "--strict"]).arg(app);
    run(&mut cmd, "verify the new app's signature")?;
    Ok(())
}

/// The app is ad-hoc signed, so Gatekeeper cannot assess it. Trust comes from
/// the release checksum; without this flag the update would not open.
fn strip_quarantine(app: &Path) {
    let _ = Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(app)
        .status();
}

fn is_arm64(exe: &Path) -> bool {
    let Ok(out) = Command::new("lipo").args(["-archs"]).arg(exe).output() else {
        return false;
    };
    out.status.success()
        && String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .any(|arch| arch.starts_with("arm64"))
}

fn run(cmd: &mut Command, what: &str) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("Could not {what}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "Could not {what}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Mounts the DMG read-only and detaches it again when dropped.
struct Mount {
    point: PathBuf,
}

impl Mount {
    fn attach(dmg: &Path, point: &Path) -> Result<Self, String> {
        let mut cmd = Command::new("hdiutil");
        cmd.args([
            "attach",
            "-readonly",
            "-nobrowse",
            "-noautoopen",
            "-mountpoint",
        ])
        .arg(point)
        .arg(dmg);
        run(&mut cmd, "open the update DMG")?;
        Ok(Self {
            point: point.to_path_buf(),
        })
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil")
            .args(["detach", "-force"])
            .arg(&self.point)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Result<Self, String> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!("{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&path)
            .map_err(|e| format!("Could not create a temporary folder: {e}"))?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn bundle_id(app: &Path) -> Option<String> {
    let text = std::fs::read_to_string(app.join("Contents/Info.plist")).ok()?;
    plist_value(&text, "CFBundleIdentifier")
}

fn app_version(app: &Path) -> Option<String> {
    let text = std::fs::read_to_string(app.join("Contents/Info.plist")).ok()?;
    plist_value(&text, "CFBundleShortVersionString")
}

/// Reads a `<key>NAME</key><string>VALUE</string>` pair from an XML plist.
/// Info.plist values used here are plain strings, a full parser is not needed.
fn plist_value(text: &str, key: &str) -> Option<String> {
    let open = format!("<key>{key}</key>");
    let rest = &text[text.find(&open)? + open.len()..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = start + rest[start..].find("</string>")?;
    Some(rest[start..end].trim().to_string())
}

fn version_key(version: &str) -> Option<[u64; 3]> {
    let version = version.trim().strip_prefix('v').unwrap_or(version.trim());
    let mut parts = version.split('.').map(|p| p.parse::<u64>().ok());
    let a = parts.next()??;
    let b = parts.next()??;
    let c = parts.next()??;
    parts.next().is_none().then_some([a, b, c])
}

/// Whether this user can create a file in `dir`. A temporary probe is the
/// only reliable check: ownership and group write rules differ per machine.
fn dir_writable(dir: &Path) -> bool {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let probe = dir.join(format!(".otozane-update-{}-{nanos}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// The helper script. It is a plain shell script on purpose: the app it
/// replaces contains the binary that wrote it, so it must not be busy.
fn restart_script(
    pid: u32,
    target: &Path,
    staged: &Path,
    backup: &Path,
    log: &Path,
    plist: Option<&Path>,
) -> String {
    let quote = |p: &Path| crate::driver::shell_quote(&p.to_string_lossy());
    format!(
        r#"#!/bin/sh
# Otozane update helper: waits for the UI to quit, then swaps the app bundle.
exec >>{} 2>&1
pid={pid}
if [ "$pid" -gt 0 ]; then
    n=0
    while kill -0 "$pid" 2>/dev/null; do
        n=$((n + 1))
        if [ "$n" -ge 300 ]; then
            echo "update: the app did not quit, nothing changed"
            exit 1
        fi
        sleep 0.2
    done
fi
target={}
staged={}
backup={}
plist={}
echo "update: replacing $target"
rm -rf "$backup"
if ! mv "$target" "$backup"; then
    echo "update: could not move the current app aside"
    exit 1
fi
if ! mv "$staged" "$target"; then
    echo "update: could not install the new app, restoring"
    mv "$backup" "$target"
    exit 1
fi
# Starts the service from the replaced bundle, through the login agent when
# one is installed, otherwise detached like a first start without it.
if [ -f "$plist" ]; then
    launchctl bootstrap "gui/$(id -u)" "$plist" >/dev/null 2>&1 ||
        launchctl kickstart -k "gui/$(id -u)/{label}" >/dev/null 2>&1 || true
elif [ -x "$target/Contents/MacOS/otozaned" ]; then
    (cd / && nohup "$target/Contents/MacOS/otozaned" >/dev/null 2>&1 &)
fi
if ! open "$target"; then
    echo "update: could not open the new app, restoring"
    rm -rf "$target"
    mv "$backup" "$target"
    open "$target"
    exit 1
fi
rm -rf "$backup"
rm -f "$0"
echo "update: done"
"#,
        quote(log),
        quote(target),
        quote(staged),
        quote(backup),
        plist.map_or_else(|| "''".to_string(), quote),
        label = crate::launchd::LABEL,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn app_fixture(root: &Path, version: &str) -> PathBuf {
        let app = root.join("Otozane.app");
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::write(
            app.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>
<key>CFBundleExecutable</key><string>otozane</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
</dict></plist>
"#
            ),
        )
        .unwrap();
        // A real Mach-O, so codesign and lipo have something to work with.
        fs::copy("/bin/echo", app.join("Contents/MacOS/otozane")).unwrap();
        app
    }

    fn sign(app: &Path) {
        assert!(
            Command::new("codesign")
                .args(["--force", "--sign", "-"])
                .arg(app)
                .status()
                .unwrap()
                .success()
        );
    }

    fn executable(app: &Path) -> PathBuf {
        app.join("Contents/MacOS/otozane")
    }

    #[test]
    fn versions_are_three_numbers_and_ordered() {
        assert_eq!(version_key("v0.6.0"), Some([0, 6, 0]));
        assert_eq!(version_key(" 1.2.10 "), Some([1, 2, 10]));
        assert!(version_key("1.2").is_none());
        assert!(version_key("1.2.3.4").is_none());
        assert!(version_key("1.2.x").is_none());
        assert!(version_key("1.2.10").unwrap() > version_key("1.2.9").unwrap());
    }

    #[test]
    fn plist_reads_only_string_values() {
        let text = "<key>CFBundleVersion</key><string>0.6.0</string>";
        assert_eq!(
            plist_value(text, "CFBundleVersion").as_deref(),
            Some("0.6.0")
        );
        assert_eq!(plist_value(text, "Missing"), None);
    }

    #[test]
    fn asset_name_follows_the_release() {
        assert_eq!(
            asset_for("v0.6.0", "aarch64").unwrap(),
            "otozane-v0.6.0-macos-arm64.dmg"
        );
        assert!(asset_for("0.6", "aarch64").is_err());
        assert!(asset_for("0.6.0", "x86_64").is_err());
    }

    #[test]
    fn detects_a_bundle_only_where_it_can_be_replaced() {
        let root = TempDir::new("otozane-detect").unwrap();
        let app = app_fixture(root.path(), "0.5.0");
        let bundle = detect_in(&executable(&app)).unwrap();
        assert_eq!(bundle.app(), app);
        assert_eq!(bundle.version(), "0.5.0");

        let loose = root.path().join("otozane");
        fs::write(&loose, "").unwrap();
        assert!(detect_in(&loose).is_err());

        let translated = app_fixture(&root.path().join("AppTranslocation/x"), "0.5.0");
        let err = detect_in(&executable(&translated)).unwrap_err();
        assert!(err.contains("temporary"), "{err}");

        let locked = root.path().join("locked");
        let locked_app = app_fixture(&locked, "0.5.0");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let err = detect_in(&executable(&locked_app)).unwrap_err();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(err.contains("not writable"), "{err}");
    }

    #[test]
    fn driver_difference_is_reported_for_another_build() {
        let root = TempDir::new("otozane-driver").unwrap();
        let bundled = root.path().join("bundled.driver");
        let installed = root.path().join("installed.driver");
        for (dir, version, body) in [(&bundled, "0.6.0", "new"), (&installed, "0.5.0", "old")] {
            fs::create_dir_all(dir.join("Contents/MacOS")).unwrap();
            fs::write(
                dir.join("Contents/Info.plist"),
                format!("<key>CFBundleVersion</key><string>{version}</string>"),
            )
            .unwrap();
            fs::write(dir.join("Contents/MacOS/Otozane"), body).unwrap();
        }
        let state = compare_drivers(Some(&bundled), Some(&installed));
        assert_eq!(state.bundled_version.as_deref(), Some("0.6.0"));
        assert_eq!(state.installed_version.as_deref(), Some("0.5.0"));
        assert!(state.needs_reinstall);

        let same = compare_drivers(Some(&bundled), Some(&bundled));
        assert!(!same.needs_reinstall);
        let missing = compare_drivers(Some(&bundled), Some(&root.path().join("absent")));
        assert!(!missing.needs_reinstall);
        assert_eq!(missing.installed_version, None);
    }

    /// Builds a signed app and a DMG from it, then stages it in a sandbox.
    #[test]
    fn stages_a_signed_dmg_and_rejects_the_wrong_version() {
        let root = TempDir::new("otozane-stage").unwrap();
        let src = TempDir::new("otozane-stage-src").unwrap();
        let app = app_fixture(src.path(), "0.6.0");
        sign(&app);
        let dmg = root.path().join("otozane-v0.6.0-macos-arm64.dmg");
        let status = Command::new("hdiutil")
            .args([
                "create", "-quiet", "-volname", "Otozane", "-fs", "HFS+", "-format", "UDZO",
            ])
            .arg("-srcfolder")
            .arg(src.path())
            .arg(&dmg)
            .status()
            .unwrap();
        assert!(status.success(), "could not create the test DMG");

        let target_root = root.path().join("Applications");
        let target = app_fixture(&target_root, "0.5.0");
        let installed = detect_in(&executable(&target)).unwrap();
        let updates = root.path().join("updates");
        let staged = stage_into(&dmg, &installed, "v0.6.0", &updates).unwrap();
        assert_eq!(staged.version(), "0.6.0");
        assert!(
            updates
                .join("Otozane-0.6.0.app/Contents/MacOS/otozane")
                .exists()
        );
        assert!(target.join("Contents/MacOS/otozane").exists());

        let err = stage_into(&dmg, &installed, "0.7.0", &updates).unwrap_err();
        assert!(err.contains("expected 0.7.0"), "{err}");
        assert!(!updates.join("Otozane-0.7.0.app").exists());

        let err = stage_into(&dmg, &installed, "0.5.0", &updates).unwrap_err();
        assert!(err.contains("not newer"), "{err}");
    }

    fn fake_tool(dir: &Path, name: &str) {
        let path = dir.join(name);
        fs::write(&path, "#!/bin/sh\necho \"$0 $@\" >> \"$MARK\"\nexit 0\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn helper_swaps_the_app_and_restores_it_on_failure() {
        let root = TempDir::new("otozane-helper").unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        fake_tool(&bin, "launchctl");
        fake_tool(&bin, "open");
        let mark = root.path().join("mark");
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());

        let target = root.path().join("Otozane.app");
        let staged = root.path().join("staged.app");
        let backup = root.path().join("backup.app");
        let log = root.path().join("last-update.log");
        let plist = root.path().join("agent.plist");
        fs::write(&plist, "").unwrap();
        let app_marker = |dir: &Path| fs::read_to_string(dir.join("which")).unwrap();
        let make = |dir: &Path, which: &str| {
            fs::create_dir_all(dir).unwrap();
            fs::write(dir.join("which"), which).unwrap();
        };
        let script = root.path().join("restart.sh");
        let run = |pid| {
            fs::write(
                &script,
                restart_script(pid, &target, &staged, &backup, &log, Some(&plist)),
            )
            .unwrap();
            Command::new("sh")
                .arg(&script)
                .env("PATH", &path)
                .env("MARK", &mark)
                .status()
                .unwrap()
        };

        make(&target, "old");
        make(&staged, "new");
        assert!(run(0).success());
        assert_eq!(app_marker(&target), "new");
        assert!(!backup.exists());
        assert!(!script.exists());
        let marks = fs::read_to_string(&mark).unwrap();
        assert!(marks.contains("launchctl bootstrap gui/"), "{marks}");
        assert!(marks.contains(&plist.display().to_string()), "{marks}");
        assert!(
            marks.contains(&format!("open {}", target.display())),
            "{marks}"
        );
        assert!(fs::read_to_string(&log).unwrap().contains("update: done"));

        // A missing staged app must leave the installed one exactly as it was.
        make(&target, "old");
        assert!(!run(0).success());
        assert_eq!(app_marker(&target), "old");
        assert!(!backup.exists());
        assert!(fs::read_to_string(&log).unwrap().contains("restoring"));
    }
}
