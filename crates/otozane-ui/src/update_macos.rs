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
//! config directory.

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
    #[cfg(test)]
    pub fn app(&self) -> &Path {
        &self.app
    }

    /// `CFBundleShortVersionString` of the installed app.
    #[cfg(test)]
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
#[derive(Debug)]
pub struct Staged {
    version: String,
    work: Option<tempfile::TempDir>,
    target: PathBuf,
    installed_version: String,
    log: PathBuf,
    failure: PathBuf,
}

impl Staged {
    /// Starts the helper that swaps the bundle once this process exits.
    pub fn restart(&mut self) -> Result<(), String> {
        use crate::client;
        use std::os::unix::process::CommandExt;

        let work = self
            .work
            .as_ref()
            .ok_or("The update was already handed off.")?;
        let lock = crate::update::installation_lock().map_err(|e| e.to_string())?;
        if app_version(&self.target).as_ref() != Some(&self.installed_version) {
            return Err("The installed bundle changed. Reopen Otozane to check again.".into());
        }
        let plist = crate::launchd::plist_path().map_err(|e| e.to_string())?;
        let agent = crate::launchd::snapshot_for_update(
            &plist,
            &work.path().join("agent.plist"),
            &self.target.join("Contents/MacOS/otozaned"),
        )?;
        for path in [&self.target, work.path(), &self.failure] {
            if path.to_str().is_none() {
                return Err("The update location contains an unsupported path.".into());
            }
        }
        let was_running = client::service_is_running()
            .map_err(|e| format!("Could not check the audio service status: {e}"))?;
        let script = work.path().join("restart.sh");
        let log = std::fs::File::create(&self.log)
            .map_err(|e| format!("Could not open the update log: {e}"))?;
        std::fs::write(
            &script,
            restart_script(
                std::process::id(),
                &self.target,
                work.path(),
                agent.as_deref(),
                was_running,
                &self.failure,
                &self.version,
            ),
        )
        .map_err(|e| format!("Could not write the update helper: {e}"))?;
        let mut cmd = Command::new("/bin/sh");
        // The helper holds the installation lock after this process exits.
        cmd.arg(&script)
            .stdin(lock)
            .stderr(log.try_clone().map_err(|e| e.to_string())?)
            .stdout(log)
            .process_group(0);
        let mut handoff = || {
            client::stop_for_update(|| Ok(()))
                .map_err(|e| format!("Could not stop the audio service: {e}"))?;
            cmd.spawn()
                .map_err(|e| format!("Could not start the update helper: {e}"))?;
            Ok(())
        };
        handoff().map_err(|e| client::restore_after_update_error(was_running, e))?;
        let _ = self.work.take().unwrap().keep();
        Ok(())
    }
}

/// Validates the app in the DMG and stages it beside the installed bundle.
pub fn stage(dmg: &Path, bundle: &Bundle, version: &str) -> Result<Staged, String> {
    stage_into(dmg, bundle, version, &crate::update::staging_dir())
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
    let parent = bundle
        .app
        .parent()
        .ok_or("The installed app has no parent folder.")?;
    let work = tempfile::Builder::new()
        .prefix(".otozane-update-")
        .tempdir_in(parent)
        .map_err(|e| format!("Could not stage the new app beside the installed bundle: {e}"))?;
    let app = work.path().join("staged.app");
    let mount = TempDir::new("otozane-update-dmg")?;
    {
        let _mounted = Mount::attach(dmg, mount.path())?;
        copy_app(&only_app(mount.path())?, &app)?;
    }
    validate_app(&app, expected)?;
    strip_quarantine(&app)?;
    Ok(Staged {
        version: expected.to_string(),
        work: Some(work),
        target: bundle.app.clone(),
        installed_version: bundle.version.clone(),
        log: updates.join("last-update.log"),
        failure: updates.join("last-update-error"),
    })
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
    if plist_value(&text, "CFBundleExecutable").as_deref() != Some("otozane") {
        return Err("The new app has an unexpected bundle executable.".into());
    }
    for executable in ["otozane", "otozaned"] {
        let path = app.join("Contents/MacOS").join(executable);
        let meta = std::fs::metadata(&path)
            .map_err(|e| format!("The new app is missing {executable}: {e}"))?;
        if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 || !is_arm64(&path) {
            return Err(format!(
                "The new app has no runnable Apple Silicon {executable}."
            ));
        }
    }
    let mut cmd = Command::new("codesign");
    cmd.args(["--verify", "--deep", "--strict"]).arg(app);
    run(&mut cmd, "verify the new app's signature")?;
    Ok(())
}

