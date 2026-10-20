//! Clips and screenshots for `/review`: find the folder a clip app saves to,
//! match each death to the clip saved just after it, and take screenshots
//! of the seconds before the death with ffmpeg.
//!
//! A clip is matched by when it was saved: instant replay saves the last
//! seconds, so a clip saved soon after a death ends just after it. The
//! death's wall-clock time is the replay's timestamp plus the death's match
//! time, which assumes the timestamp is when recording started (unverified).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};

/// A clip saved this long after a death, at most, is that death's clip.
pub const MATCH_WINDOW_S: f64 = 90.0;
const VIDEO_EXTS: [&str; 4] = ["mp4", "mkv", "mov", "webm"];

/// A clip app and the folder it saves VALORANT clips to by default.
#[derive(Debug, Clone)]
pub struct App {
    pub name: &'static str,
    pub dir: PathBuf,
    /// VALORANT clips found there.
    pub found: usize,
}

pub fn videos_dir() -> Option<PathBuf> {
    directories::UserDirs::new().and_then(|d| d.video_dir().map(Path::to_path_buf))
}

/// Where screenshots go: `Documents\valreview\Clips`.
pub fn shots_root() -> Option<PathBuf> {
    directories::UserDirs::new()
        .and_then(|d| d.document_dir().map(Path::to_path_buf))
        .map(|d| d.join("valreview").join("Clips"))
}

/// The known apps' usual folders and how many VALORANT clips each holds.
/// Only Game Bar's and ShadowPlay's defaults are fixed; the others can be
/// moved in the app, so the Videos scan catches the rest.
pub fn apps() -> Vec<App> {
    let Some(v) = videos_dir() else { return vec![] };
    [
        ("Medal", v.join("Medal").join("Clips").join("VALORANT")),
        ("NVIDIA ShadowPlay", v.join("VALORANT")),
        ("Xbox Game Bar", v.join("Captures")),
        ("AMD Adrenalin", v.join("Radeon ReLive").join("VALORANT")),
        ("Outplayed", v.join("Overwolf").join("Outplayed").join("VALORANT")),
        ("SteelSeries Moments", v.join("SteelSeries Moments")),
    ]
    .into_iter()
    .map(|(name, dir)| App {
        name,
        found: find(&dir).len(),
        dir,
    })
    .collect()
}

#[derive(Debug, Clone)]
pub struct Clip {
    pub path: PathBuf,
    /// When the file was last written: the moment the clip was saved.
    pub saved_unix: f64,
}

/// VALORANT clips in `dir` and its folders, three levels down. In a folder
/// that isn't only VALORANT's (Game Bar, the whole Videos folder), a clip
/// counts when "valorant" is in its name or its folder's.
pub fn find(dir: &Path) -> Vec<Clip> {
    let only_valorant = dir
        .to_string_lossy()
        .to_lowercase()
        .contains("valorant");
    let mut out = Vec::new();
    walk(dir, 0, only_valorant, &mut out);
    out.sort_by(|a, b| a.saved_unix.total_cmp(&b.saved_unix));
    out
}

fn walk(dir: &Path, depth: usize, only_valorant: bool, out: &mut Vec<Clip>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            if depth < 3 {
                let v = only_valorant || path.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains("valorant"));
                walk(&path, depth + 1, v, out);
            }
            continue;
        }
        let video = path
            .extension()
            .is_some_and(|x| VIDEO_EXTS.iter().any(|v| x.eq_ignore_ascii_case(v)));
        let named = only_valorant || path.to_string_lossy().to_lowercase().contains("valorant");
        if !video || !named {
            continue;
        }
        let saved = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64());
        if let Some(saved_unix) = saved {
            out.push(Clip { path, saved_unix });
        }
    }
}

/// A death's clip: the file, and how many seconds before its end the death was.
#[derive(Debug, Clone)]
pub struct Matched {
    pub path: PathBuf,
    pub before_end_s: f64,
}

/// Unix seconds of each death: `played_at` (RFC 3339) plus the match time.
pub fn death_times(played_at: &str, times_ms: &[u32]) -> Option<Vec<f64>> {
    let start = parse_rfc3339(played_at)?;
    Some(times_ms.iter().map(|t| start as f64 + *t as f64 / 1000.0).collect())
}

/// Each death's clip: the first clip saved within `MATCH_WINDOW_S` after it,
/// no clip used twice.
pub fn match_deaths(deaths_unix: &[f64], clips: &[Clip]) -> Vec<Option<Matched>> {
    let mut used = vec![false; clips.len()];
    deaths_unix
        .iter()
        .map(|&t| {
            let (i, c) = clips
                .iter()
                .enumerate()
                .filter(|(i, c)| !used[*i] && c.saved_unix >= t && c.saved_unix - t <= MATCH_WINDOW_S)
                .min_by(|a, b| a.1.saved_unix.total_cmp(&b.1.saved_unix))?;
            used[i] = true;
            Some(Matched {
                path: c.path.clone(),
                before_end_s: c.saved_unix - t,
            })
        })
        .collect()
}

/// `2026-10-19T21:04:11Z` to unix seconds.
pub fn parse_rfc3339(t: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| t.get(r).and_then(|s| s.parse::<i64>().ok());
    let days = crate::time::days_from_civil(n(0..4)?, n(5..7)?, n(8..10)?);
    Some(days * 86_400 + n(11..13)? * 3600 + n(14..16)? * 60 + n(17..19).unwrap_or(0))
}

