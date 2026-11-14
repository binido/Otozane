//! Installing and removing the Otozane Microphone audio driver on macOS.
//! Both need administrator rights, macOS asks for the password itself.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

const INSTALLED: &str = "/Library/Audio/Plug-Ins/HAL/Otozane.driver";

/// Result of the last install or remove, taken by the UI to show a toast.
static RESULT: Mutex<Option<Result<String, String>>> = Mutex::new(None);

pub fn installed() -> bool {
    Path::new(INSTALLED).exists()
}

/// The driver shipped in the app bundle, next to the binaries in Contents/Resources.
fn bundled() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("../Resources/Otozane.driver")).filter(|p| p.exists())
}

pub fn can_install() -> bool {
    bundled().is_some()
}

/// Copies the bundled driver into place in the background.
pub fn install() {
    let Some(src) = bundled() else {
        *RESULT.lock().unwrap() = Some(Err("The app has no driver to install".into()));
        return;
    };
    let cmd = format!(
        "rm -rf {INSTALLED} && cp -R {} /Library/Audio/Plug-Ins/HAL/ && killall coreaudiod",
        shell_quote(&src.display().to_string())
    );
    run(cmd, "Otozane Microphone installed");
}

/// Removes the driver in the background.
pub fn remove() {
    run(
        format!("rm -rf {INSTALLED} && killall coreaudiod"),
        "Otozane Microphone removed",
    );
}

/// Takes the outcome of a finished install or remove.
pub fn take_result() -> Option<Result<String, String>> {
    RESULT.lock().unwrap().take()
}

/// Runs `cmd` as root through AppleScript. Off the UI thread, since it waits
/// for the password dialog. coreaudiod is restarted so it loads the change,
/// all sound on the Mac pauses for a moment.
fn run(cmd: String, done: &'static str) {
    std::thread::spawn(move || {
        let script = format!(
            "do shell script \"{}\" with administrator privileges",
            cmd.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let out = std::process::Command::new("osascript")
            .args(["-e", &script])
            .output();
        let result = match out {
            Ok(o) if o.status.success() => Ok(done.to_string()),
            // -128 is the user pressing Cancel in the password dialog.
            Ok(o) if String::from_utf8_lossy(&o.stderr).contains("-128") => Err("Cancelled".into()),
            Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
            Err(e) => Err(e.to_string()),
        };
        *RESULT.lock().unwrap() = Some(result);
    });
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn quotes_paths_for_the_shell() {
        assert_eq!(super::shell_quote("/A b/it's"), r"'/A b/it'\''s'");
    }
}