/// Package verification precedes quarantine removal for the ad-hoc signed bundle.
fn strip_quarantine(app: &Path) -> Result<(), String> {
    const ATTR: &str = "com.apple.quarantine";
    let _ = Command::new("/usr/bin/xattr")
        .args(["-drs", ATTR])
        .arg(app)
        .output()
        .map_err(|e| format!("Could not remove the update's quarantine attribute: {e}"))?;
    let names = run(
        Command::new("/usr/bin/xattr").arg("-rs").arg(app),
        "check the update's quarantine attributes",
    )?;
    if names
        .lines()
        .any(|line| line.ends_with(": com.apple.quarantine") || line == ATTR)
    {
        return Err("The staged update is still quarantined.".into());
    }
    Ok(())
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

pub fn take_failure() -> Option<String> {
    take_failure_in(&crate::update::staging_dir())
}

fn take_failure_in(updates: &Path) -> Option<String> {
    let path = updates.join("last-update-error");
    if !path.is_file() {
        return None;
    }
    let consumed = tempfile::NamedTempFile::new_in(updates).ok()?;
    std::fs::rename(path, consumed.path()).ok()?;
    let reason = std::fs::read_to_string(consumed.path()).ok()?;
    Some(format!(
        "{reason} See {} for details.",
        updates.join("last-update.log").display()
    ))
}

fn restart_script(
    pid: u32,
    target: &Path,
    work: &Path,
    plist: Option<&Path>,
    was_running: bool,
    failure: &Path,
    version: &str,
) -> String {
    let quote = |p: &Path| crate::driver::shell_quote(&p.to_string_lossy());
    format!(
        r#"#!/bin/sh
target={}
work={}
plist={}
was_running={}
failure={}
version={}
domain="gui/$(id -u)"
job="$domain/{label}"
backup="$work/backup.app"
service_pid=
service_mode=detached
[ -z "$plist" ] || service_mode=agent

stop_agent() {{
    # Status 113 from launchctl print means the service is absent.
    bootout_status=0
    launchctl bootout "$job" || bootout_status=$?
    attempts=0
    while [ "$attempts" -lt 20 ]; do
        print_status=0
        launchctl print "$job" >/dev/null 2>&1 || print_status=$?
        case "$print_status" in
            113) return 0 ;;
            0) [ "$bootout_status" -eq 0 ] || return 1 ;;
            *) echo "update: could not check the launchd job (exit $print_status)"; return 1 ;;
        esac
        attempts=$((attempts + 1))
        sleep 0.1
    done
    echo "update: the launchd job did not unload"
    return 1
}}

start_service() {{
    if [ "$service_mode" = agent ]; then
        launchctl bootstrap "$domain" "$plist"
    else
        [ -x "$target/Contents/MacOS/otozaned" ] || return 1
        nohup "$target/Contents/MacOS/otozaned" </dev/null >/dev/null 2>&1 &
        service_pid=$!
    fi
}}

wait_for_child_exit() {{
    for attempt in 1 2 3 4 5 6 7 8 9 10; do
        kill -0 "$service_pid" 2>/dev/null || return 0
        sleep 0.1
    done
    ! kill -0 "$service_pid" 2>/dev/null
}}

stop_service() {{
    if [ "$service_mode" = agent ]; then
        stop_agent
    elif [ -n "$service_pid" ]; then
        kill -TERM "$service_pid" 2>/dev/null || :
        if ! wait_for_child_exit; then
            kill -KILL "$service_pid" 2>/dev/null || :
            wait_for_child_exit || return 1
        fi
        wait "$service_pid" 2>/dev/null
        service_pid=
    fi
}}

report_failure() {{
    echo "update: $1"
    report=$(mktemp "$failure.XXXXXX") || return 1
    printf '%s' "$1" > "$report" && mv -f "$report" "$failure"
}}

