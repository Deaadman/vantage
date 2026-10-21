//! Updates from GitHub releases. Each release carries `manifest.json`:
//!
//! ```json
//! {"version": "0.2.0", "platforms": {"windows-x64": {"file": "vantage-windows-x64.exe", "sha256": "…"}},
//!  "notes": ["What's new, one line each"],
//!  "vrfkit": {"tag": "v0.4.0", "notes": ["What's new in the vrfkit it pins"]}}
//! ```
//!
//! `notes` and `vrfkit` are optional, written at release time from
//! CHANGELOG.md and vrfkit.json; an update saves them so the first launch of
//! the new version can say what's new.
//!
//! vrfkit also updates on its own: `latest_pin` reads `vrfkit.json` from
//! `main`, and `install::update_vrfkit` switches to a newer release it names.
//!
//! An update downloads the new exe into `versions\<version>`, checks its hash,
//! and runs it as `vantage install --update`: the new version installs
//! itself and the vrfkit release it pins (see `install`). The running
//! Vantage is untouched until it exits.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::install::{self, Layout};
use crate::vrfkit::Pin;

pub const REPO: &str = "Deaadman/vantage";
pub const PLATFORM: &str = "windows-x64";
/// Set to `1` to stop the shell's background check; `vantage update` still works.
pub const DISABLE_ENV: &str = "VANTAGE_DISABLE_AUTOUPDATER";
/// A base URL serving `manifest.json` and the files it names, in place of
/// GitHub releases (for testing an update end to end).
pub const URL_ENV: &str = "VANTAGE_UPDATE_URL";

/// One update at a time per process (the shell's background check and `/update`).
static BUSY: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub platforms: BTreeMap<String, Asset>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub vrfkit: Option<VrfkitNotes>,
}

/// The vrfkit a release pins, and what's new in it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VrfkitNotes {
    pub tag: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

pub const RELEASES: &str = "github.com/Deaadman/vantage/releases/tag";
pub const VRFKIT_RELEASES: &str = "github.com/yakisoba0728/vrfkit/releases/tag";

/// What an installed update brings, shown once on its first launch.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WhatsNew {
    pub version: String,
    pub notes: Vec<String>,
    /// Set when the update brings a different vrfkit than the one before.
    pub vrfkit: Option<VrfkitNotes>,
}

impl WhatsNew {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("whats-new.json")
    }

    /// Saved by the version doing the update, which knows the vrfkit it had.
    pub fn save(data_dir: &Path, manifest: &Manifest) {
        let w = WhatsNew {
            version: manifest.version.clone(),
            notes: manifest.notes.clone(),
            vrfkit: manifest
                .vrfkit
                .clone()
                .filter(|v| install::is_newer(&v.tag, &crate::vrfkit::active().tag)),
        };
        if let Ok(json) = serde_json::to_string_pretty(&w) {
            let _ = std::fs::write(Self::path(data_dir), json);
        }
    }

    /// The notes for `version` (this build), removed so they show once.
    pub fn take(data_dir: &Path, version: &str) -> Option<Self> {
        let path = Self::path(data_dir);
        let w: Self = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
        if !install::is_newer(version, &w.version) {
            let _ = std::fs::remove_file(&path);
        }
        (w.version == version).then_some(w)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub file: String,
    pub sha256: String,
}

impl Manifest {
    pub fn asset(&self) -> Result<&Asset> {
        self.platforms
            .get(PLATFORM)
            .with_context(|| format!("release {} has no {PLATFORM} build", self.version))
    }
}

/// Why the background check is off, if it is.
pub fn auto_disabled(config_auto_update: Option<bool>) -> Option<String> {
    if std::env::var_os(DISABLE_ENV).is_some_and(|v| !v.is_empty() && v != "0") {
        return Some(format!("disabled by {DISABLE_ENV}"));
    }
    (config_auto_update == Some(false)).then(|| "disabled by auto_update = false in config.toml".to_owned())
}

fn override_base() -> Option<String> {
    std::env::var(URL_ENV)
        .ok()
        .filter(|u| !u.is_empty())
        .map(|u| u.trim_end_matches('/').to_owned())
}

/// The newest release's manifest.
pub fn latest() -> Result<Manifest> {
    let url = match override_base() {
        Some(base) => format!("{base}/manifest.json"),
        None => format!("https://github.com/{REPO}/releases/latest/download/manifest.json"),
    };
    let mut resp = install::http_get(&url)?;
    let text = resp
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading {url}"))?;
    let manifest: Manifest = serde_json::from_str(&text).with_context(|| format!("parsing {url}"))?;
    if install::version_key(&manifest.version).iter().all(|&n| n == 0) {
        bail!("{url} names no usable version ({:?})", manifest.version);
    }
    Ok(manifest)
}

/// Where vrfkit releases are downloaded from; a pin naming anywhere else is refused.
const VRFKIT_DOWNLOADS: &str = "https://github.com/yakisoba0728/vrfkit/releases/download/";

/// The vrfkit `main` names in `vrfkit.json`: merging a change to it updates
/// vrfkit everywhere, without a Vantage release.
pub fn latest_pin() -> Result<Pin> {
    let (url, any_host) = match override_base() {
        Some(base) => (format!("{base}/vrfkit.json"), true),
        None => (
            format!("https://raw.githubusercontent.com/{REPO}/main/vrfkit.json"),
            false,
        ),
    };
    let mut resp = install::http_get(&url)?;
    let text = resp
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading {url}"))?;
    let pin: Pin = serde_json::from_str(&text).with_context(|| format!("parsing {url}"))?;
    check_pin(&pin, any_host).with_context(|| format!("{url} names an unusable vrfkit"))?;
    Ok(pin)
}

