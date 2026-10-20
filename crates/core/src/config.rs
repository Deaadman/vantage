//! `config.toml` in the data dir.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Your account UUID.
    pub me: Option<String>,
    /// Path to `vrfkit.exe`.
    pub vrfkit: Option<PathBuf>,
    /// Accept a vrfkit binary with this SHA-256 instead of the pinned release
    /// (set this deliberately after upgrading vrfkit).
    pub vrfkit_sha256: Option<String>,
    /// Where the client saves replays.
    pub demos_dir: Option<PathBuf>,
    /// `false` stops the shell's background update check (`valreview update` still works).
    pub auto_update: Option<bool>,
}

impl Config {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("config.toml")
    }

    /// A missing file is an empty config.
    pub fn load(data_dir: &Path) -> Result<Self> {
        let path = Self::path(data_dir);
        if !path.is_file() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(data_dir).with_context(|| format!("creating {}", data_dir.display()))?;
        let path = Self::path(data_dir);
        std::fs::write(&path, toml::to_string_pretty(self)?).with_context(|| format!("writing {}", path.display()))
    }

    /// Configured demos dir, else `%LOCALAPPDATA%\VALORANT\Saved\Demos`.
    pub fn demos_dir(&self) -> Option<PathBuf> {
        self.demos_dir.clone().or_else(default_demos_dir)
    }
}

pub fn default_demos_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("VALORANT").join("Saved").join("Demos"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("valreview-config-{}", std::process::id()));
        assert!(Config::load(&dir).unwrap().me.is_none());
        let c = Config {
            me: Some("43082880-1822-5e84-a3f2-337d8a51a958".into()),
            vrfkit: Some(r"C:\tools\vrfkit.exe".into()),
            ..Default::default()
        };
        c.save(&dir).unwrap();
        let back = Config::load(&dir).unwrap();
        assert_eq!(back.me, c.me);
        assert_eq!(back.vrfkit, c.vrfkit);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
