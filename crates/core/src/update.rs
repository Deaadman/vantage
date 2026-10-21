//! Updates from GitHub releases, through Abalone's updater
//! (`abalone::update`). Each release's `manifest.json` also names the vrfkit
//! it pins, and what's new in it:
//!
//! ```json
//! {"version": "0.2.0", "platforms": {"windows-x64": {"file": "vantage-windows-x64.exe", "sha256": "…"}},
//!  "notes": ["What's new, one line each"],
//!  "vrfkit": {"tag": "v0.4.0", "notes": ["What's new in the vrfkit it pins"]}}
//! ```
//!
//! An update keeps the vrfkit notes for "what's new" only when it brings a
//! newer vrfkit than the one in use.
//!
//! vrfkit also updates on its own: `latest_pin` reads `vrfkit.json` from
//! `main`, and `install::update_vrfkit` switches to a newer release it names.

use std::path::Path;

pub use abalone::update::{Manifest, State, Updated, Updater, WhatsNew, news};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Map;

use crate::install;
use crate::vrfkit::Pin;

pub const REPO: &str = "Deaadman/vantage";
pub const RELEASES: &str = "github.com/Deaadman/vantage/releases/tag";
pub const VRFKIT_RELEASES: &str = "github.com/yakisoba0728/vrfkit/releases/tag";

/// The vrfkit a release pins, and what's new in it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VrfkitNotes {
    pub tag: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// The vrfkit notes an update kept, if it brought a newer vrfkit.
pub fn vrfkit_notes(w: &WhatsNew) -> Option<VrfkitNotes> {
    serde_json::from_value(w.extra.get("vrfkit")?.clone()).ok()
}

/// This Vantage's updater, for `version` (this build), keeping its data in `data_dir`.
pub fn updater(data_dir: &Path, version: &str) -> Result<Updater> {
    Ok(Updater::new("Vantage", install::APP, REPO, version, data_dir)?
        .data_dir_flag("--data-dir")
        .keep_extra(|m| keep_vrfkit(m, &crate::vrfkit::active().tag)))
}

/// What an update keeps for "what's new": the vrfkit notes, when the vrfkit
/// is newer than `running` (the one the version doing the update uses).
fn keep_vrfkit(m: &Manifest, running: &str) -> Map<String, serde_json::Value> {
    let mut keep = Map::new();
    if let Some(v) = m.extra.get("vrfkit")
        && let Ok(n) = serde_json::from_value::<VrfkitNotes>(v.clone())
        && install::is_newer(&n.tag, running)
    {
        keep.insert("vrfkit".into(), v.clone());
    }
    keep
}

/// Where vrfkit releases are downloaded from; a pin naming anywhere else is refused.
const VRFKIT_DOWNLOADS: &str = "https://github.com/yakisoba0728/vrfkit/releases/download/";

/// The vrfkit `main` names in `vrfkit.json`: merging a change to it updates
/// vrfkit everywhere, without a Vantage release.
pub fn latest_pin(updater: &Updater) -> Result<Pin> {
    let url = updater.main_file("vrfkit.json");
    let any_host = updater.override_base().is_some();
    let text = updater.get_text(&url)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_notes_the_release_workflow_writes() {
        // As .github/workflows/release.yml writes it from CHANGELOG.md.
        let m: Manifest = serde_json::from_str(
            r#"{
                "version": "0.2.0",
                "platforms": {"windows-x64": {"file": "vantage-windows-x64.exe", "sha256": "ab"}},
                "notes": ["Habits are grouped – four groups.", "One more."],
                "vrfkit": {"tag": "v0.3.0", "notes": ["Reads 13.10"]}
            }"#,
        )
        .unwrap();
        assert_eq!(m.notes, ["Habits are grouped – four groups.", "One more."]);
        assert_eq!(m.asset("windows-x64").unwrap().file, "vantage-windows-x64.exe");

        // Saved for the next launch: the vrfkit notes only when the update brings a newer one.
        let w = |running: &str| WhatsNew {
            version: m.version.clone(),
            notes: m.notes.clone(),
            extra: keep_vrfkit(&m, running),
        };
        let kept = vrfkit_notes(&w("v0.2.0")).unwrap();
        assert_eq!((kept.tag.as_str(), kept.notes.len()), ("v0.3.0", 1));
        assert!(vrfkit_notes(&w("v0.3.0")).is_none());
        assert!(updater(Path::new("."), "0.1.0").is_ok());
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
