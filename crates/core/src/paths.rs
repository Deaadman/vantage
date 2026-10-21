//! Where Vantage keeps its files.

use std::path::{Path, PathBuf};

use anyhow::Result;

pub const DATA_DIR_ENV: &str = "VANTAGE_DATA_DIR";
pub const HOME_ENV: &str = "VANTAGE_HOME";

/// `explicit`, else `$VANTAGE_DATA_DIR`, else `%LOCALAPPDATA%\vantage\data`.
pub fn data_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    shellkit::files::data_dir(crate::install::APP, explicit)
}

/// Where the installer puts `bin`, `versions` and `vrfkit`: `$VANTAGE_HOME`,
/// else `%LOCALAPPDATA%\vantage` (the data dir's default parent).
pub fn install_root() -> Result<PathBuf> {
    Ok(crate::install::layout()?.root)
}

/// One log file per run, for bug reports (`shellkit::log`).
pub fn logs_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

pub fn catalog_path(data_dir: &Path) -> PathBuf {
    data_dir.join("catalog.json")
}

pub fn db_path(data_dir: &Path) -> PathBuf {
    data_dir.join("vantage.db")
}
