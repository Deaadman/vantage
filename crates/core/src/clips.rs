//! Clips and screenshots for `/review`: find the folder a clip app saves to,
//! match each death to the clip saved just after it, and take screenshots
//! of the seconds before the death with ffmpeg.
//!
//! Medal's clips are matched from Medal's own database, which tags each clip
//! with the VALORANT match and when its recording started: Medal saves a
//! match's death clips all at once when the match ends, so their file times
//! say nothing about the deaths. Other clips are matched by when they were
//! saved: instant replay saves the last seconds, so a clip saved soon after a
//! death ends just after it. The death's wall-clock time is the replay's
//! timestamp plus the death's match time plus `REPLAY_CLOCK_LAG_S`.

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

/// The known apps' default folders and how many VALORANT clips each holds.
/// Only Medal documents its default: `C:\Medal\Clips\Valorant`, with clips
/// named like `MedalTVValorant20250809233847.mp4` (support.medal.tv). It
/// can be moved in Medal; other apps' folders are typed as "Another folder…".
pub fn apps() -> Vec<App> {
    let drive = PathBuf::from(format!(
        "{}\\",
        std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into())
    ));
    [("Medal", drive.join("Medal").join("Clips").join("Valorant"))]
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
/// that isn't only VALORANT's (a clip app's shared folder, the whole Videos folder), a clip
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

/// How much later a death happens on the wall clock than `played_at` plus its
/// match time. It holds within a match but not between them: against Medal's
/// death bookmarks it was 5.8 s in one match and 7.1 s in the next (`played_at`
/// is to the second, and the game starts recording after a varying wait). The
/// middle of those keeps clips without a bookmark within about 0.7 s.
pub const REPLAY_CLOCK_LAG_S: f64 = 6.5;

/// Unix seconds of each death: `played_at` (RFC 3339) plus the match time.
pub fn death_times(played_at: &str, times_ms: &[u32]) -> Option<Vec<f64>> {
    let start = parse_rfc3339(played_at)? as f64 + REPLAY_CLOCK_LAG_S;
    Some(times_ms.iter().map(|t| start + *t as f64 / 1000.0).collect())
}

/// A clip from Medal's database: the file, when its recording started, and
/// where in it Medal bookmarked the death, from the game's own event.
#[derive(Debug, Clone)]
pub struct MedalClip {
    pub path: PathBuf,
    pub start_unix: f64,
    pub death_at_s: Option<f64>,
}

