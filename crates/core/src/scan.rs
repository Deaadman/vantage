//! `scan`: find new replays, export them with vrfkit, store the facts, and
//! delete the raw export.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::builds::{BuildSupport, check_build};
use crate::catalog::Catalog;
use crate::game::Game;
use crate::store::{FileState, Store};
use crate::tracks::write_tracks;
use crate::vrfkit::{CouldNotRun, Vrfkit};
use crate::vrfkit_export::Export;

pub struct ScanOptions {
    pub demos_dir: PathBuf,
    pub data_dir: PathBuf,
    /// Keep `raw/<replay>/` (about 45 MB per match) instead of deleting it.
    pub keep_raw: bool,
    /// Try files that failed before again.
    pub retry_failed: bool,
}

#[derive(Debug)]
pub enum Outcome {
    Stored {
        match_id: String,
        map: String,
        warnings: Vec<String>,
    },
    AlreadyStored,
    PreviouslyFailed {
        detail: String,
    },
    Duplicate {
        match_id: String,
    },
    /// The replay's build is outside what this vrfkit decodes; nothing was exported.
    Unsupported {
        reason: String,
    },
    /// Skipped: unsupported last time with the same vrfkit.
    PreviouslyUnsupported {
        reason: String,
    },
    Failed {
        error: String,
    },
    /// Skipped for now: changed moments ago, so it may still be downloading.
    NotReady,
}

/// How long a replay must be left alone before it's read.
const SETTLE: std::time::Duration = std::time::Duration::from_secs(15);

/// Whether `vrf` has been left alone for `SETTLE`, so it's done downloading.
pub fn settled(vrf: &Path) -> bool {
    let age = std::fs::metadata(vrf)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| m.elapsed().ok());
    !age.is_some_and(|a| a < SETTLE)
}

#[derive(Debug)]
pub struct FileOutcome {
    pub file: String,
    pub outcome: Outcome,
}

pub fn raw_dir(data_dir: &Path, vrf: &Path) -> PathBuf {
    data_dir.join("raw").join(vrf.file_stem().unwrap_or_default())
}

pub fn tracks_path(data_dir: &Path, match_id: &str) -> PathBuf {
    data_dir.join("tracks").join(format!("{match_id}.parquet"))
}

/// Replays in the demos dir, oldest name first.
pub fn find_replays(demos_dir: &Path) -> Result<Vec<PathBuf>> {
    if !demos_dir.is_dir() {
        bail!(
            "replay folder {} does not exist. Download a replay in the client first, or set demos_dir with `vantage init --demos-dir`",
            demos_dir.display()
        );
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(demos_dir)
        .with_context(|| format!("listing {}", demos_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("vrf")))
        .collect();
    files.sort();
    Ok(files)
}

/// Scans every replay; `on_file` is called after each one so the CLI can
/// print progress. Only setup errors abort; per-file errors become `Failed`.
pub fn scan(
    store: &mut Store,
    vrfkit: &Vrfkit,
    catalog: &Catalog,
    opts: &ScanOptions,
    mut on_file: impl FnMut(&FileOutcome),
) -> Result<Vec<FileOutcome>> {
    let mut out = Vec::new();
    for vrf in find_replays(&opts.demos_dir)? {
        let file = vrf.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let outcome = scan_one(store, vrfkit, catalog, opts, &vrf, &file).unwrap_or_else(|e| Outcome::Failed {
            error: format!("{e:#}"),
        });
        let fo = FileOutcome { file, outcome };
        on_file(&fo);
        out.push(fo);
    }
    Ok(out)
}

fn scan_one(
    store: &mut Store,
    vrfkit: &Vrfkit,
    catalog: &Catalog,
    opts: &ScanOptions,
    vrf: &Path,
    file: &str,
) -> Result<Outcome> {
    let size = std::fs::metadata(vrf)
        .with_context(|| format!("reading {}", vrf.display()))?
        .len();
    // A replay changed moments ago may still be downloading; the next scan gets it.
    if !settled(vrf) {
        return Ok(Outcome::NotReady);
    }
    match store.file_state(file, size)? {
        FileState::Stored { .. } => return Ok(Outcome::AlreadyStored),
        // Reported when first found; quiet afterwards.
        FileState::Logged { status, .. } if status == "duplicate" => return Ok(Outcome::AlreadyStored),
        // A different vrfkit may decode what this one could not, so retry then.
        FileState::Logged {
            status,
            detail,
            vrfkit_version,
        } if !opts.retry_failed && vrfkit_version == vrfkit.version => {
            return Ok(match status.as_str() {
                "unsupported" => Outcome::PreviouslyUnsupported { reason: detail },
                _ => Outcome::PreviouslyFailed { detail },
            });
        }
        _ => {}
    }

    // The header check costs ~50 ms and spares a doomed full export.
    match vrfkit.inspect_build(vrf) {
        Ok(build) => {
            if let BuildSupport::Unsupported(reason) = check_build(&build, &vrfkit.decodes) {
                store.log_file(file, size, "unsupported", &reason, &vrfkit.version)?;
                return Ok(Outcome::Unsupported { reason });
            }
        }
        Err(e) => return failed(store, vrfkit, file, size, e),
    }

    let raw = raw_dir(&opts.data_dir, vrf);
    // A bug on one odd replay must not end the whole scan.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        export_and_store(store, vrfkit, catalog, opts, vrf, file, size, &raw)
    }))
    .unwrap_or_else(|panic| {
        let what = panic
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        Err(anyhow::anyhow!("internal error in Vantage (please report): {what}"))
    });
    if !opts.keep_raw && raw.exists() {
        // Our own scratch directory under the data dir; nothing else lives there.
        let _ = std::fs::remove_dir_all(&raw);
    }
    match result {
        Ok(outcome) => Ok(outcome),
        Err(e) => failed(store, vrfkit, file, size, e),
    }
}

