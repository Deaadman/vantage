//! Where valreview keeps its files.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

pub const DATA_DIR_ENV: &str = "VALREVIEW_DATA_DIR";

/// `explicit`, else `$VALREVIEW_DATA_DIR`, else `%LOCALAPPDATA%\valreview\data`.
pub fn data_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    if let Some(p) = std::env::var_os(DATA_DIR_ENV).filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    directories::ProjectDirs::from("", "", "valreview")
        .map(|d| d.data_local_dir().to_path_buf())
        .ok_or_else(|| anyhow!("cannot find a home directory; pass --data-dir or set {DATA_DIR_ENV}"))
}

pub fn catalog_path(data_dir: &Path) -> PathBuf {
    data_dir.join("catalog.json")
}
