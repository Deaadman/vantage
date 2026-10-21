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

/// Where screenshots go: `Pictures\Vantage\Clips`.
pub fn shots_root() -> Option<PathBuf> {
    directories::UserDirs::new()
        .and_then(|d| d.picture_dir().map(Path::to_path_buf))
        .map(|d| d.join("Vantage").join("Clips"))
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
    let only_valorant = dir.to_string_lossy().to_lowercase().contains("valorant");
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
                let v = only_valorant
                    || path
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().to_lowercase().contains("valorant"));
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
    /// How long the clip is, from an MP4 or MOV header.
    pub length_s: Option<f64>,
    pub bytes: Option<u64>,
}

/// Unix seconds of each death: `played_at` (RFC 3339) plus the match time.
pub fn death_times(played_at: &str, times_ms: &[u32]) -> Option<Vec<f64>> {
    let start = parse_rfc3339(played_at)?;
    Some(times_ms.iter().map(|t| start as f64 + *t as f64 / 1000.0).collect())
}

/// Each death's clip: the first clip saved within `MATCH_WINDOW_S` after it
/// that is long enough to show it, no clip used twice.
pub fn match_deaths(deaths_unix: &[f64], clips: &[Clip]) -> Vec<Option<Matched>> {
    let lengths: Vec<Option<f64>> = clips.iter().map(|c| mp4_length(&c.path)).collect();
    match_deaths_with(deaths_unix, clips, &lengths)
}

fn match_deaths_with(deaths_unix: &[f64], clips: &[Clip], lengths: &[Option<f64>]) -> Vec<Option<Matched>> {
    let mut used = vec![false; clips.len()];
    deaths_unix
        .iter()
        .map(|&t| {
            // A clip whose length is known must start before the death.
            let covers = |i: usize, c: &Clip| lengths[i].is_none_or(|len| c.saved_unix - t <= len);
            let (i, c) = clips
                .iter()
                .enumerate()
                .filter(|(i, c)| !used[*i] && c.saved_unix >= t && c.saved_unix - t <= MATCH_WINDOW_S && covers(*i, c))
                .min_by(|a, b| a.1.saved_unix.total_cmp(&b.1.saved_unix))?;
            used[i] = true;
            Some(Matched {
                path: c.path.clone(),
                before_end_s: c.saved_unix - t,
                length_s: lengths[i],
                bytes: std::fs::metadata(&c.path).ok().map(|m| m.len()),
            })
        })
        .collect()
}

/// A clip's length from its `moov/mvhd` box (MP4 and MOV; other formats give `None`).
pub fn mp4_length(path: &Path) -> Option<f64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let end = f.metadata().ok()?.len();
    let (moov, moov_len) = find_box(&mut f, 0, end, b"moov")?;
    let (mvhd, _) = find_box(&mut f, moov, moov + moov_len, b"mvhd")?;
    f.seek(SeekFrom::Start(mvhd)).ok()?;
    let mut b = [0u8; 32];
    f.read_exact(&mut b).ok()?;
    // Version 1 has 64-bit times: version and flags, created, modified, then the scale and duration.
    let (scale, duration) = if b[0] == 1 {
        (
            u32::from_be_bytes(b[20..24].try_into().ok()?),
            u64::from_be_bytes(b[24..32].try_into().ok()?),
        )
    } else {
        (
            u32::from_be_bytes(b[12..16].try_into().ok()?),
            u64::from(u32::from_be_bytes(b[16..20].try_into().ok()?)),
        )
    };
    (scale > 0).then(|| duration as f64 / f64::from(scale))
}

