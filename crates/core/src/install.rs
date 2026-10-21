//! The installed copy in `%LOCALAPPDATA%\vantage` (or `$VANTAGE_HOME`):
//!
//! - `versions\<version>\vantage.exe`: every installed version.
//! - `bin\vantage.exe`: a copy of the current one; `bin` is on PATH.
//! - `vrfkit\<tag>\vrfkit.exe`: vrfkit releases, with `vrfkit\pin.json`
//!   naming the one in use once a vrfkit update has installed one.
//!
//! The versions and the launcher are Abalone's (`abalone::install`);
//! Vantage adds vrfkit, fetched before the launcher is swapped, and ffmpeg.

use std::fs;
use std::path::{Path, PathBuf};

pub use abalone::install::{Layout, check_sha256, is_newer, report, sha256_file, version_key};
use anyhow::{Context, Result, bail};

use crate::vrfkit::{Pin, Vrfkit};

/// The exe's name, and the install folder's.
pub const APP: &str = "vantage";

/// `$VANTAGE_HOME`, else `%LOCALAPPDATA%\vantage`.
pub fn layout() -> Result<Layout> {
    Layout::new(APP)
}

/// `vrfkit\<tag>\vrfkit.exe`.
pub fn vrfkit_exe(layout: &Layout, tag: &str) -> PathBuf {
    layout.root.join("vrfkit").join(tag).join("vrfkit.exe")
}

#[derive(Debug, Default)]
pub struct Installed {
    /// vrfkit was downloaded (not already present).
    pub vrfkit_downloaded: bool,
}

/// Installs `exe` (the running binary, version `version`): copies it into
/// `versions`, makes sure the vrfkit in use is there, points `bin` at it and
/// removes old versions.
pub fn install(layout: &Layout, version: &str, exe: &Path) -> Result<Installed> {
    let target = abalone::install::copy_in(layout, version, exe)?;
    let vrfkit_downloaded = ensure_vrfkit(layout)?;
    abalone::install::place_launcher(layout, &target)?;
    cleanup(layout, version);
    Ok(Installed { vrfkit_downloaded })
}

/// Best effort: old launchers and versions (see `abalone::install::cleanup`),
/// and all but the newest older vrfkit release.
pub fn cleanup(layout: &Layout, current: &str) {
    abalone::install::cleanup(layout, current);
    abalone::install::prune(&layout.root.join("vrfkit"), &crate::vrfkit::active().tag);
}

/// Downloads and verifies the vrfkit in use unless it is already installed.
/// True when it downloaded.
pub fn ensure_vrfkit(layout: &Layout) -> Result<bool> {
    fetch_vrfkit(layout, &crate::vrfkit::active())
}

/// Installs `pin`'s vrfkit and switches to it, when it's newer than the one
/// in use and Vantage `vantage` reads its output. It must start before
/// Vantage switches; the one before stays installed. True when it switched.
pub fn update_vrfkit(layout: &Layout, pin: &Pin, vantage: &str) -> Result<bool> {
    let current = crate::vrfkit::active();
    if !is_newer(&pin.tag, &current.tag) || is_newer(&pin.min_vantage, vantage) {
        return Ok(false);
    }
    fetch_vrfkit(layout, pin)?;
    if !Vrfkit::starts(&vrfkit_exe(layout, &pin.tag)) {
        bail!(
            "vrfkit {} doesn't start on this PC, so Vantage keeps using {}",
            pin.tag,
            current.tag
        );
    }
    pin.save(layout)?;
    abalone::install::prune(&layout.root.join("vrfkit"), &pin.tag);
    Ok(true)
}

/// Downloads and verifies `pin`'s vrfkit unless it is already installed.
fn fetch_vrfkit(layout: &Layout, pin: &Pin) -> Result<bool> {
    let exe = vrfkit_exe(layout, &pin.tag);
    if exe.is_file() && sha256_file(&exe)?.eq_ignore_ascii_case(&pin.exe_sha256) {
        return Ok(false);
    }
    let dir = exe.parent().expect("vrfkit_exe has a parent");
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let zip_path = dir.join("download.zip");
    download(&pin.zip_url, &zip_path)?;
    let result = (|| {
        check_sha256(&zip_path, &pin.zip_sha256, "the vrfkit release ZIP")?;
        let partial = dir.join("vrfkit.exe.partial");
        extract_file(&zip_path, "vrfkit.exe", &partial)?;
        check_sha256(&partial, &pin.exe_sha256, "vrfkit.exe")?;
        fs::rename(&partial, &exe).with_context(|| format!("moving vrfkit into {}", exe.display()))
    })();
    let _ = fs::remove_file(&zip_path);
    result.map(|()| true)
}

/// The winget package install.ps1 and `/update` install ffmpeg from.
pub const FFMPEG_PACKAGE: &str = "Gyan.FFmpeg";

/// Installs ffmpeg with winget unless it is already found. True when it installed.
pub fn ensure_ffmpeg() -> Result<bool> {
    if crate::clips::ffmpeg().is_some() {
        return Ok(false);
    }
    let mut cmd = std::process::Command::new("winget");
    cmd.args([
        "install",
        "--id",
        FFMPEG_PACKAGE,
        "--exact",
        "--silent",
        "--accept-source-agreements",
        "--accept-package-agreements",
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: winget's progress would draw over the shell.
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd
        .output()
        .context("couldn't run winget to install ffmpeg; install it with `winget install Gyan.FFmpeg`")?;
    if !out.status.success() || crate::clips::ffmpeg().is_none() {
        let said = String::from_utf8_lossy(&out.stdout);
        let last = said
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .unwrap_or("no output");
        bail!("winget couldn't install ffmpeg ({last})");
    }
    Ok(true)
}

/// Who's asking, for requests.
const AGENT: &str = concat!("vantage/", env!("CARGO_PKG_VERSION"));

pub fn http_get(url: &str) -> Result<ureq::http::Response<ureq::Body>> {
    abalone::install::http_get(url, AGENT)
}

/// Downloads `url` to `dest` (through a temporary name).
pub fn download(url: &str, dest: &Path) -> Result<()> {
    abalone::install::download(url, dest, AGENT)
}

/// Extracts the entry whose file name is `name` from the ZIP at `zip_path`.
fn extract_file(zip_path: &Path, name: &str, dest: &Path) -> Result<()> {
    let file = fs::File::open(zip_path).with_context(|| format!("opening {}", zip_path.display()))?;
    let mut zip = zip::ZipArchive::new(file).with_context(|| format!("reading {}", zip_path.display()))?;
    let index = (0..zip.len())
        .find(|&i| {
            zip.by_index(i)
                .ok()
                .is_some_and(|e| e.is_file() && e.name().is_ok_and(|n| n.rsplit(['/', '\\']).next() == Some(name)))
        })
        .with_context(|| format!("{} has no {name}", zip_path.display()))?;
    let mut entry = zip.by_index(index)?;
    let mut out = fs::File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    std::io::copy(&mut entry, &mut out).with_context(|| format!("extracting {name}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vrfkit_lives_beside_the_versions() {
        let layout = Layout::at("C:/v", APP);
        assert_eq!(
            vrfkit_exe(&layout, "v0.4.0"),
            Path::new("C:/v/vrfkit/v0.4.0/vrfkit.exe")
        );
        assert_eq!(layout.launcher(), Path::new("C:/v/bin").join(layout.exe_name()));
    }
}
