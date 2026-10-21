//! Runs vrfkit as a subprocess, and picks the release to run.
//!
//! `vrfkit.json` at the repo root names the release. Each build has it built
//! in, and installed copies also fetch the newest one from `main` (see
//! `update::latest_pin`), so a new vrfkit reaches players without a Vantage
//! release. A vrfkit installed that way saves its pin as `vrfkit\pin.json`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::builds::Decodes;
pub use crate::install::sha256_file;
use crate::install::{Layout, is_newer};

/// One vrfkit release: where it comes from, its hashes, and what it decodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub tag: String,
    pub zip_url: String,
    pub zip_sha256: String,
    /// SHA-256 of `vrfkit.exe` inside the ZIP.
    pub exe_sha256: String,
    /// The oldest and newest VALORANT builds it decodes, e.g. `["11.06", "13.08"]`.
    pub builds: [String; 2],
    /// What players see when it arrives as an update, one line each.
    #[serde(default)]
    pub notes: Vec<String>,
    /// The oldest Vantage that reads its output.
    pub min_vantage: String,
}

impl Pin {
    pub fn decodes(&self) -> Decodes {
        let code = |b: &str| crate::builds::parse_version(b).unwrap_or(0);
        Decodes {
            tag: self.tag.clone(),
            oldest: code(&self.builds[0]),
            newest: code(&self.builds[1]),
        }
    }

    /// Where an update saves the pin it installed.
    pub fn saved_path(layout: &Layout) -> PathBuf {
        layout.root.join("vrfkit").join("pin.json")
    }

    fn load_saved(layout: &Layout) -> Option<Pin> {
        serde_json::from_str(&std::fs::read_to_string(Self::saved_path(layout)).ok()?).ok()
    }

    pub fn save(&self, layout: &Layout) -> Result<()> {
        let path = Self::saved_path(layout);
        crate::files::write_atomic(&path, serde_json::to_string_pretty(self)?.as_bytes())
    }
}

/// The pin built into this Vantage: `vrfkit.json` when it was built.
pub fn bundled() -> &'static Pin {
    static BUNDLED: OnceLock<Pin> = OnceLock::new();
    BUNDLED.get_or_init(|| serde_json::from_str(include_str!("../../../vrfkit.json")).expect("vrfkit.json parses"))
}

/// The pin in use: one a vrfkit update saved, when it's newer than the
/// built-in one, else the built-in one.
pub fn active() -> Pin {
    newest(crate::install::layout().ok().and_then(|l| Pin::load_saved(&l)))
}

fn newest(saved: Option<Pin>) -> Pin {
    match saved {
        Some(p) if !is_newer(&bundled().tag, &p.tag) => p,
        _ => bundled().clone(),
    }
}

#[derive(Debug, Clone)]
pub struct Vrfkit {
    pub path: PathBuf,
    pub sha256: String,
    /// Release tag, or `custom` for a hash accepted through config.
    pub version: String,
    /// The replay builds it decodes; a custom build is taken to decode what
    /// the pin in use does.
    pub decodes: Decodes,
}

impl Vrfkit {
    /// Finds vrfkit (configured path, else the installed pinned release, else
    /// `vrfkit.exe` on PATH) and checks its hash.
    pub fn locate(configured: Option<&Path>, accepted_sha256: Option<&str>) -> Result<Self> {
        let path = match configured {
            Some(p) => p.to_path_buf(),
            None => installed_path()
                .filter(|p| p.is_file())
                .or_else(find_on_path)
                .context("vrfkit not found: run `vantage update` to install it")?,
        };
        if !path.is_file() {
            bail!(
                "vrfkit not found at {}; fix vrfkit in config.toml, or remove it to use the one `vantage update` installs",
                path.display()
            );
        }
        let sha256 = sha256_file(&path)?;
        let active = active();
        let known = [&active, bundled()];
        let (version, decodes) = if let Some(pin) = known.iter().find(|p| p.exe_sha256.eq_ignore_ascii_case(&sha256)) {
            (pin.tag.clone(), pin.decodes())
        } else if accepted_sha256.is_some_and(|h| h.eq_ignore_ascii_case(&sha256)) {
            ("custom".to_owned(), active.decodes())
        } else {
            let mut tags: Vec<&str> = known.iter().map(|p| p.tag.as_str()).collect();
            tags.dedup();
            bail!(
                "{} has SHA-256 {sha256}, which is not a vrfkit release Vantage knows ({}). \
                 If you upgraded vrfkit on purpose, set vrfkit_sha256 = \"{sha256}\" in config.toml",
                path.display(),
                tags.join(", ")
            );
        };
        Ok(Vrfkit {
            path,
            sha256,
            version,
            decodes,
        })
    }