/// This match's clips in Medal's database, `Databases\medal-*.db` in Medal's
/// folder: the clip folder or one above it. Empty when there is no database
/// or it can't be read, and then clips are matched by when they were saved.
pub fn medal_clips(clips_dir: &Path, match_id: &str) -> Vec<MedalClip> {
    let mut out: Vec<MedalClip> = medal_dbs(clips_dir)
        .iter()
        .flat_map(|db| read_medal_db(db, match_id).unwrap_or_default())
        .collect();
    out.retain(|c| c.path.is_file());
    out.sort_by(|a, b| a.start_unix.total_cmp(&b.start_unix));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

/// Medal's databases for the clip folder (see `medal_clips`).
fn medal_dbs(clips_dir: &Path) -> Vec<PathBuf> {
    clips_dir
        .ancestors()
        .take(4)
        .filter_map(|root| std::fs::read_dir(root.join("Databases")).ok())
        .flat_map(|entries| entries.flatten().map(|e| e.path()))
        .filter(|p| {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            name.starts_with("medal-") && name.ends_with(".db")
        })
        .collect()
}

/// Medal saved clips after `since_unix` but hasn't recorded them in its
/// database yet, which it does minutes later: until then they can't be told
/// apart, as Medal saves a match's clips all at once when it ends.
pub fn medal_pending(clips_dir: &Path, since_unix: f64) -> bool {
    !medal_dbs(clips_dir).is_empty() && find(clips_dir).iter().any(|c| c.saved_unix >= since_unix)
}

/// Changes when the clips in `clips_dir` or Medal's records of them do, so a
/// review can look again for clips it didn't have.
pub fn stamp(clips_dir: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let file = |p: &Path| std::fs::metadata(p).ok().map(|m| (m.len(), m.modified().ok()));
    for db in medal_dbs(clips_dir) {
        // Medal writes to the `-wal` file first and the database later.
        file(&db).hash(&mut h);
        file(&PathBuf::from(format!("{}-wal", db.display()))).hash(&mut h);
    }
    for c in find(clips_dir) {
        c.path.hash(&mut h);
        c.saved_unix.to_bits().hash(&mut h);
    }
    h.finish()
}

fn read_medal_db(db: &Path, match_id: &str) -> rusqlite::Result<Vec<MedalClip>> {
    use rusqlite::{Connection, OpenFlags};
    // Medal keeps it open while it runs; reading alongside it is fine.
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    let mut stmt = conn.prepare(
        "SELECT video_path, metadata FROM contents WHERE video_path IS NOT NULL AND instr(metadata, ?1) > 0",
    )?;
    let rows = stmt.query_map([match_id.as_bytes()], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    Ok(rows
        .flatten()
        .filter_map(|(path, meta)| {
            Some(MedalClip {
                path: path.into(),
                start_unix: clip_start(&meta)?,
                death_at_s: death_bookmark(&meta),
            })
        })
        .collect())
}

/// `clipStartTime` from a clip's metadata in Medal's database: a binary
/// encoding with the time as text, `2026-10-22T09:18:08.398+00:00`.
fn clip_start(meta: &[u8]) -> Option<f64> {
    let key = b"clipStartTime";
    let at = meta.windows(key.len()).position(|w| w == key)? + key.len();
    // A byte or two of length and type before the text.
    let rest = meta.get(at..(at + 48).min(meta.len()))?;
    let from = rest.windows(2).take(4).position(|w| w == b"20")?;
    let text: String = rest[from..]
        .iter()
        .take_while(|b| b.is_ascii_digit() || b"-T:.+Z".contains(b))
        .map(|&b| b as char)
        .collect();
    let utc = text.ends_with("+00:00") || text.ends_with('Z');
    if !utc {
        return None;
    }
    let frac = text
        .get(19..)
        .and_then(|f| f.strip_prefix('.'))
        .map(|f| f.chars().take_while(char::is_ascii_digit).collect::<String>())
        .and_then(|d| format!("0.{d}").parse::<f64>().ok())
        .unwrap_or(0.0);
    Some(parse_rfc3339(&text)? as f64 + frac)
}

/// Seconds into the clip of Medal's `Death` bookmark, from the same encoding:
/// `bookmarks … time` `9.733` `name` `Death`, each text after a byte of
/// length and type. The replay's clock only places a death to within a second
/// or two (the lag varies by match); the bookmark is within a frame or so.
fn death_bookmark(meta: &[u8]) -> Option<f64> {
    let from = meta.windows(9).position(|w| w == b"bookmarks")?;
    let rest = &meta[from..];
    rest.windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b"time")
        .find_map(|(i, _)| {
            let after = rest.get(i + 5..)?;
            let digits: String = after
                .iter()
                .take_while(|b| b.is_ascii_digit() || **b == b'.')
                .map(|&b| b as char)
                .collect();
            // The bookmark's name follows its time: `name`, a byte, `Death`.
            let name = after.get(digits.len()..(digits.len() + 12).min(after.len()))?;
            let death = name.windows(5).any(|w| w == b"Death");
            if death { digits.parse().ok() } else { None }
        })
}

/// Each death's clip from Medal: the clip whose recording holds the death,
/// the one with the death nearest its middle when two do, no clip used twice.
pub fn match_medal(deaths_unix: &[f64], clips: &[MedalClip]) -> Vec<Option<Matched>> {
    let lengths: Vec<Option<f64>> = clips.iter().map(|c| mp4_length(&c.path)).collect();
    match_medal_with(deaths_unix, clips, &lengths)
}

fn match_medal_with(deaths_unix: &[f64], clips: &[MedalClip], lengths: &[Option<f64>]) -> Vec<Option<Matched>> {
    let mut used = vec![false; clips.len()];
    deaths_unix
        .iter()
        .map(|&t| {
            let (i, c, len) = clips
                .iter()
                .enumerate()
                .filter_map(|(i, c)| Some((i, c, lengths[i]?)))
                .filter(|(i, c, len)| !used[*i] && c.start_unix <= t && t <= c.start_unix + len)
                .min_by(|a, b| {
                    let off = |(_, c, len): &(usize, &MedalClip, f64)| (t - c.start_unix - len / 2.0).abs();
                    off(a).total_cmp(&off(b))
                })?;
            used[i] = true;
            Some(Matched {
                path: c.path.clone(),
                before_end_s: c
                    .death_at_s
                    .filter(|d| (0.0..=len).contains(d))
                    .map_or(c.start_unix + len - t, |d| len - d),
                length_s: Some(len),
                bytes: std::fs::metadata(&c.path).ok().map(|m| m.len()),
            })
        })
        .collect()
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
    /// The four moments in a 2×2 grid, and the close-up.
    Grid,
    /// Just the moment of the death.
    One,
    /// The four moments as separate images, and the close-up.
    Four,
}

impl ShotStyle {
    /// `strip` and `three` were the grid and the separate images when there were three moments.
    pub fn parse(s: Option<&str>) -> Self {
        match s {
            Some("one") => ShotStyle::One,
            Some("four" | "three") => ShotStyle::Four,
            _ => ShotStyle::Grid,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            ShotStyle::Grid => "grid",
            ShotStyle::One => "one",
            ShotStyle::Four => "four",
        }
    }

    pub const ALL: [ShotStyle; 3] = [ShotStyle::Grid, ShotStyle::One, ShotStyle::Four];

    pub fn name(self) -> &'static str {
        match self {
            ShotStyle::Grid => "4 moments in one image, and a close-up",
            ShotStyle::One => "1 image, the moment you died",
            ShotStyle::Four => "4 separate images, and a close-up",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            ShotStyle::Grid => "3 s and 1 s before, as you died, just after",
            ShotStyle::One => "fewest files",
            ShotStyle::Four => "for AIs that read small images badly",
        }
    }
}

