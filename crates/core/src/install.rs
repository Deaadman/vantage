//! The installed copy in `%LOCALAPPDATA%\vantage` (or `$VANTAGE_HOME`):
//!
//! - `versions\<version>\vantage.exe`: every installed version.
//! - `bin\vantage.exe`: a copy of the current one; `bin` is on PATH.
//! - `vrfkit\<tag>\vrfkit.exe`: the vrfkit release each version pins.
//!
//! A version installs itself (`vantage install`): it copies itself into
//! `versions`, fetches its vrfkit, and swaps the copy in `bin`. Windows lets a
//! running exe be renamed but not overwritten, so the old launcher is renamed
//! aside first and deleted on a later install. A running Vantage keeps
//! working; the next launch starts the new version.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::vrfkit::{PINNED, sha256_file};

/// Installed versions and vrfkit releases kept besides the current one.
const KEEP_OLDER: usize = 1;
/// Larger than any release asset; a guard against a runaway response.
const MAX_DOWNLOAD: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn new() -> Result<Self> {
        Ok(Layout {
            root: crate::paths::install_root()?,
        })
    }

    pub fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }

    /// `bin\vantage.exe`, the one on PATH.
    pub fn launcher(&self) -> PathBuf {
        self.bin().join("vantage.exe")
    }

    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }

    pub fn version_exe(&self, version: &str) -> PathBuf {
        self.versions().join(version).join("vantage.exe")
    }

    pub fn vrfkit_exe(&self, tag: &str) -> PathBuf {
        self.root.join("vrfkit").join(tag).join("vrfkit.exe")
    }

    /// True when `exe` is the launcher or an installed version, i.e. this
    /// Vantage was installed rather than built or copied somewhere else.
    pub fn contains(&self, exe: &Path) -> bool {
        let (Ok(exe), Ok(root)) = (fs::canonicalize(exe), fs::canonicalize(&self.root)) else {
            return false;
        };
        exe.starts_with(root.join("bin")) || exe.starts_with(root.join("versions"))
    }
}

#[derive(Debug, Default)]
pub struct Installed {
    /// vrfkit was downloaded (not already present).
    pub vrfkit_downloaded: bool,
}

/// Installs `exe` (the running binary, version `version`): copies it into
/// `versions`, makes sure its pinned vrfkit is there, points `bin` at it and
/// removes old versions.
pub fn install(layout: &Layout, version: &str, exe: &Path) -> Result<Installed> {
    let target = layout.version_exe(version);
    if !same_file(exe, &target) {
        copy_verified(exe, &target)?;
    }
    let vrfkit_downloaded = ensure_vrfkit(layout)?;
    place_launcher(layout, &target)?;
    cleanup(layout, version);
    Ok(Installed { vrfkit_downloaded })
}

