//! About Otozane settings section: build metadata and the project repository.

use crate::widgets::{self, card, row, section_with};
use egui::Ui;

const REPOSITORY: &str = "https://github.com/binido/Otozane";
const CREATOR: &str = "Binido";

/// Shows the build version, license, creator and a link to the repository.
/// Returns an error if the system browser could not be started.
pub fn section(ui: &mut Ui) -> std::io::Result<()> {
    let mut opened = Ok(());
    section_with(ui, "About Otozane", |ui| {
        if widgets::link(ui, "GitHub repository").clicked() {
            opened = open_in_browser(REPOSITORY);
        }
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
    opened
}

/// Hands the URL to the desktop's default browser.
fn open_in_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}
