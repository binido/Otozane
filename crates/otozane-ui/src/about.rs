//! About Otozane settings section: build metadata and the project repository.

use crate::widgets::{self, card, row, section_with};
use egui::Ui;
use std::io;
use std::process::{Command, Stdio};

const REPOSITORY: &str = "https://github.com/binido/Otozane";
const CREATOR: &str = "Binido";

/// Shows the build version, license, creator and a link to the repository.
/// Returns whether the repository link was clicked.
pub fn section(ui: &mut Ui) -> bool {
    let mut clicked = false;
    section_with(ui, "About Otozane", |ui| {
        clicked = widgets::link(ui, "GitHub repository").clicked();
    });
    card(ui, |ui| {
        row(ui, "Version", None, |ui| {
            ui.label(widgets::medium(env!("CARGO_PKG_VERSION")));
        });
        widgets::divider(ui);
        row(ui, "License", None, |ui| {
            ui.label(widgets::medium(env!("CARGO_PKG_LICENSE")));
        });
        widgets::divider(ui);
        row(ui, "Created by", None, |ui| {
            ui.label(widgets::medium(CREATOR));
        });
    });
    clicked
}

/// Opens the repository in the default browser on a background thread.
pub fn open_repository(
    ctx: egui::Context,
    report_error: impl FnOnce(io::Error) + Send + 'static,
) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";

    std::thread::Builder::new().spawn(move || {
        if let Err(error) = run_opener(Command::new(opener).arg(REPOSITORY)) {
            report_error(error);
            ctx.request_repaint();
        }
    })?;
    Ok(())
}

fn run_opener(command: &mut Command) -> io::Result<()> {
    let status = command
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("opener returned {status}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_an_unsuccessful_opener_exit() {
        let error = run_opener(Command::new("/bin/sh").args(["-c", "exit 4"])).unwrap_err();
        assert!(error.to_string().contains("exit status: 4"));
    }

    #[test]
    fn accepts_a_successful_opener_exit() {
        run_opener(Command::new("/bin/sh").args(["-c", "exit 0"])).unwrap();
    }
}