/// How screenshots of a death are taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShotStyle {
    /// 3 s before, 1 s before and the death, side by side in one image.
    Strip,
    /// Just the moment of the death.
    One,
    /// The three moments as separate images.
    Three,
}

impl ShotStyle {
    pub fn parse(s: Option<&str>) -> Self {
        match s {
            Some("one") => ShotStyle::One,
            Some("three") => ShotStyle::Three,
            _ => ShotStyle::Strip,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            ShotStyle::Strip => "strip",
            ShotStyle::One => "one",
            ShotStyle::Three => "three",
        }
    }

    pub const ALL: [ShotStyle; 3] = [ShotStyle::Strip, ShotStyle::One, ShotStyle::Three];

    pub fn name(self) -> &'static str {
        match self {
            ShotStyle::Strip => "3 moments in one image",
            ShotStyle::One => "1 image, the moment you died",
            ShotStyle::Three => "3 separate images",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            ShotStyle::Strip => "3 s before, 1 s before, the death",
            ShotStyle::One => "fewest files",
            ShotStyle::Three => "for AIs that read small images badly",
        }
    }
}

/// Seconds before the death of each moment taken.
const MOMENTS_S: [f64; 3] = [3.0, 1.0, 0.0];

/// `ffmpeg.exe` on PATH or in `%LOCALAPPDATA%\valreview\ffmpeg`.
pub fn ffmpeg() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Ok(root) = crate::paths::install_root() {
        dirs.push(root.join("ffmpeg"));
        dirs.push(root.join("ffmpeg").join("bin"));
    }
    dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
}

/// A death's screenshot files: `R07.jpg`, or `R07a.jpg` … `R07c.jpg`.
pub fn shot_names(round: u32, style: ShotStyle) -> Vec<String> {
    match style {
        ShotStyle::Three => ["a", "b", "c"].iter().map(|s| format!("R{round:02}{s}.jpg")).collect(),
        _ => vec![format!("R{round:02}.jpg")],
    }
}

/// Takes the screenshots of one death into `dir`, unless they are there already.
pub fn screenshot(ffmpeg: &Path, clip: &Matched, round: u32, style: ShotStyle, dir: &Path) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let names = shot_names(round, style);
    let paths: Vec<PathBuf> = names.iter().map(|n| dir.join(n)).collect();
    if paths.iter().all(|p| p.is_file()) {
        return Ok(paths);
    }
    // Seconds from the clip's end, never past its start.
    let from_end = |before: f64| format!("-{:.2}", (clip.before_end_s + before).max(0.05));
    let input = clip.path.to_string_lossy().into_owned();
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
    match style {
        ShotStyle::One => {
            cmd.args(["-sseof", &from_end(0.0), "-i", &input, "-frames:v", "1", "-q:v", "3"]);
            cmd.arg(&paths[0]);
        }
        ShotStyle::Strip => {
            for m in MOMENTS_S {
                cmd.args(["-sseof", &from_end(m), "-i", &input]);
            }
            cmd.args([
                "-filter_complex",
                "[0:v]scale=960:-2[a];[1:v]scale=960:-2[b];[2:v]scale=960:-2[c];[a][b][c]hstack=inputs=3",
                "-frames:v",
                "1",
                "-q:v",
                "3",
            ]);
            cmd.arg(&paths[0]);
        }
        ShotStyle::Three => {
            for (m, p) in MOMENTS_S.iter().zip(&paths) {
                let mut one = Command::new(ffmpeg);
                one.args(["-hide_banner", "-loglevel", "error", "-y", "-sseof", &from_end(*m), "-i", &input]);
                one.args(["-frames:v", "1", "-q:v", "3"]).arg(p);
                run(&mut one)?;
            }
            return Ok(paths);
        }
    }
    run(&mut cmd)?;
    Ok(paths)
}

fn run(cmd: &mut Command) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashing over the shell.
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().context("running ffmpeg")?;
    if !out.status.success() {
        bail!("ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deaths_get_the_clip_saved_just_after() {
        let start = parse_rfc3339("2026-10-19T21:04:00Z").unwrap();
        let deaths = death_times("2026-10-19T21:04:00Z", &[60_000, 200_000, 400_000]).unwrap();
        assert_eq!(deaths[0], start as f64 + 60.0);
        let clip = |s: f64| Clip {
            path: format!("{s}.mp4").into(),
            saved_unix: start as f64 + s,
        };
        let clips = [clip(70.0), clip(75.0), clip(500.0)];
        let m = match_deaths(&deaths, &clips);
        assert_eq!(m[0].as_ref().unwrap().before_end_s, 10.0);
        // 75 is too long after the second death's 200; 500 is too late for 400.
        assert!(m[1].is_none() && m[2].is_none());
        assert_eq!(shot_names(7, ShotStyle::Three), ["R07a.jpg", "R07b.jpg", "R07c.jpg"]);
        assert_eq!(ShotStyle::parse(None), ShotStyle::Strip);
    }

    #[test]
    fn finds_only_valorant_videos_in_a_shared_folder() {
        let dir = std::env::temp_dir().join(format!("valreview-clips-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("VALORANT")).unwrap();
        std::fs::write(dir.join("VALORANT").join("a.mp4"), b"").unwrap();
        std::fs::write(dir.join("Valorant 2026.10.19 - 21.05.01.mp4"), b"").unwrap();
        std::fs::write(dir.join("other game.mp4"), b"").unwrap();
        std::fs::write(dir.join("notes.txt"), b"").unwrap();
        assert_eq!(find(&dir).len(), 2);
        assert_eq!(find(&dir.join("VALORANT")).len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
