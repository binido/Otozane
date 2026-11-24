//! Starting `otozaned` through a launchd user agent on macOS.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) const LABEL: &str = "io.github.binido.otozane";

pub(crate) fn plist_path() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is not set"))?;
    Ok(Path::new(&home).join(format!("Library/LaunchAgents/{LABEL}.plist")))
}

/// The agent definition. A crash restarts the service, a clean exit
/// (Stop in the app, a second instance) does not.
fn plist_text(service: &Path) -> String {
    let exe = service
        .display()
        .to_string()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{LABEL}</string>
    <key>ProgramArguments</key><array><string>{exe}</string></array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
    <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
"#
    )
}

fn domain() -> String {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    format!("gui/{}", unsafe { getuid() })
}

fn launchctl(args: &[&str]) -> io::Result<()> {
    let out = Command::new("launchctl").args(args).output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

pub(crate) fn snapshot_for_update(
    source: &Path,
    snapshot: &Path,
    executable: &Path,
) -> Result<Option<PathBuf>, String> {
    let contents = match std::fs::read(source) {
        Ok(contents) => contents,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Could not read the Otozane LaunchAgent: {e}")),
    };
    std::fs::write(snapshot, contents)
        .map_err(|e| format!("Could not prepare the Otozane LaunchAgent: {e}"))?;
    let output = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(snapshot)
        .output()
        .map_err(|e| format!("Could not inspect the Otozane LaunchAgent: {e}"))?;
    let config: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Could not read the Otozane LaunchAgent configuration.")?;
    let correct_path =
        |value: &serde_json::Value| value.as_str().map(Path::new) == Some(executable);
    if !output.status.success()
        || config["Label"].as_str() != Some(LABEL)
        || !correct_path(&config["ProgramArguments"][0])
        || config
            .get("Program")
            .is_some_and(|program| !correct_path(program))
    {
        return Err("The LaunchAgent does not point to this Otozane app. In this copy of Otozane, turn Start with session off and back on under Settings → Background, then retry the update.".into());
    }
    Ok(Some(snapshot.to_owned()))
}

/// Whether the agent starts the service at login.
pub fn autostart_enabled() -> bool {
    plist_path().is_ok_and(|p| p.exists())
}

/// Installs or removes the agent. `start_now` also loads it, which starts the
/// service. Pass false while another instance is running.
pub fn set_autostart(on: bool, start_now: bool) -> io::Result<()> {
    let _installation_lock = crate::update::installation_lock()?;
    let path = plist_path()?;
    // Unloading an agent that is not loaded fails, that is fine.
    let _ = launchctl(&["bootout", &format!("{}/{LABEL}", domain())]);
    if !on {
        return std::fs::remove_file(path).or_else(|e| match e.kind() {
            io::ErrorKind::NotFound => Ok(()),
            _ => Err(e),
        });
    }
    let service = crate::client::local_service()
        .ok_or_else(|| io::Error::other("otozaned is not next to this app"))?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, plist_text(&service))?;
    if start_now {
        launchctl(&["bootstrap", &domain(), &path.display().to_string()])?;
    }
    Ok(())
}

/// Starts `otozaned`: through launchd when the agent is installed, otherwise
/// detached from this process, from next to the current executable if
/// present, else from `PATH`.
pub fn start_service() -> io::Result<()> {
    if let Ok(path) = plist_path()
        && path.exists()
    {
        let target = format!("{}/{LABEL}", domain());
        return launchctl(&["kickstart", &target])
            .or_else(|_| launchctl(&["bootstrap", &domain(), &path.display().to_string()]));
    }
    crate::client::start_detached()
}

#[cfg(test)]
mod tests {
    #[test]
    fn plist_escapes_the_service_path() {
        let text = super::plist_text(std::path::Path::new("/Apps/A&B <x>/otozaned"));
        assert!(text.contains("<string>/Apps/A&amp;B &lt;x&gt;/otozaned</string>"));
    }

    #[test]
    fn update_rejects_a_stale_agent_and_keeps_a_verified_snapshot() {
        use super::*;

        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("agent.plist");
        let snapshot = root.path().join("snapshot.plist");
        let service = root
            .path()
            .join("Current app/Otozane.app/Contents/MacOS/otozaned");
        let stale = root
            .path()
            .join("Previous app/Otozane.app/Contents/MacOS/otozaned");
        assert_eq!(
            snapshot_for_update(&source, &snapshot, &service).unwrap(),
            None
        );
        std::fs::write(&source, plist_text(&stale)).unwrap();
        assert!(snapshot_for_update(&source, &snapshot, &service).is_err());
        let expected = plist_text(&service);
        std::fs::write(&source, &expected).unwrap();
        assert_eq!(
            snapshot_for_update(&source, &snapshot, &service).unwrap(),
            Some(snapshot.clone())
        );
        std::fs::write(&source, plist_text(&stale)).unwrap();
        assert_eq!(std::fs::read_to_string(snapshot).unwrap(), expected);
    }
}