/// A replay that couldn't be read. It's logged so later scans skip it, unless
/// vrfkit never got to read it: then it's tried again next time.
fn failed(store: &Store, vrfkit: &Vrfkit, file: &str, size: u64, e: anyhow::Error) -> Result<Outcome> {
    let error = format!("{e:#}");
    if !e.chain().any(|c| c.is::<CouldNotRun>()) {
        store.log_file(file, size, "failed", &error, &vrfkit.version)?;
    }
    Ok(Outcome::Failed { error })
}

#[allow(clippy::too_many_arguments)]
fn export_and_store(
    store: &mut Store,
    vrfkit: &Vrfkit,
    catalog: &Catalog,
    opts: &ScanOptions,
    vrf: &Path,
    file: &str,
    size: u64,
    raw: &Path,
) -> Result<Outcome> {
    if raw.exists() {
        std::fs::remove_dir_all(raw).with_context(|| format!("clearing old export {}", raw.display()))?;
    }
    vrfkit.export(vrf, raw)?;
    let export = Export::load(raw)?;

    let mut warnings = Vec::new();
    if let BuildSupport::Unsupported(s) = check_build(&export.manifest.replay_build, &vrfkit.decodes) {
        bail!("{s}");
    }
    if store.has_match(&export.manifest.match_id)? {
        let match_id = export.manifest.match_id.clone();
        store.log_file(file, size, "duplicate", &match_id, &vrfkit.version)?;
        return Ok(Outcome::Duplicate { match_id });
    }

    let game = Game::new(export);
    // Deathmatch and other modes without the spike and two teams aren't reviewed.
    let mode = game.mode_folder().map(|f| catalog.mode_name(f));
    if let Some(reason) = game.unsupported_mode_named(mode.as_deref()) {
        store.log_file(file, size, "unsupported", &reason, &vrfkit.version)?;
        return Ok(Outcome::Unsupported { reason });
    }
    warnings.extend(game.warnings.iter().cloned());
    let ctx = store.insert_match(&game, catalog, file, size, &vrfkit.version)?;
    if let Err(e) = write_tracks(&game, &tracks_path(&opts.data_dir, &ctx.match_id)) {
        warnings.push(format!("tracks not written: {e:#}"));
    }
    Ok(Outcome::Stored {
        map: ctx.map_name.or(ctx.map_url).unwrap_or_else(|| "unknown map".into()),
        match_id: ctx.match_id,
        warnings,
    })
}