/// Seconds before the death of each moment taken; negative is after it.
/// A quarter second before is what I saw as I was shot; half a second after
/// shows the "Killed by" panel and often the killer outlined in red.
const MOMENTS_S: [f64; 4] = [3.0, 1.0, 0.25, -0.5];
/// The moment of the death, for the close-up and the one-image style.
const DEATH_S: f64 = MOMENTS_S[2];

/// Width of each moment in the grid: two across fit the 1568 px Claude reads
/// without shrinking the image (the other AIs' limits are close).
const GRID_TILE_W: u32 = 720;

/// The close-up: the middle half of the screen as I died,
/// at the clip's own resolution, where the crosshair, the peek and a killer
/// in front are; enlarged so the AI looks at it closely.
const CLOSE_UP: &str = "crop=iw/2:ih/2,scale=1280:-2:flags=lanczos";

/// Changes whenever the screenshots are taken differently, so the ones taken
/// before are taken again (see `forget_old_shots`).
const SHOTS_LAYOUT: &str = "3";
const LAYOUT_FILE: &str = "layout.txt";

/// `ffmpeg.exe` on PATH, where winget links or unpacks it, or in
/// `%LOCALAPPDATA%\vantage\ffmpeg`. winget's folders are checked by name: a
/// PATH change only reaches programs started after the install, and winget
/// adds the package folder to PATH instead of linking when it can't link.
pub fn ffmpeg() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    // Per user, then machine wide (an install from an admin terminal).
    let winget_roots: Vec<PathBuf> = [("LOCALAPPDATA", "Microsoft"), ("ProgramFiles", "")]
        .iter()
        .filter_map(|(var, sub)| std::env::var_os(var).map(|v| PathBuf::from(v).join(sub).join("WinGet")))
        .collect();
    for root in &winget_roots {
        dirs.push(root.join("Links"));
    }
    if let Ok(root) = crate::paths::install_root() {
        dirs.push(root.join("ffmpeg"));
        dirs.push(root.join("ffmpeg").join("bin"));
    }
    // winget unpacks to `Packages\Gyan.FFmpeg_<source>\ffmpeg-<version>-full_build\bin`.
    for root in &winget_roots {
        dirs.extend(winget_package_bins(&root.join("Packages")));
    }
    dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
}