restore_service() {{
    [ "$was_running" = 1 ] || return 0
    if [ "$service_mode" = agent ]; then
        stop_agent || return 1
    fi
    start_service
}}

reopen_previous() {{
    reason=$1
    if ! restore_service; then
        reason="$reason Could not restart the previous audio service."
    fi
    report_failure "$reason"
    open "$target" || echo "update: could not reopen the previous app"
    rm -rf "$work"
    exit 1
}}

restore() {{
    reason=$1
    if [ -e "$target" ] || ! mv "$backup" "$target"; then
        report_failure "$reason Could not restore the previous app from $backup."
        exit 1
    fi
    reopen_previous "$reason The previous app was restored."
}}

cd / || exit 1
pid={pid}
if [ "$pid" -gt 0 ]; then
    n=0
    while kill -0 "$pid" 2>/dev/null; do
        n=$((n + 1))
        if [ "$n" -ge 300 ]; then
            reason="The previous app did not exit in time. Its bundle was left in place."
            if ! restore_service; then
                reason="$reason Could not restart the previous audio service."
            fi
            report_failure "$reason"
            rm -rf "$work"
            exit 1
        fi
        sleep 0.2
    done
fi

if [ "$service_mode" = agent ] && ! stop_agent; then
    reopen_previous "Could not unload the Otozane LaunchAgent. The previous bundle was left in place."
fi
echo "update: replacing $target"
if ! mv "$target" "$backup"; then
    reopen_previous "Could not move the previous app aside."
fi
if ! mv "$work/staged.app" "$target"; then
    restore "Could not install the new app."
fi
reason=
# The backup supplies the readiness probe for releases without that command.
if ! start_service || ! "$backup/Contents/MacOS/otozane" --wait-for-update-service "$version" </dev/null; then
    reason="The replacement audio service did not become ready."
elif ! open "$target"; then
    reason="Could not open the new app."
fi
if [ -n "$reason" ]; then
    if ! stop_service || ! mv "$target" "$work/failed.app"; then
        report_failure "$reason Could not remove the failed update. The previous app is at $backup."
        exit 1
    fi
    restore "$reason"