fn check_pin(pin: &Pin, any_host: bool) -> Result<()> {
    let hex = |h: &str| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit());
    if !hex(&pin.zip_sha256) || !hex(&pin.exe_sha256) {
        bail!("its hashes aren't SHA-256");
    }
    if !any_host && !pin.zip_url.starts_with(VRFKIT_DOWNLOADS) {
        bail!("{} isn't a vrfkit release download", pin.zip_url);
    }
    if pin.decodes().newest == 0 {
        bail!("its builds {:?} aren't versions", pin.builds);
    }
    Ok(())
}

/// Where a release's file is downloaded from: the tag's own URL, so a release
/// published mid-update cannot mix files from two versions.
fn file_url(manifest: &Manifest, file: &str) -> String {
    match override_base() {
        Some(base) => format!("{base}/{file}"),
        None => format!(
            "https://github.com/{REPO}/releases/download/{}/{file}",
            manifest.version
        ),
    }
}

/// Downloads, verifies and installs `manifest`'s version by running it as
/// `install --update`. Returns the installed exe.
pub fn apply(layout: &Layout, manifest: &Manifest, data_dir: &Path) -> Result<PathBuf> {
    let _busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    let asset = manifest.asset()?;
    let exe = layout.version_exe(&manifest.version);
    let present = exe.is_file() && install::check_sha256(&exe, &asset.sha256, "").is_ok();
    if !present {
        let dir = exe.parent().expect("version_exe has a parent");
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        install::download(&file_url(manifest, &asset.file), &exe)?;
        install::check_sha256(&exe, &asset.sha256, &asset.file)?;
    }
    let output = Command::new(&exe)
        .arg("install")
        .arg("--update")
        .arg("--data-dir")
        .arg(data_dir)
        .env(crate::paths::HOME_ENV, &layout.root)
        .output()
        .with_context(|| format!("running {}", exe.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let last = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no error output");
        bail!("Vantage {} failed to install itself: {last}", manifest.version);
    }
    WhatsNew::save(data_dir, manifest);
    Ok(exe)
}

/// The last update check, for `vantage status`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub checked_at: String,
    pub result: String,
}

impl State {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("update.json")
    }

    pub fn load(data_dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(Self::path(data_dir)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Best effort: a failed write only loses the status line.
    pub fn record(data_dir: &Path, result: impl Into<String>) {
        let state = State {
            checked_at: crate::time::now_rfc3339(),
            result: result.into(),
        };
        if std::fs::create_dir_all(data_dir).is_ok()
            && let Ok(json) = serde_json::to_string_pretty(&state)
        {
            let _ = std::fs::write(Self::path(data_dir), json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_manifest() {
        let m: Manifest = serde_json::from_str(
            r#"{"version":"0.2.0","platforms":{"windows-x64":{"file":"vantage-windows-x64.exe","sha256":"ab"}}}"#,
        )
        .unwrap();
        assert_eq!(m.asset().unwrap().file, "vantage-windows-x64.exe");
        assert!(file_url(&m, "x.exe").ends_with("/releases/download/0.2.0/x.exe") || override_base().is_some());
    }

    #[test]
    fn reads_the_notes_the_release_workflow_writes() {
        // As .github/workflows/release.yml writes it from CHANGELOG.md, with no vrfkit notes.
        let m: Manifest = serde_json::from_str(
            r#"{
                "version": "0.2.0",
                "platforms": {"windows-x64": {"file": "vantage-windows-x64.exe", "sha256": "ab"}},
                "notes": ["Habits are grouped – four groups.", "One more."],
                "vrfkit": {"tag": "v0.3.0", "notes": []}
            }"#,
        )
        .unwrap();
        assert_eq!(m.notes, ["Habits are grouped – four groups.", "One more."]);
        let v = m.vrfkit.as_ref().unwrap();
        assert_eq!((v.tag.as_str(), v.notes.len()), ("v0.3.0", 0));

        // Saved for the next launch: the vrfkit notes only when the update brings a newer one.
        let dir = std::env::temp_dir().join(format!("vantage-whats-new-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        WhatsNew::save(&dir, &m);
        let w = WhatsNew::take(&dir, "0.2.0").unwrap();
        assert_eq!(w.notes, m.notes);
        assert_eq!(
            w.vrfkit.is_some(),
            install::is_newer(&v.tag, &crate::vrfkit::active().tag)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checks_a_pin_before_using_it() {
        let good = crate::vrfkit::bundled().clone();
        assert!(check_pin(&good, false).is_ok());
        let elsewhere = Pin {
            zip_url: "https://example.com/vrfkit.zip".into(),
            ..good.clone()
        };
        assert!(check_pin(&elsewhere, false).is_err());
        assert!(check_pin(&elsewhere, true).is_ok());
        let short = Pin {
            exe_sha256: "abc".into(),
            ..good.clone()
        };
        assert!(check_pin(&short, false).is_err());
        let no_builds = Pin {
            builds: ["?".into(), "?".into()],
            ..good
        };
        assert!(check_pin(&no_builds, false).is_err());
    }
}
