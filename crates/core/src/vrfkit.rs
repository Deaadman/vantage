//! Runs the pinned vrfkit release as a subprocess.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// Release binaries valreview has been checked against: (tag, SHA-256 of `vrfkit.exe`).
/// v0.3.0: the release ZIP matches GitHub's asset digest
/// (sha256:58cd76fb…40df) and the exe matches its `build-info.json`.
pub const KNOWN_RELEASES: &[(&str, &str)] = &[(
    "v0.3.0",
    "1a211eebbaf0a794201baefa2b32162d74c93fec38a8db3a793c0d93429ba0ca",
)];

#[derive(Debug, Clone)]
pub struct Vrfkit {
    pub path: PathBuf,
    pub sha256: String,
    /// Release tag, or `custom` for a hash accepted through config.
    pub version: String,
}

impl Vrfkit {
    /// Finds vrfkit (configured path, else `vrfkit.exe` on PATH) and checks its hash.
    pub fn locate(configured: Option<&Path>, accepted_sha256: Option<&str>) -> Result<Self> {
        let path = match configured {
            Some(p) => p.to_path_buf(),
            None => find_on_path().context(
                "vrfkit not found: download the v0.3.0 Windows release from \
                 https://github.com/yakisoba0728/vrfkit/releases and run `valreview init --vrfkit <path to vrfkit.exe>`",
            )?,
        };
        if !path.is_file() {
            bail!(
                "vrfkit not found at {}; fix it with `valreview init --vrfkit <path>`",
                path.display()
            );
        }
        let sha256 = sha256_file(&path)?;
        let version = if let Some((tag, _)) = KNOWN_RELEASES.iter().find(|(_, h)| *h == sha256) {
            (*tag).to_owned()
        } else if accepted_sha256.is_some_and(|h| h.eq_ignore_ascii_case(&sha256)) {
            "custom".to_owned()
        } else {
            bail!(
                "{} has SHA-256 {sha256}, which is not a vrfkit release valreview knows ({}). \
                 If you upgraded vrfkit on purpose, set vrfkit_sha256 = \"{sha256}\" in config.toml",
                path.display(),
                KNOWN_RELEASES.iter().map(|(t, _)| *t).collect::<Vec<_>>().join(", ")
            );
        };
        Ok(Vrfkit { path, sha256, version })
    }

    /// `vrfkit export <vrf> --out <out_dir>`; `out_dir` must be new or empty.
    pub fn export(&self, vrf: &Path, out_dir: &Path) -> Result<()> {
        let output = Command::new(&self.path)
            .arg("export")
            .arg(vrf)
            .arg("--out")
            .arg(out_dir)
            .output()
            .with_context(|| format!("running {}", self.path.display()))?;
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

fn find_on_path() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join("vrfkit.exe"))
        .find(|p| p.is_file())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_and_rejects_unknown_binaries() {
        let dir = std::env::temp_dir().join(format!("valreview-vrfkit-{}", std::process::id()));
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