/// The `bin` folders of every ffmpeg winget unpacked under `packages`.
fn winget_package_bins(packages: &Path) -> Vec<PathBuf> {
    let children = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|r| {
                r.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default()
    };
    children(packages)
        .into_iter()
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().to_lowercase().starts_with("gyan.ffmpeg"))
        })
        .flat_map(|pkg| children(&pkg))
        .map(|build| build.join("bin"))
        .collect()
}

/// A death's screenshot files: `R07.jpg` for the grid, `R07a.jpg` …
/// `R07d.jpg` for the four moments, `R07z.jpg` for the close-up, and just
/// `R07c.jpg`, the moment of the death, for one. A name is always the same
/// picture, so changing style never shows a wrong image.
pub fn shot_names(round: u32, style: ShotStyle) -> Vec<String> {
    let name = |s: &str| format!("R{round:02}{s}.jpg");
    match style {
        ShotStyle::Four => ["a", "b", "c", "d", "z"].map(name).to_vec(),
        ShotStyle::One => vec![name("c")],
        ShotStyle::Grid => vec![name(""), name("z")],
    }
}

/// Deletes a match's screenshots taken by an older layout, whose names may
/// now mean other pictures, so they are taken again. `dir` is one match's folder.
pub fn forget_old_shots(dir: &Path) -> Result<()> {
    let marker = dir.join(LAYOUT_FILE);
    if std::fs::read_to_string(&marker).is_ok_and(|l| l.trim() == SHOTS_LAYOUT) {
        return Ok(());
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for p in entries.flatten().map(|e| e.path()) {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.starts_with('R') && name.ends_with(".jpg") {
                std::fs::remove_file(&p).with_context(|| format!("deleting {}", p.display()))?;
            }
        }
    }
    crate::files::write_atomic(&marker, SHOTS_LAYOUT.as_bytes())
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
    let one_frame = |before: f64, filter: Option<&str>, out: &Path| -> Result<()> {
        if out.is_file() {
            return Ok(());
        }
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
        if let Some(f) = filter {
            one.args(["-vf", f]);
        }
        one.args(["-frames:v", "1", "-q:v", "3"]).arg(part(out));
        run(&mut one)?;
        finish(out)
    };
    let close_up = |out: &Path| one_frame(DEATH_S, Some(CLOSE_UP), out);
    match style {
        ShotStyle::One => one_frame(DEATH_S, None, &paths[0])?,
        ShotStyle::Four => {
            for (m, p) in MOMENTS_S.iter().zip(&paths) {
                one_frame(*m, None, p)?;
            }
            close_up(&paths[4])?;
        }
        ShotStyle::Grid => {
            if !paths[0].is_file() {
                let mut cmd = Command::new(ffmpeg);
                cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
                for m in MOMENTS_S {
                    cmd.args(["-sseof", &from_end(m), "-i", &input]);
                }
                let w = GRID_TILE_W;
                let grid = format!(
                    "[0:v]scale={w}:-2[a];[1:v]scale={w}:-2[b];[2:v]scale={w}:-2[c];[3:v]scale={w}:-2[d];\
                     [a][b]hstack[top];[c][d]hstack[bottom];[top][bottom]vstack"
                );
                cmd.args(["-filter_complex", &grid, "-frames:v", "1", "-q:v", "3"]);
                cmd.arg(part(&paths[0]));
                run(&mut cmd)?;
                finish(&paths[0])?;
            }
            close_up(&paths[1])?;
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
        let start = parse_rfc3339("2026-10-19T21:04:00Z").unwrap() as f64 + REPLAY_CLOCK_LAG_S;
        let deaths = death_times("2026-10-19T21:04:00Z", &[60_000, 200_000, 400_000]).unwrap();
        assert_eq!(deaths[0], start + 60.0);
        let clip = |s: f64| Clip {
            path: format!("{s}.mp4").into(),
            saved_unix: start + s,
        };
        let clips = [clip(70.0), clip(75.0), clip(500.0)];
        let m = match_deaths(&deaths, &clips);
        assert_eq!(m[0].as_ref().unwrap().before_end_s, 10.0);
        // 75 is too long after the second death's 200; 500 is too late for 400.
        assert!(m[1].is_none() && m[2].is_none());
        assert_eq!(
            shot_names(7, ShotStyle::Four),
            ["R07a.jpg", "R07b.jpg", "R07c.jpg", "R07d.jpg", "R07z.jpg"]
        );
        assert_eq!(shot_names(7, ShotStyle::Grid), ["R07.jpg", "R07z.jpg"]);
        assert_eq!(shot_names(7, ShotStyle::One), ["R07c.jpg"]);
        assert_eq!(ShotStyle::parse(None), ShotStyle::Grid);
        assert_eq!(ShotStyle::parse(Some("strip")), ShotStyle::Grid);
        assert_eq!(ShotStyle::parse(Some("three")), ShotStyle::Four);

        // A 30 s clip saved 40 s after the death starts after it: skipped for the 60 s one.
        let m = match_deaths_with(&deaths[..1], &clips[..2], &[Some(5.0), Some(60.0)]);
        assert_eq!(m[0].as_ref().unwrap().before_end_s, 15.0);
    }

    #[test]
    fn medal_clips_saved_after_the_match_still_match_their_deaths() {
        // Medal's metadata around the time: a length byte, then the text.
        let mut meta = b"\x97title\x97Death\xc7clipStartTime\xc7\x1d".to_vec();
        meta.extend(b"2026-10-22T09:18:08.398+00:00\xc7\x11activeWindowTitle");
        let start = clip_start(&meta).unwrap();
        assert!((start - (parse_rfc3339("2026-10-22T09:18:08Z").unwrap() as f64 + 0.398)).abs() < 1e-6);
        assert_eq!(clip_start(b"clipStartTime\x1d2026-10-22T19:48:08.398+10:30"), None);
        assert_eq!(clip_start(b"no time here"), None);

        // Two deaths 140 s apart, each in a 20 s clip starting 10 s before it;
        // both clips saved long after, which matching by save time can't use.
        let deaths = [start + 10.0, start + 150.0];
        let clip = |s: f64| MedalClip {
            path: format!("{s}.mp4").into(),
            start_unix: start + s,
            death_at_s: None,
        };
        let mut clips = [clip(0.0), clip(140.0)];
        let m = match_medal_with(&deaths, &clips, &[Some(20.0), Some(20.0)]);
        assert_eq!(m[0].as_ref().unwrap().path, clips[0].path);
        assert!((m[1].as_ref().unwrap().before_end_s - 10.0).abs() < 1e-6);
        // Medal's bookmark of the death beats the replay's clock.
        clips[1].death_at_s = Some(11.25);
        let m = match_medal_with(&deaths, &clips, &[Some(20.0), Some(20.0)]);
        assert!((m[1].as_ref().unwrap().before_end_s - 8.75).abs() < 1e-6);
        // A death no clip holds, and a clip of unknown length, get nothing.
        let m = match_medal_with(&[start + 60.0, start + 150.0], &clips, &[Some(20.0), None]);
        assert!(m[0].is_none() && m[1].is_none());
    }

    #[test]
    fn clips_medal_hasnt_recorded_yet_are_noticed() {
        let dir = std::env::temp_dir().join(format!("vantage-pending-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let clips = dir.join("Clips").join("Valorant");
        std::fs::create_dir_all(&clips).unwrap();
        std::fs::create_dir_all(dir.join("Databases")).unwrap();
        // No Medal database: nothing to wait for.
        std::fs::write(clips.join("MedalTVValorant1.mp4"), b"").unwrap();
        assert!(!medal_pending(&clips, 0.0));
        std::fs::write(dir.join("Databases").join("medal-1.db"), b"").unwrap();
        assert!(medal_pending(&clips, 0.0));
        assert!(!medal_pending(&clips, 4e9), "clips from before the match don't count");
        // Medal writing its records, or a new clip, changes the stamp.
        let before = stamp(&clips);
        assert_eq!(stamp(&clips), before);
        std::fs::write(dir.join("Databases").join("medal-1.db-wal"), b"rows").unwrap();
        let after = stamp(&clips);
        assert_ne!(after, before);
        std::fs::write(clips.join("MedalTVValorant2.mp4"), b"").unwrap();
        assert_ne!(stamp(&clips), after);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_medal_s_death_bookmark() {
        // As Medal stores it, cut short after the bookmark's timestamp.
        let meta = b"\xd7\x13videoLengthSeconds\x95\x1220.152\xcbbookmarks\xcbO\xccMGtimeU9.733GnameWDeathGdata\xbc\x87Event ID\x176\x97timestamp\xc3\r1792744435484";
        assert_eq!(death_bookmark(meta), Some(9.733));
        // A kill bookmark, or none at all, isn't a death.
        assert_eq!(
            death_bookmark(b"bookmarks\xcbO\xccMGtimeU4.5GnameWKill\x97timestamp"),
            None
        );
        assert_eq!(death_bookmark(b"clipStartTime 2026"), None);
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
    fn screenshots_from_an_older_layout_are_taken_again() {
        let dir = std::env::temp_dir().join(format!("vantage-layout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("R07.jpg"), b"old").unwrap();
        std::fs::write(dir.join("notes.txt"), b"").unwrap();
        forget_old_shots(&dir).unwrap();
        assert!(!dir.join("R07.jpg").exists() && dir.join("notes.txt").exists());
        std::fs::write(dir.join("R07.jpg"), b"new").unwrap();
        forget_old_shots(&dir).unwrap();
        assert!(dir.join("R07.jpg").exists(), "this layout's screenshots are kept");
        std::fs::remove_dir_all(&dir).unwrap();
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

    #[test]
    fn finds_medal_clips_in_its_valorant_folder() {
        let dir = std::env::temp_dir().join(format!("vantage-medal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let valorant = dir.join("Medal").join("Clips").join("Valorant");
        std::fs::create_dir_all(&valorant).unwrap();
        std::fs::create_dir_all(dir.join("Medal").join("Imported")).unwrap();
        for name in ["MedalTVValorant20250809233847.mp4", "MedalTVValorant20250809233848.mp4"] {
            std::fs::write(valorant.join(name), b"").unwrap();
        }
        std::fs::write(
            dir.join("Medal")
                .join("Imported")
                .join("MedalTVValorant20250809224907.mp4"),
            b"",
        )
        .unwrap();
        assert_eq!(find(&valorant).len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn finds_ffmpeg_where_winget_unpacked_it() {
        let dir = std::env::temp_dir().join(format!("vantage-winget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bin = dir
            .join("Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe")
            .join("ffmpeg-9.0.2-full_build")
            .join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(dir.join("Other.Package").join("x").join("bin")).unwrap();
        assert_eq!(winget_package_bins(&dir), vec![bin]);
        assert!(winget_package_bins(&dir.join("missing")).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