/// Downloads and verifies the pinned vrfkit unless it is already installed.
/// True when it downloaded.
pub fn ensure_vrfkit(layout: &Layout) -> Result<bool> {
    let exe = layout.vrfkit_exe(PINNED.tag);
    if exe.is_file() && sha256_file(&exe)? == PINNED.exe_sha256 {
        return Ok(false);
    }
    let dir = exe.parent().expect("vrfkit_exe has a parent");
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let zip_path = dir.join("download.zip");
    download(PINNED.zip_url, &zip_path)?;
    let result = (|| {
        check_sha256(&zip_path, PINNED.zip_sha256, "the vrfkit release ZIP")?;
        let partial = dir.join("vrfkit.exe.partial");
        extract_file(&zip_path, "vrfkit.exe", &partial)?;
        check_sha256(&partial, PINNED.exe_sha256, "vrfkit.exe")?;
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

/// Makes `bin\vantage.exe` a copy of `src`. A launcher that is running
/// cannot be overwritten, so it is renamed aside first.
fn place_launcher(layout: &Layout, src: &Path) -> Result<()> {
    let launcher = layout.launcher();
    if launcher.is_file() && sha256_file(&launcher)? == sha256_file(src)? {
        return Ok(());
    }
    fs::create_dir_all(layout.bin()).with_context(|| format!("creating {}", layout.bin().display()))?;
    let staged = layout.bin().join("vantage.exe.new");
    fs::copy(src, &staged).with_context(|| format!("copying {} to {}", src.display(), staged.display()))?;
    if launcher.exists() {
        let aside = layout.bin().join(format!("vantage.exe.old-{}", unique_suffix()));
        fs::rename(&launcher, &aside).with_context(|| format!("moving the old {} aside", launcher.display()))?;
    }
    fs::rename(&staged, &launcher).with_context(|| format!("placing {}", launcher.display()))
}

/// Best effort: deletes launchers renamed aside (those still running stay
/// locked and are retried next time), and all but the newest older version
/// and vrfkit release.
pub fn cleanup(layout: &Layout, current: &str) {
    if let Ok(entries) = fs::read_dir(layout.bin()) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("vantage.exe.old-") {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    prune(&layout.versions(), current);
    prune(&layout.root.join("vrfkit"), PINNED.tag);
}

/// Removes subdirectories of `dir` except `keep` and the `KEEP_OLDER` highest
/// other versions. A directory whose exe is running cannot be removed and is
/// left for next time.
fn prune(dir: &Path, keep: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut others: Vec<(Vec<u64>, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().is_dir() && e.file_name() != keep)
        .map(|e| (version_key(&e.file_name().to_string_lossy()), e.path()))
        .collect();
    others.sort();
    let n = others.len().saturating_sub(KEEP_OLDER);
    for (_, path) in others.into_iter().take(n) {
        let _ = fs::remove_dir_all(path);
    }
}

/// `v1.2.3` or `1.2.3-beta` as numbers for ordering; non-numeric parts are 0.
pub fn version_key(v: &str) -> Vec<u64> {
    v.trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// True when `candidate` is a higher version than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    version_key(candidate) > version_key(current)
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((fs::canonicalize(a), fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

/// Copies `src` to `dst` through a temporary name, so `dst` is never half written.
fn copy_verified(src: &Path, dst: &Path) -> Result<()> {
    if dst.is_file() && sha256_file(dst)? == sha256_file(src)? {
        return Ok(());
    }
    let dir = dst.parent().expect("versioned path has a parent");
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let partial = dst.with_extension("exe.partial");
    fs::copy(src, &partial).with_context(|| format!("copying {} to {}", src.display(), partial.display()))?;
    fs::rename(&partial, dst).with_context(|| format!("placing {}", dst.display()))
}

pub fn check_sha256(path: &Path, expected: &str, what: &str) -> Result<()> {
    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(expected.trim()) {
        let _ = fs::remove_file(path);
        bail!("{what} has SHA-256 {actual}, expected {expected}; not installed");
    }
    Ok(())
}

pub fn http_get(url: &str) -> Result<ureq::http::Response<ureq::Body>> {
    ureq::get(url)
        .header("User-Agent", concat!("vantage/", env!("CARGO_PKG_VERSION")))
        .config()
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .call()
        .with_context(|| format!("GET {url}"))
}

/// Downloads `url` to `dest` (through a temporary name).
pub fn download(url: &str, dest: &Path) -> Result<()> {
    let mut resp = http_get(url)?;
    let partial = dest.with_extension("download");
    let mut file = fs::File::create(&partial).with_context(|| format!("creating {}", partial.display()))?;
    let mut reader = resp.body_mut().with_config().limit(MAX_DOWNLOAD).reader();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = reader.read(&mut buf).with_context(|| format!("downloading {url}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
    }
    file.sync_all()?;
    drop(file);
    fs::rename(&partial, dest).with_context(|| format!("placing {}", dest.display()))
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

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{nanos}-{}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vantage-install-{name}-{}", unique_suffix()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn versions_order_numerically() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("v1.0.0", "0.99.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0-beta", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1"));
    }

    #[test]
    fn swaps_the_launcher_and_prunes_old_versions() {
        let root = temp("swap");
        let layout = Layout { root: root.clone() };
        for v in ["0.1.0", "0.2.0", "0.3.0"] {
            let exe = layout.version_exe(v);
            fs::create_dir_all(exe.parent().unwrap()).unwrap();
            fs::write(&exe, v).unwrap();
        }
        place_launcher(&layout, &layout.version_exe("0.2.0")).unwrap();
        place_launcher(&layout, &layout.version_exe("0.3.0")).unwrap();
        assert_eq!(fs::read_to_string(layout.launcher()).unwrap(), "0.3.0");
        // The replaced launcher was moved aside, as Windows needs when it runs.
        let aside = |l: &Layout| {
            fs::read_dir(l.bin())
                .unwrap()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("vantage.exe.old-"))
                .count()
        };
        assert_eq!(aside(&layout), 1);

        cleanup(&layout, "0.3.0");
        assert_eq!(aside(&layout), 0);
        assert!(layout.version_exe("0.3.0").is_file());
        assert!(layout.version_exe("0.2.0").is_file());
        assert!(!layout.version_exe("0.1.0").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copying_in_is_idempotent() {
        let root = temp("self");
        let layout = Layout { root: root.clone() };
        let src = root.join("built.exe");
        fs::write(&src, b"vantage").unwrap();
        // `install` minus ensure_vrfkit, which downloads.
        copy_verified(&src, &layout.version_exe("9.9.9")).unwrap();
        copy_verified(&src, &layout.version_exe("9.9.9")).unwrap();
        place_launcher(&layout, &layout.version_exe("9.9.9")).unwrap();
        place_launcher(&layout, &layout.version_exe("9.9.9")).unwrap();
        assert_eq!(fs::read(layout.launcher()).unwrap(), b"vantage");
        assert!(layout.contains(&layout.launcher()));
        assert!(!layout.contains(&src));
        fs::remove_dir_all(root).unwrap();
    }
}
