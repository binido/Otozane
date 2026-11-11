//! Settings and profile files under `$XDG_CONFIG_HOME/otozane/`.

use otozane_core::config::{Profile, Settings};
use std::io;
use std::path::{Path, PathBuf};

pub struct Store {
    dir: PathBuf,
}

pub use otozane_core::config::config_dir;

/// Profile names become file names, so they are restricted at this boundary.
pub fn validate_name(name: &str) -> Result<(), String> {
    let ok = !name.trim().is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control());
    if ok {
        Ok(())
    } else {
        Err(format!("invalid profile name '{name}'"))
    }
}

fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(tmp, path)
}

fn to_io(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    fn settings_path(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    fn profile_path(&self, name: &str) -> PathBuf {
        self.dir.join("profiles").join(format!("{name}.toml"))
    }

    /// Loads settings. An unreadable file is moved aside to `config.toml.bak`
    /// rather than overwritten, so a typo never destroys user configuration.
    pub fn load_settings(&self) -> Settings {
        let path = self.settings_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                eprintln!("{}: {e}; using defaults", path.display());
                let _ = std::fs::rename(&path, path.with_extension("toml.bak"));
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save_settings(&self, s: &Settings) -> io::Result<()> {
        write_atomic(&self.settings_path(), &toml::to_string(s).map_err(to_io)?)
    }

    pub fn list_profiles(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir.join("profiles"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension()? == "toml")
                    .then(|| p.file_stem()?.to_str().map(String::from))
                    .flatten()
            })
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        names
    }

    pub fn load_profile(&self, name: &str) -> io::Result<Profile> {
        validate_name(name).map_err(to_io)?;
        let s = std::fs::read_to_string(self.profile_path(name))?;
        toml::from_str(&s).map_err(to_io)
    }

    pub fn save_profile(&self, name: &str, p: &Profile) -> io::Result<()> {
        validate_name(name).map_err(to_io)?;
        write_atomic(
            &self.profile_path(name),
            &toml::to_string(p).map_err(to_io)?,
        )
    }

    pub fn delete_profile(&self, name: &str) -> io::Result<()> {
        validate_name(name).map_err(to_io)?;
        std::fs::remove_file(self.profile_path(name))
    }

    pub fn rename_profile(&self, from: &str, to: &str) -> io::Result<()> {
        validate_name(from).map_err(to_io)?;
        validate_name(to).map_err(to_io)?;
        let target = self.profile_path(to);
        if target.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("profile '{to}' already exists"),
            ));
        }
        std::fs::rename(self.profile_path(from), target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_lifecycle_and_name_validation() {
        let dir = std::env::temp_dir().join(format!("otozane-store-{}", std::process::id()));
        let store = Store::new(dir.clone());
        assert!(validate_name("../evil").is_err());
        assert!(validate_name(".hidden").is_err());
        assert!(validate_name("Streaming").is_ok());

        store.save_profile("B", &Profile::default()).unwrap();
        store.save_profile("a", &Profile::default()).unwrap();
        assert_eq!(store.list_profiles(), vec!["a", "B"]);
        store.rename_profile("a", "C").unwrap();
        assert!(store.rename_profile("C", "B").is_err());
        store.delete_profile("C").unwrap();
        assert_eq!(store.list_profiles(), vec!["B"]);
        assert_eq!(store.load_profile("B").unwrap(), Profile::default());

        std::fs::write(dir.join("config.toml"), "not = [valid").unwrap();
        assert_eq!(store.load_settings(), Settings::default());
        assert!(dir.join("config.toml.bak").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