/// The first `kind` box between `start` and `stop`: its body's offset and length.
fn find_box(f: &mut std::fs::File, start: u64, stop: u64, kind: &[u8; 4]) -> Option<(u64, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut at = start;
    while at + 8 <= stop {
        f.seek(SeekFrom::Start(at)).ok()?;
        let mut head = [0u8; 16];
        f.read_exact(&mut head[..8]).ok()?;
        let mut size = u64::from(u32::from_be_bytes(head[..4].try_into().ok()?));
        let mut body = at + 8;
        if size == 1 {
            f.read_exact(&mut head[8..16]).ok()?;
            size = u64::from_be_bytes(head[8..16].try_into().ok()?);
            body += 8;
        } else if size == 0 {
            size = stop - at;
        }
        let next = at.checked_add(size)?;
        if size < body - at || next > stop {
            return None;
        }
        if &head[4..8] == kind {
            return Some((body, next - body));
        }
        at = next;
    }
    None
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

/// `ffmpeg.exe` on PATH, where winget links it, or in `%LOCALAPPDATA%\vantage\ffmpeg`.
/// winget's links folder is checked by name: a PATH change only reaches
/// terminals opened after the install.
pub fn ffmpeg() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Microsoft").join("WinGet").join("Links"));
    }
    if let Ok(root) = crate::paths::install_root() {
        dirs.push(root.join("ffmpeg"));
        dirs.push(root.join("ffmpeg").join("bin"));
    }
    dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
}

/// A death's screenshot files: `R07.jpg` for the strip, `R07a.jpg` …
/// `R07c.jpg` for three, and just `R07c.jpg`, the moment of the death, for one.
/// Each style has its own names, so changing style never shows another's image.
pub fn shot_names(round: u32, style: ShotStyle) -> Vec<String> {
    match style {
        ShotStyle::Three => ["a", "b", "c"].iter().map(|s| format!("R{round:02}{s}.jpg")).collect(),
        ShotStyle::One => vec![format!("R{round:02}c.jpg")],
        ShotStyle::Strip => vec![format!("R{round:02}.jpg")],
    }
}

/// The longest one ffmpeg run may take before it's stopped.
const FFMPEG_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Takes the screenshots of one death into `dir`, unless they are there already.
/// `dir` must belong to this one match (see `shots_folder`).
pub fn screenshot(ffmpeg: &Path, clip: &Matched, round: u32, style: ShotStyle, dir: &Path) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let names = shot_names(round, style);
    let paths: Vec<PathBuf> = names.iter().map(|n| dir.join(n)).collect();
    if paths.iter().all(|p| p.is_file()) {
        return Ok(paths);
    }
    // Seconds from the clip's end, never past its start.
    let longest = clip.length_s.map_or(f64::INFINITY, |l| (l - 0.05).max(0.05));
    let from_end = |before: f64| format!("-{:.2}", (clip.before_end_s + before).clamp(0.05, longest));
    let input = clip.path.to_string_lossy().into_owned();
    // ffmpeg writes to a temporary name that is renamed once it succeeds, so a
    // crash never leaves a half-written image that would be kept as done.
    let part = |p: &Path| p.with_extension("part.jpg");
    let finish = |p: &Path| std::fs::rename(part(p), p).with_context(|| format!("saving {}", p.display()));
    let one_frame = |before: f64, out: &Path| -> Result<()> {
        let mut one = Command::new(ffmpeg);
        one.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-sseof",
            &from_end(before),
            "-i",
            &input,
        ]);
        one.args(["-frames:v", "1", "-q:v", "3"]).arg(part(out));
        run(&mut one)?;
        finish(out)
    };
    match style {
        ShotStyle::One => one_frame(0.0, &paths[0])?,
        ShotStyle::Three => {
            for (m, p) in MOMENTS_S.iter().zip(&paths) {
                if !p.is_file() {
                    one_frame(*m, p)?;
                }
            }
        }
        ShotStyle::Strip => {
            let mut cmd = Command::new(ffmpeg);
            cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
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
            cmd.arg(part(&paths[0]));
            run(&mut cmd)?;
            finish(&paths[0])?;
        }
    }
    Ok(paths)
}