    /// Whether the exe starts at all: `vrfkit --help` exits 0. Checked before
    /// switching to a newly installed release.
    pub fn starts(exe: &Path) -> bool {
        crate::files::output_within(Command::new(exe).arg("--help"), Duration::from_secs(30))
            .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("vrfkit"))
    }

    /// `vrfkit export <vrf> --out <out_dir>`; `out_dir` must be new or empty.
    pub fn export(&self, vrf: &Path, out_dir: &Path) -> Result<()> {
        let output = self.run(
            Command::new(&self.path)
                .arg("export")
                .arg(vrf)
                .arg("--out")
                .arg(out_dir),
            EXPORT_LIMIT,
        )?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
            let tail = tail[tail.len().saturating_sub(4)..].join(" | ");
            bail!(
                "vrfkit export failed ({}): {}",
                output.status,
                if tail.is_empty() { "no error output" } else { &tail }
            );
        }
        if !out_dir.join("manifest.json").is_file() {
            bail!("vrfkit export wrote no manifest.json");
        }
        Ok(())
    }
}

impl Vrfkit {
    /// The replay's build branch from `vrfkit inspect` (fast: header only),
    /// e.g. `++Ares-Core+release-13.08`.
    pub fn inspect_build(&self, vrf: &Path) -> Result<String> {
        let output = self.run(
            Command::new(&self.path)
                .arg("inspect")
                .arg(vrf)
                .arg("--redact-identifiers"),
            INSPECT_LIMIT,
        )?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let last = stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("no error output");
            bail!("vrfkit inspect failed ({}): {last}", output.status);
        }
        parse_branch(&String::from_utf8_lossy(&output.stdout)).context("vrfkit inspect printed no Branch line")
    }
}

/// The longest a header check may take.
const INSPECT_LIMIT: Duration = Duration::from_secs(60);
/// The longest a full export may take; a normal one takes a few seconds.
const EXPORT_LIMIT: Duration = Duration::from_secs(600);

/// vrfkit didn't run to the end: it couldn't start (antivirus holding the exe,
/// say) or it hung. Says nothing about the replay, so scan tries it again later.
#[derive(Debug)]
pub struct CouldNotRun(pub String);

impl std::fmt::Display for CouldNotRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CouldNotRun {}

impl Vrfkit {
    fn run(&self, cmd: &mut Command, limit: Duration) -> Result<std::process::Output> {
        crate::files::output_within(cmd, limit)
            .map_err(|e| anyhow::Error::new(CouldNotRun(format!("running {}: {e:#}", self.path.display()))))
    }
}

/// The `Branch:` line of `vrfkit inspect` output.
pub fn parse_branch(inspect: &str) -> Option<String> {
    inspect
        .lines()
        .find_map(|l| l.trim().strip_prefix("Branch:"))
        .map(|b| b.trim().to_owned())
        .filter(|b| !b.is_empty())
}

/// Where the installer puts the release in use.
pub fn installed_path() -> Option<PathBuf> {
    crate::install::layout()
        .ok()
        .map(|l| crate::install::vrfkit_exe(&l, &active().tag))
}

fn find_on_path() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join("vrfkit.exe"))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(tag: &str) -> Pin {
        Pin {
            tag: tag.into(),
            ..bundled().clone()
        }
    }

    #[test]
    fn the_built_in_pin_reads() {
        let p = bundled();
        assert!(p.zip_url.contains(&format!("/download/{}/", p.tag)));
        assert_eq!((p.zip_sha256.len(), p.exe_sha256.len()), (64, 64));
        let d = p.decodes();
        assert!(d.oldest > 1000 && d.oldest <= d.newest, "{:?}", p.builds);
        // CI fetches the same release, from the same file.
        assert!(include_str!("../../../.github/workflows/ci.yml").contains("vrfkit.json"));
    }

    #[test]
    fn a_saved_pin_counts_only_when_newer() {
        let tag = &bundled().tag;
        let next = format!("v{}.99.0", crate::install::version_key(tag)[0] + 1);
        assert_eq!(newest(None).tag, *tag);
        assert_eq!(newest(Some(pin(&next))).tag, next);
        // A Vantage update can bring a newer built-in pin than an older saved one.
        assert_eq!(newest(Some(pin("v0.0.1"))).tag, *tag);
    }

    #[test]
    fn branch_line() {
        let out = "=== Header ===
  Replay version:   5.3.2 (changelist 2153182552)
  Branch:           ++Ares-Core+release-13.08
";
        assert_eq!(parse_branch(out).as_deref(), Some("++Ares-Core+release-13.08"));
        assert_eq!(parse_branch("nothing"), None);
    }

    #[test]
    fn hashes_and_rejects_unknown_binaries() {
        let dir = std::env::temp_dir().join(format!("vantage-vrfkit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("vrfkit.exe");
        std::fs::write(&fake, b"abc").unwrap();
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(sha256_file(&fake).unwrap(), abc);
        assert!(Vrfkit::locate(Some(&fake), None).is_err());
        assert_eq!(Vrfkit::locate(Some(&fake), Some(abc)).unwrap().version, "custom");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