fi
rm -rf "$work"
echo "update: done"
"#,
        quote(target),
        quote(work),
        plist.map_or_else(|| "''".to_string(), quote),
        u8::from(was_running),
        quote(failure),
        crate::driver::shell_quote(version),
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
        for executable in ["otozane", "otozaned"] {
            fs::copy("/bin/echo", app.join("Contents/MacOS").join(executable)).unwrap();
        }
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
        assert_eq!(staged.version, "0.6.0");
        assert!(
            staged
                .work
                .as_ref()
                .unwrap()
                .path()
                .join("staged.app/Contents/MacOS/otozane")
                .exists()
        );
        assert!(target.join("Contents/MacOS/otozane").exists());

        let err = stage_into(&dmg, &installed, "0.7.0", &updates).unwrap_err();
        assert!(err.contains("expected 0.7.0"), "{err}");
        assert_eq!(fs::read_dir(&target_root).unwrap().count(), 2);

        let err = stage_into(&dmg, &installed, "0.5.0", &updates).unwrap_err();
        assert!(err.contains("not newer"), "{err}");
    }

    #[test]
    fn quarantine_is_removed_from_nested_files_and_absence_is_accepted() {
        let root = TempDir::new("otozane-quarantine").unwrap();
        let child = root.path().join("file");
        fs::write(&child, "").unwrap();
        assert!(
            Command::new("/usr/bin/xattr")
                .args(["-w", "com.apple.quarantine", "0081;test"])
                .arg(&child)
                .status()
                .unwrap()
                .success()
        );
        strip_quarantine(root.path()).unwrap();
        strip_quarantine(root.path()).unwrap();
        assert!(strip_quarantine(&root.path().join("missing")).is_err());
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
        fake_tool(&bin, "open");
        let launchctl = bin.join("launchctl");
        fs::write(
            &launchctl,
            r#"#!/bin/sh
echo "$0 $@" >> "$MARK"
case "$1" in
    bootout)
        printf 2 > "$PENDING_REMOVAL"
        ;;
    print)
        [ "$HOLD_AGENT" = 1 ] && exit 0
        pending=$(cat "$PENDING_REMOVAL")
        [ "$pending" -gt 0 ] || exit 113
        printf '%s' "$((pending - 1))" > "$PENDING_REMOVAL"
        ;;
    bootstrap)
        [ "$(cat "$PENDING_REMOVAL")" = 0 ]
        ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(launchctl, fs::Permissions::from_mode(0o755)).unwrap();
        let mark = root.path().join("mark");
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());

        let target = root.path().join("Otozane.app");
        let work = root.path().join("work");
        let staged = work.join("staged.app");
        let log = root.path().join("last-update.log");
        let plist = root.path().join("agent.plist");
        fs::write(&plist, "").unwrap();
        let failure = root.path().join("last-update-error");
        let app_marker = |dir: &Path| fs::read_to_string(dir.join("which")).unwrap();
        let make = |dir: &Path, which: &str| {
            fs::create_dir_all(dir).unwrap();
            fs::write(dir.join("which"), which).unwrap();
            let macos = dir.join("Contents/MacOS");
            fs::create_dir_all(&macos).unwrap();
            let executable = macos.join("otozane");
            fs::write(
                &executable,
                r#"#!/bin/sh
echo "probe $*" >> "$MARK"
[ "${PROBE_FAIL:-0}" = 0 ] && [ -d "$(dirname "$0")/../.." ]
"#,
            )
            .unwrap();
            fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
        };
        let run = |probe_fail, hold_agent| {
            fs::create_dir_all(&work).unwrap();
            let script = work.join("restart.sh");
            fs::write(
                &script,
                restart_script(0, &target, &work, Some(&plist), true, &failure, "0.6.0"),
            )
            .unwrap();
            let output = Command::new("sh")
                .arg(&script)
                .env("PATH", &path)
                .env("MARK", &mark)
                .env("PROBE_FAIL", if probe_fail { "1" } else { "0" })
                .env("HOLD_AGENT", if hold_agent { "1" } else { "0" })
                .env("PENDING_REMOVAL", root.path().join("pending-removal"))
                .output()
                .unwrap();
            fs::write(&log, output.stdout).unwrap();
            output.status
        };

        make(&target, "old");
        make(&staged, "new");
        assert!(run(false, false).success());
        assert_eq!(app_marker(&target), "new");
        assert!(!work.exists());
        let marks = fs::read_to_string(&mark).unwrap();
        let commands: Vec<_> = marks.lines().take(5).collect();
        assert!(commands[0].contains("launchctl bootout"), "{marks}");
        for command in &commands[1..4] {
            assert!(command.contains("launchctl print"), "{marks}");
        }
        assert!(commands[4].contains("launchctl bootstrap"), "{marks}");
        assert!(marks.contains(&plist.display().to_string()), "{marks}");
        assert!(
            marks.contains("probe --wait-for-update-service 0.6.0"),
            "{marks}"
        );
        assert!(!failure.exists());
        assert!(
            marks.contains(&format!("open {}", target.display())),
            "{marks}"
        );
        assert!(fs::read_to_string(&log).unwrap().contains("update: done"));

        fs::write(
            bin.join("open"),
            "#!/bin/sh\nwhich=$(cat \"$1/which\")\nprintf '%s\\n' \"$which\" >> \"$MARK\"\n[ \"$which\" = old ]\n",
        )
        .unwrap();
        fs::write(&mark, "").unwrap();
        make(&target, "old");
        make(&staged, "new");
        assert!(!run(false, false).success());
        assert_eq!(app_marker(&target), "old");
        assert!(!work.exists());
        let report = take_failure_in(root.path()).unwrap();
        assert!(report.contains("Could not open the new app."), "{report}");
        assert!(report.contains("restored"), "{report}");
        assert!(take_failure_in(root.path()).is_none());
        let lines = fs::read_to_string(&mark).unwrap();
        let events: Vec<_> = lines
            .lines()
            .filter(|line| !line.contains("launchctl print"))
            .collect();
        assert_eq!(events.len(), 8, "{lines}");
        assert!(events[0].contains("launchctl bootout"));
        assert!(events[1].contains("launchctl bootstrap"));
        assert_eq!(events[2], "probe --wait-for-update-service 0.6.0");
        assert_eq!(events[3], "new");
        assert!(events[4].contains("launchctl bootout"));
        assert!(events[6].contains("launchctl bootstrap"));
        assert_eq!(events[7], "old");

        fs::write(&mark, "").unwrap();
        make(&staged, "new");
        assert!(!run(true, false).success());
        assert_eq!(app_marker(&target), "old");
        let report = take_failure_in(root.path()).unwrap();
        assert!(report.contains("did not become ready"), "{report}");
        assert!(
            !fs::read_to_string(&mark)
                .unwrap()
                .lines()
                .any(|event| event == "new")
        );

        // A missing staged app must leave the installed one exactly as it was.
        make(&target, "old");
        assert!(!run(false, false).success());
        assert_eq!(app_marker(&target), "old");
        assert!(!work.exists());
        assert!(
            fs::read_to_string(&log)
                .unwrap()
                .contains("Could not install the new app")
        );

        fs::write(&mark, "").unwrap();
        make(&staged, "new");
        assert!(!run(false, true).success());
        assert_eq!(app_marker(&target), "old");
        let report = take_failure_in(root.path()).unwrap();
        assert!(report.contains("Could not unload"), "{report}");
        assert!(
            !fs::read_to_string(&mark)
                .unwrap()
                .contains("launchctl bootstrap")
        );
    }
    #[test]
    fn helper_stops_the_detached_replacement_before_restoring_the_old_service() {
        check_detached_rollback(false);
    }

    #[test]
    fn helper_recovers_when_a_detached_replacement_does_not_exit_on_term() {
        check_detached_rollback(true);
    }

    fn check_detached_rollback(ignore_term: bool) {
        let root = TempDir::new("otozane-detached-rollback").unwrap();
        let target = root.path().join("Otozane.app");
        let work = root.path().join("work");
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let daemon = r#"#!/bin/sh
kind=$(cat "$(dirname "$0")/which")
if [ "$kind" = new ]; then
    if [ "$IGNORE_TERM" = 1 ]; then
        trap '' TERM
    else
        trap 'echo "stop new" >> "$MARK"; exit 0' TERM
    fi
    echo "$$" > "$CHILD_PID"
    echo "start new" >> "$MARK"
    while :; do sleep 0.05; done
else
    if kill -0 "$(cat "$CHILD_PID")" 2>/dev/null; then
        echo "replacement still running" >> "$MARK"
        exit 1
    fi
    echo "start old" >> "$MARK"
fi
"#;
        for (bundle, version) in [(&target, "old"), (&work.join("staged.app"), "new")] {
            let dir = bundle.join("Contents/MacOS");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("which"), version).unwrap();
            let executable = dir.join("otozaned");
            fs::write(&executable, daemon).unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let open = bin.join("open");
        fs::write(
            &open,
            r#"#!/bin/sh
kind=$(cat "$1/Contents/MacOS/which")
for attempt in 1 2 3 4 5 6 7 8 9 10; do
    if grep -q "start $kind" "$MARK"; then break; fi
    sleep 0.05
done
echo "open $kind" >> "$MARK"
[ "$kind" = old ]
"#,
        )
        .unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).unwrap();
        let script = work.join("restart.sh");
        fake_tool(&target.join("Contents/MacOS"), "otozane");
        let failure = root.path().join("last-update-error");
        fs::write(
            &script,
            restart_script(0, &target, &work, None, true, &failure, "0.6.0"),
        )
        .unwrap();
        let mark = root.path().join("events");
        fs::write(&mark, "").unwrap();
        let output = Command::new("/bin/sh")
            .arg(script)
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("MARK", &mark)
            .env("IGNORE_TERM", if ignore_term { "1" } else { "0" })
            .env("CHILD_PID", root.path().join("child.pid"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!work.exists());
        let events = fs::read_to_string(mark).unwrap();
        let ordered: Vec<_> = events
            .lines()
            .filter(|line| !line.contains("--wait-for-update-service"))
            .collect();
        if ignore_term {
            assert_eq!(ordered, ["start new", "open new", "start old", "open old"]);
        } else {
            assert_eq!(
                ordered,
                ["start new", "open new", "stop new", "start old", "open old"]
            );
        }
        assert_eq!(
            fs::read_to_string(target.join("Contents/MacOS/which")).unwrap(),
            "old"
        );
    }
}