/// The folder for one match's screenshots: its local start time and map, plus
/// the start of its id, so two matches on the same map and day never share one.
pub fn shots_folder(played_local: Option<&str>, map: &str, match_id: &str) -> String {
    let when = played_local.map_or_else(|| "undated".to_owned(), |t| t.replace(':', "."));
    let id: String = match_id.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
    let name = format!("{when} {map} {id}");
    // Nothing from the replay may reach the path as a separator or a reserved character.
    name.chars()
        .map(|c| {
            if c.is_control() || r#"\/:*?"<>|"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

fn run(cmd: &mut Command) -> Result<()> {
    let out = crate::files::output_within(cmd, FFMPEG_LIMIT).context("running ffmpeg")?;
    if !out.status.success() {
        bail!("ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An MP4 with `ftyp`, then `moov` holding an `mvhd` of the given version.
    fn mp4(version: u8, scale: u32, duration: u64) -> Vec<u8> {
        let boxed = |kind: &[u8], body: &[u8]| {
            let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            b.extend(kind);
            b.extend(body);
            b
        };
        let mut mvhd = vec![version, 0, 0, 0];
        if version == 1 {
            mvhd.extend([0; 16]);
            mvhd.extend(scale.to_be_bytes());
            mvhd.extend(duration.to_be_bytes());
        } else {
            mvhd.extend([0; 8]);
            mvhd.extend(scale.to_be_bytes());
            mvhd.extend((duration as u32).to_be_bytes());
        }
        mvhd.extend([0; 80]);
        let mut file = boxed(b"ftyp", b"isom\0\0\0\0");
        file.extend(boxed(b"moov", &boxed(b"mvhd", &mvhd)));
        file
    }

    #[test]
    fn clip_lengths_from_the_mp4_header() {
        let dir = std::env::temp_dir().join(format!("vantage-mp4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let length = |name: &str, bytes: &[u8]| {
            let p = dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            mp4_length(&p)
        };
        assert_eq!(length("v0.mp4", &mp4(0, 1000, 30_500)), Some(30.5));
        assert_eq!(length("v1.mp4", &mp4(1, 90_000, 90_000 * 65)), Some(65.0));
        assert_eq!(length("noscale.mp4", &mp4(0, 0, 100)), None);
        assert_eq!(length("empty.mp4", b""), None);
        assert_eq!(length("text.mp4", b"not a video at all"), None);
        // A box claiming a 64-bit size near u64::MAX: no overflow, no answer.
        let mut huge = 1u32.to_be_bytes().to_vec();
        huge.extend(b"free");
        huge.extend((u64::MAX - 4).to_be_bytes());
        huge.extend(mp4(0, 1000, 1000));
        assert_eq!(length("huge.mp4", &huge), None);
        // Cut off halfway through the moov box.
        let cut = mp4(0, 1000, 1000);
        assert_eq!(length("cut.mp4", &cut[..cut.len() - 40]), None);
        assert_eq!(mp4_length(&dir.join("missing.mp4")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        assert_ne!(shot_names(7, ShotStyle::One), shot_names(7, ShotStyle::Strip));
        assert_eq!(ShotStyle::parse(None), ShotStyle::Strip);

        // A 30 s clip saved 40 s after the death starts after it: skipped for the 60 s one.
        let m = match_deaths_with(&deaths[..1], &clips[..2], &[Some(5.0), Some(60.0)]);
        assert_eq!(m[0].as_ref().unwrap().before_end_s, 15.0);
    }

    #[test]
    fn each_match_has_its_own_screenshot_folder() {
        let a = shots_folder(Some("2026-10-19 21:04"), "Ascent", "5c673443-5bdc-4576");
        let b = shots_folder(Some("2026-10-19 22:10"), "Ascent", "9f8b32c5-c243-41ec");
        assert_eq!(a, "2026-10-19 21.04 Ascent 5c673443");
        assert_ne!(a, b);
        assert_eq!(shots_folder(None, "..\\x/y", "../.."), "undated .._x_y");
    }

    #[test]
    fn finds_only_valorant_videos_in_a_shared_folder() {
        let dir = std::env::temp_dir().join(format!("vantage-clips-{}", std::process::id()));
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
