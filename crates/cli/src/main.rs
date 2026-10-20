#[macro_use]
mod out;
mod coach_views;
mod editor;
mod help;
mod panel;
#[cfg(test)]
mod render_tests;
mod review_view;
mod settings;
mod shell;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use out::Kind;
use valreview_core::builds::check_game_version;
use valreview_core::catalog::{self, CatalogSource};
use valreview_core::clips::{self, Matched, ShotStyle};
use valreview_core::coach::{self, Debrief, plural};
use valreview_core::config::Config;
use valreview_core::habits;
use valreview_core::install::{self, Layout};
use valreview_core::minimap::Minimap;
use valreview_core::paths;
use valreview_core::ranks;
use valreview_core::scan::{Outcome, ScanOptions, scan};
use valreview_core::store::{MyMatch, Store};
use valreview_core::update;
use valreview_core::vrfkit::{PINNED, Vrfkit};
use valreview_core::{Catalog, MatchReview, ai};

/// This build's version; updates compare against it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The name on screen.
pub const APP_NAME: &str = "Val Review";
/// `/matches` shows this many unless `--all`.
pub const MATCHES_SHOWN: usize = 10;

#[derive(Parser)]
#[command(
    name = "valreview",
    version,
    about = "A VALORANT coach that reads your own replays",
    after_help = "Run without a command for the interactive shell: it watches your replays and takes /commands."
)]
struct Cli {
    /// Where the database and settings live
    /// (default: $VALREVIEW_DATA_DIR, else %LOCALAPPDATA%\valreview\data).
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// What went wrong in a match, and what to fix next time
    Review {
        /// A match from /matches (default: your newest)
        #[arg(long = "match", value_name = "MATCH")]
        match_id: Option<String>,
    },
    /// A game plan for one map: where you win, where you lose, what to do
    Plan {
        /// Map name, e.g. Lotus (default: the map you play most)
        #[arg(long)]
        map: Option<String>,
    },
    /// The mistakes you keep making, and which are getting better
    Habits,
    /// Pick one habit to work on; every new match checks it
    Focus {
        /// The habit to focus on (leave it out to see your current focus)
        habit: Option<String>,
    },
    /// What wins games at your rank, and how you compare
    Rank,
    /// Your matches, newest first
    Matches {
        /// Show every match, not just the newest 10
        #[arg(long)]
        all: bool,
    },
    /// Look for new replays now
    Scan {
        /// Replay folder for this run (default: from settings)
        #[arg(long)]
        demos_dir: Option<PathBuf>,
        /// Keep each raw vrfkit export (about 45 MB per match) under the data dir
        #[arg(long)]
        keep_raw: bool,
        /// Try files that failed in an earlier scan again
        #[arg(long)]
        retry_failed: bool,
    },
    /// Your rank, clip folder, focus and other settings
    #[command(alias = "status")]
    Settings,
    // Hidden: the shell syncs on launch once the cache is a day old.
    /// Download agents, maps and weapons from valorant-api.com into the local cache.
    #[command(hide = true)]
    Sync,
    // Hidden: the shell updates in the background on launch.
    /// Download and install the newest Val Review and the vrfkit it needs (restart to use it).
    #[command(hide = true)]
    Update,
    /// Install this exe and its vrfkit into %LOCALAPPDATA%\valreview (install.ps1 runs this).
    #[command(hide = true)]
    Install {
        /// Run by an older valreview applying an update: print nothing on success.
        #[arg(long)]
        update: bool,
    },
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        errln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let data_dir = paths::data_dir(cli.data_dir.as_deref())?;
    match cli.command {
        Some(command) => {
            if !matches!(
                command,
                Command::Sync | Command::Settings | Command::Update | Command::Install { .. }
            ) {
                startup_check(&data_dir);
            }
            run_command(&data_dir, command)
        }
        None if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() => shell::run(&data_dir),
        None => {
            Cli::command().print_help()?;
            Ok(())
        }
    }
}

fn run_command(data_dir: &Path, command: Command) -> Result<()> {
    let data_dir = data_dir.to_path_buf();
    match command {
        Command::Settings => settings::print_status(&data_dir),
        Command::Update => update_cmd(&data_dir)?,
        Command::Install { update } => install_cmd(&data_dir, update)?,
        Command::Review { match_id } => print_review(&load_review(&data_dir, match_id)?),
        Command::Plan { map } => print_plan(&load_plan(&data_dir, map)?),
        Command::Habits => {
            let (progress, focus, _) = load_habits(&data_dir)?;
            for p in progress {
                outln!(
                    "{:<34}{:<10}{:<22}{}{}",
                    p.habit.name,
                    habits::spark(&p.values),
                    p.now,
                    p.trend.word(),
                    if p.habit.slug == focus { "  ◆ focus" } else { "" }
                );
            }
        }
        Command::Focus { habit } => focus_cmd(&data_dir, habit)?,
        Command::Rank => {
            let (rank, standing, _) = load_rank(&data_dir)?;
            let adv = ranks::advice(ranks::band(&rank));
            outln!("{rank} · what wins games at {}", adv.label);
            for (i, (t, why)) in adv.wins.iter().enumerate() {
                outln!("  {}. {t}. {why}", i + 1);
            }
            for t in &standing.better {
                outln!("  + {t}");
            }
            for t in &standing.behind {
                outln!("  - {t}");
            }
        }
        Command::Matches { all } => print_matches(&data_dir, all)?,
        Command::Scan {
            demos_dir,
            keep_raw,
            retry_failed,
        } => {
            scan_cmd(&data_dir, demos_dir, keep_raw, retry_failed)?;
        }
        Command::Sync => {
            let path = paths::catalog_path(&data_dir);
            let cat =
                catalog::fetch().context("valorant-api.com sync failed; the existing cache (if any) is unchanged")?;
            cat.save(&path)?;
            outln!(
                "synced game version {}: {} agents, {} maps, {} weapons",
                cat.game_version.as_deref().unwrap_or("?"),
                cat.agents.len(),
                cat.maps.len(),
                cat.weapons.len(),
            );
        }
    }
    Ok(())
}

/// Looks for new replays and reports them. Returns the matches it stored.
fn scan_cmd(data_dir: &Path, demos_dir: Option<PathBuf>, keep_raw: bool, retry_failed: bool) -> Result<Vec<String>> {
    let cfg = Config::load(data_dir)?;
    let vrfkit = Vrfkit::locate(cfg.vrfkit.as_deref(), cfg.vrfkit_sha256.as_deref())?;
    let demos_dir = demos_dir
        .or_else(|| cfg.demos_dir())
        .context("no replay folder; set it in /settings")?;
    let catalog = load_catalog(data_dir);
    let mut store = Store::open(&paths::db_path(data_dir))?;
    let opts = ScanOptions {
        demos_dir,
        data_dir: data_dir.to_path_buf(),
        keep_raw: keep_raw || cfg.keep_raw == Some(true),
        retry_failed,
    };
    let results = scan(&mut store, &vrfkit, &catalog, &opts, |_| {})?;

    // One summary line, then only what needs attention.
    let count = |pred: fn(&Outcome) -> bool| results.iter().filter(|r| pred(&r.outcome)).count();
    let stored = count(|o| matches!(o, Outcome::Stored { .. }));
    let seen = count(|o| matches!(o, Outcome::AlreadyStored | Outcome::Duplicate { .. }));
    let failed = count(|o| matches!(o, Outcome::Failed { .. } | Outcome::PreviouslyFailed { .. }));
    let unsupported = count(|o| matches!(o, Outcome::Unsupported { .. } | Outcome::PreviouslyUnsupported { .. }));
    let n = results.len();
    out::row(vec![
        (Kind::Bold, format!("Checked {}", plural(n, "replay"))),
        (Kind::Dim, format!("  {stored} new · {seen} already done")),
    ]);
    let mut ids = Vec::new();
    for f in &results {
        if let Outcome::Stored { match_id, map, .. } = &f.outcome {
            let agent = cfg
                .me
                .as_deref()
                .and_then(|me| store.review(match_id, me).ok().flatten())
                .map(|r| format!("{} on ", r.agent))
                .unwrap_or_default();
            out::row(vec![(Kind::Good, "  + ".into()), (Kind::Out, format!("{agent}{map}"))]);
            ids.push(match_id.clone());
        }
    }
    if failed > 0 {
        out::row(vec![
            (Kind::Warn, "  ! ".into()),
            (
                Kind::Out,
                format!(
                    "{} couldn't be read",
                    if failed == 1 {
                        "1 replay".into()
                    } else {
                        format!("{failed} replays")
                    }
                ),
            ),
            (Kind::Dim, " · usually a download that was cut short".into()),
        ]);
    }
    if unsupported > 0 {
        out::row(vec![
            (Kind::Warn, "  ! ".into()),
            (
                Kind::Out,
                format!("{unsupported} from a VALORANT version Val Review can't read yet"),
            ),
            (Kind::Dim, " · read after the next update".into()),
        ]);
    }
    Ok(ids)
}

fn me_of(cfg: &Config) -> Result<String> {
    cfg.me
        .clone()
        .context("Val Review doesn't know which account is you yet. It finds you once 2 matches share one account, or pick it in /settings.")
}

/// The id of a stored match from a prefix, or the newest.
fn find_match(store: &Store, prefix: Option<&str>) -> Result<String> {
    let all = store.matches()?;
    let Some(prefix) = prefix.map(str::trim) else {
        return all
            .into_iter()
            .next()
            .map(|m| m.0)
            .context("No matches yet. Download a replay in the VALORANT client and it shows up here.");
    };
    let ids: Vec<String> = all
        .into_iter()
        .map(|m| m.0)
        .filter(|id| id.starts_with(prefix))
        .collect();
    match ids.as_slice() {
        [one] => Ok(one.clone()),
        [] => anyhow::bail!("no match starts with \"{prefix}\". /matches lists them."),
        many => anyhow::bail!("\"{prefix}\" fits {} matches. Type a few more letters.", many.len()),
    }
}

/// One match's reviews for every player in it.
fn lobby_of(store: &Store, match_id: &str) -> Result<Vec<MatchReview>> {
    let mut lobby = Vec::new();
    for (subject, _, _) in store.players_of(match_id)? {
        if let Some(r) = store.review(match_id, &subject)? {
            lobby.push(r);
        }
    }
    Ok(lobby)
}

fn map_and_minimap(
    data_dir: &Path,
    r: &MatchReview,
) -> (Option<catalog::MapInfo>, std::result::Result<Minimap, String>) {
    let catalog = load_catalog(data_dir);
    let map = r
        .context
        .map_url
        .as_deref()
        .and_then(|u| catalog.map_by_url(u))
        .cloned();
    let minimap = match &map {
        Some(m) => Minimap::load(data_dir, m).map_err(|e| format!("{e:#}")),
        None => Err("Val Review doesn't know this map yet".into()),
    };
    (map, minimap)
}

/// A clip folder to pick from.
#[derive(Debug, Clone)]
pub struct ClipFolder {
    pub name: String,
    pub about: String,
    /// VALORANT clips found (`None`: not counted, like the Videos scan).
    pub found: Option<usize>,
    /// `None`: type the path yourself.
    pub path: Option<PathBuf>,
}

/// The clip apps' folders, the Videos scan, and a folder of your own.
pub fn clip_folders() -> Vec<ClipFolder> {
    let mut out: Vec<ClipFolder> = clips::apps()
        .into_iter()
        .map(|a| ClipFolder {
            name: a.name.into(),
            about: if a.found > 0 {
                format!("found {} VALORANT clips in {}", a.found, a.dir.display())
            } else {
                "not found on this PC".into()
            },
            found: Some(a.found),
            path: Some(a.dir),
        })
        .collect();
    if let Some(v) = clips::videos_dir() {
        out.push(ClipFolder {
            name: "Scan my Videos folder".into(),
            about: format!("looks for VALORANT clips anywhere in {}", v.display()),
            found: None,
            path: Some(v),
        });
    }
    out.push(ClipFolder {
        name: "Another folder…".into(),
        about: "type the path to your clips".into(),
        found: None,
        path: None,
    });
    out
}

/// The Clips and Screenshots tabs.
pub enum Clips {
    NotSet(Vec<ClipFolder>),
    Set {
        dir: PathBuf,
        /// Each death's clip, in the order of the debrief's deaths.
        per_death: Vec<Option<Matched>>,
        shots_dir: PathBuf,
        /// (round, files) for each death with screenshots.
        shots: Vec<(u32, Vec<PathBuf>)>,
        /// Why some or all screenshots are missing.
        shot_error: Option<String>,
        style: ShotStyle,
    },
}

/// Everything `/review` shows for one match.
pub struct ReviewData {
    pub debrief: Debrief,
    pub focus: Option<habits::Check>,
    pub notes: Vec<String>,
    pub map: Option<catalog::MapInfo>,
    pub minimap: std::result::Result<Minimap, String>,
    pub clips: Clips,
    pub ai: Option<ai::Saved>,
    pub ai_provider: Option<&'static ai::Provider>,
}

impl ReviewData {
    /// The screenshots the AI review sends.
    pub fn shots(&self) -> Vec<ai::Shot> {
        match &self.clips {
            Clips::Set { shots, .. } => shots
                .iter()
                .flat_map(|(_, files)| files)
                .map(|p| ai::Shot {
                    name: p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    path: p.clone(),
                })
                .collect(),
            Clips::NotSet(_) => vec![],
        }
    }
}

/// Loads my review of a stored match, its debrief, and its clips (taking
/// any screenshots not taken yet).
pub(crate) fn load_review(data_dir: &Path, match_id: Option<String>) -> Result<ReviewData> {
    let cfg = Config::load(data_dir)?;
    let me = me_of(&cfg)?;
    let store = Store::open(&paths::db_path(data_dir))?;
    let id = find_match(&store, match_id.as_deref())?;
    let review = store
        .review(&id, &me)?
        .context("You aren't in this match, or it was stored by an older valreview. /matches lists yours.")?;
    let lobby = lobby_of(&store, &id)?;
    let debrief = coach::debrief(&review, &lobby);
    let mine = store.reviews_of(&me)?;
    let upto = mine
        .iter()
        .position(|r| r.context.match_id == id)
        .map_or(mine.len(), |i| i + 1);
    let focus = habits::check(habits::focus(cfg.focus.as_deref()), &mine[..upto]);
    let (map, minimap) = map_and_minimap(data_dir, &review);
    let clips = match &cfg.clips_dir {
        None => Clips::NotSet(clip_folders()),
        Some(dir) => load_clips(dir, &debrief, ShotStyle::parse(cfg.shots.as_deref())),
    };
    let mut notes = review.warnings.clone();
    notes.extend(review.context.build_note.clone());
    Ok(ReviewData {
        ai: ai::load(data_dir, &id),
        ai_provider: cfg.ai.as_deref().and_then(ai::provider),
        debrief,
        focus,
        notes,
        map,
        minimap,
        clips,
    })
}

fn load_clips(dir: &Path, d: &Debrief, style: ShotStyle) -> Clips {
    let found = clips::find(dir);
    let times: Vec<u32> = d.deaths.iter().map(|x| x.time_ms).collect();
    let per_death = d
        .played_at
        .as_deref()
        .and_then(|p| clips::death_times(p, &times))
        .map(|t| clips::match_deaths(&t, &found))
        .unwrap_or_else(|| vec![None; d.deaths.len()]);
    let folder = format!(
        "{} {}",
        d.played_at.as_deref().and_then(|t| t.get(..10)).unwrap_or("undated"),
        d.map
    );
    let shots_dir = clips::shots_root()
        .unwrap_or_else(|| dir.join("valreview"))
        .join(folder);
    let mut shots = Vec::new();
    let mut shot_error = None;
    if per_death.iter().any(Option::is_some) {
        match clips::ffmpeg() {
            None => {
                shot_error = Some(
                    "Screenshots need ffmpeg, which isn't installed. Install it (winget install ffmpeg) and open this review again."
                        .into(),
                )
            }
            Some(ff) => {
                for (x, c) in d.deaths.iter().zip(&per_death) {
                    let Some(c) = c else { continue };
                    match clips::screenshot(&ff, c, x.round, style, &shots_dir) {
                        Ok(files) => shots.push((x.round, files)),
                        Err(e) => shot_error = Some(format!("Round {}'s screenshot failed: {e:#}", x.round)),
                    }
                }
            }
        }
    }
    Clips::Set {
        dir: dir.to_path_buf(),
        per_death,
        shots_dir,
        shots,
        shot_error,
        style,
    }
}

/// The map's name as stored, or the end of its path.
fn map_label(m: &str) -> &str {
    m.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(m)
}

/// Everything `/plan` shows.
pub(crate) fn load_plan(data_dir: &Path, map: Option<String>) -> Result<coach_views::PlanData> {
    let cfg = Config::load(data_dir)?;
    let me = me_of(&cfg)?;
    let store = Store::open(&paths::db_path(data_dir))?;
    let mine = store.reviews_of(&me)?;
    let name = |r: &MatchReview| coach::map_name(r);
    let map_name = match map {
        Some(m) => {
            let known = catalog_maps(data_dir);
            let want = m.trim().to_lowercase();
            known
                .iter()
                .chain(mine.iter().map(name).collect::<Vec<_>>().iter())
                .find(|k| k.to_lowercase() == want)
                .cloned()
                .with_context(|| {
                    format!(
                        "Val Review doesn't know a map called {m}. Try one of: {}.",
                        known.join(", ")
                    )
                })?
        }
        None => {
            let mut counts: Vec<(String, usize)> = Vec::new();
            for r in &mine {
                match counts.iter_mut().find(|(n, _)| *n == name(r)) {
                    Some((_, k)) => *k += 1,
                    None => counts.push((name(r), 1)),
                }
            }
            counts
                .into_iter()
                .max_by_key(|(_, k)| *k)
                .map(|(n, _)| n)
                .context("No matches yet, so there's nothing to plan from.")?
        }
    };
    let here: Vec<MatchReview> = mine.into_iter().filter(|r| name(r) == map_name).collect();
    let plan = valreview_core::plan::plan(&map_name, &here);
    let fix = here.last().and_then(|r| {
        let d = coach::debrief(r, &[]);
        d.fixes.first().map(|f| (f.title.clone(), f.try_short.clone()))
    });
    let (map, minimap) = match here.last() {
        Some(r) => map_and_minimap(data_dir, r),
        None => (None, Err("you haven't played here yet".into())),
    };
    Ok(coach_views::PlanData {
        plan,
        rank: cfg.rank.clone(),
        fix,
        map,
        minimap,
    })
}

fn catalog_maps(data_dir: &Path) -> Vec<String> {
    let (cat, _) = Catalog::load_or_builtin(&paths::catalog_path(data_dir));
    cat.maps
        .iter()
        .filter(|m| !m.callouts.is_empty())
        .map(|m| m.name.clone())
        .collect()
}

/// Every habit over my recent matches, my focus, and how many matches.
pub(crate) fn load_habits(data_dir: &Path) -> Result<(Vec<habits::Progress>, &'static str, usize)> {
    let cfg = Config::load(data_dir)?;
    let me = me_of(&cfg)?;
    let mine = Store::open(&paths::db_path(data_dir))?.reviews_of(&me)?;
    if mine.is_empty() {
        anyhow::bail!("No matches yet. Download a replay in the VALORANT client and it shows up here.");
    }
    Ok((
        habits::progress(&mine),
        habits::focus(cfg.focus.as_deref()).slug,
        mine.len(),
    ))
}

/// My rank, how I compare with my lobbies, and my focus's name.
pub(crate) fn load_rank(data_dir: &Path) -> Result<(String, valreview_core::standing::Standing, &'static str)> {
    let cfg = Config::load(data_dir)?;
    let rank = cfg
        .rank
        .clone()
        .context("Your rank isn't set yet. Open /settings, go to Config and pick it under Rank.")?;
    let store = Store::open(&paths::db_path(data_dir))?;
    let games = match &cfg.me {
        Some(me) => {
            let mine = store.reviews_of(me)?;
            let recent = &mine[mine.len().saturating_sub(habits::WINDOW)..];
            recent
                .iter()
                .map(|r| {
                    Ok(valreview_core::standing::Game {
                        lobby: lobby_of(&store, &r.context.match_id)?,
                        me: r.clone(),
                    })
                })
                .collect::<Result<Vec<_>>>()?
        }
        None => vec![],
    };
    Ok((
        rank,
        valreview_core::standing::standing(&games),
        habits::focus(cfg.focus.as_deref()).name,
    ))
}

/// `/focus`: show the focus, or pick one.
fn focus_cmd(data_dir: &Path, habit: Option<String>) -> Result<()> {
    let mut cfg = Config::load(data_dir)?;
    let slugs = habits::HABITS.iter().map(|h| h.slug).collect::<Vec<_>>().join(", ");
    let Some(want) = habit else {
        focus_line(data_dir);
        out::emit(Kind::Dim, format!("  To switch, type /focus and pick a habit: {slugs}"));
        return Ok(());
    };
    let h =
        habits::habit(&want).with_context(|| format!("There's no habit called \"{want}\". Pick one of: {slugs}."))?;
    cfg.focus = Some(h.slug.to_owned());
    cfg.save(data_dir)?;
    out::row(vec![
        (Kind::Paint(shell::YELLOW, true), "◆ ".into()),
        (Kind::Out, "Your focus is now ".into()),
        (Kind::Bold, h.name.into()),
        (Kind::Out, ". After each match, Val Review checks it first.".into()),
    ]);
    out::emit(Kind::Dim, format!("  What to do: {}", h.drill));
    Ok(())
}

/// `◆ Your focus: Dying where nobody can trade  ▃▂▂▁  getting better`
pub(crate) fn focus_line(data_dir: &Path) {
    let cfg = Config::load(data_dir).unwrap_or_default();
    let focus = habits::focus(cfg.focus.as_deref());
    let mut row = vec![
        (Kind::Paint(shell::YELLOW, false), "◆ ".into()),
        (Kind::Out, "Your focus: ".into()),
        (Kind::Bold, focus.name.into()),
    ];
    let progress = cfg
        .me
        .as_deref()
        .and_then(|me| Store::open(&paths::db_path(data_dir)).ok()?.reviews_of(me).ok())
        .map(|mine| habits::progress(&mine))
        .and_then(|p| p.into_iter().find(|p| p.habit.slug == focus.slug));
    if let Some(p) = progress.filter(|p| !p.values.is_empty()) {
        let c = coach_views::trend_color(p.trend);
        row.push((Kind::Paint(c, false), format!("  {}", habits::spark(&p.values))));
        row.push((Kind::Paint(c, true), format!("  {}", p.trend.lately())));
    }
    out::row(row);
}

/// After a new match: the focus first, then the top fix and what went well.
pub(crate) fn focus_check(data_dir: &Path, match_id: &str) {
    let Ok(d) = load_review(data_dir, Some(match_id.to_owned())) else {
        return;
    };
    let b = &d.debrief;
    let (res, color) = review_view::result_style(b.won, b.lost);
    outln!();
    out::row(vec![
        (Kind::Paint(review_view::GREEN, true), "✓ ".into()),
        (Kind::Out, "New match: ".into()),
        (Kind::Bold, format!("{} on {}", b.agent, b.map)),
        (Kind::Dim, " · ".into()),
        (Kind::Paint(color, true), format!("{res} {}–{}", b.won, b.lost)),
        (
            Kind::Dim,
            format!(" · {}, {}", plural(b.kills, "kill"), plural(b.deaths_n, "death")),
        ),
    ]);
    if let Some(c) = &d.focus {
        outln!();
        out::row(vec![
            (Kind::Paint(shell::YELLOW, true), "◆ Focus check: ".into()),
            (Kind::Bold, c.habit.name.into()),
        ]);
        out::row(vec![
            (Kind::Dim, format!("  {:<14}", "This match")),
            (Kind::Bold, review_view::focus_result(c)),
        ]);
        if c.streak >= 2 {
            out::row(vec![
                (Kind::Dim, format!("  {:<14}", "Lately")),
                (
                    Kind::Paint(review_view::GREEN, true),
                    format!("at or under your usual {} matches in a row", c.streak),
                ),
            ]);
        }
        if let Some(f) = b.fixes.iter().find(|f| f.habit == Some(c.habit.slug)) {
            out::row(vec![
                (Kind::Dim, format!("  {:<14}", "Try")),
                (Kind::Out, f.try_short.clone()),
            ]);
        }
    }
    if !b.fixes.is_empty() || !b.keep.is_empty() {
        outln!();
        out::row(vec![(Kind::Bold, "Also this match".into())]);
        if let Some(f) = b.fixes.first() {
            out::row(vec![
                (Kind::Paint(shell::YELLOW, true), "  ! ".into()),
                (Kind::Out, f.title.clone()),
            ]);
        }
        if let Some(k) = b.keep.first() {
            out::row(vec![
                (Kind::Paint(review_view::GREEN, true), "  + ".into()),
                (Kind::Out, k.clone()),
            ]);
        }
    }
    out::emit(
        Kind::Dim,
        "  /review for the full debrief, your clips and the AI review".into(),
    );
}

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Days since 1970 and the parts of a `YYYY-MM-DD…` date.
fn day_parts(ymd: &str) -> Option<(i64, i64, i64)> {
    let mut parts = ymd.get(..10)?.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (Some(Some(y)), Some(Some(m @ 1..=12)), Some(Some(d))) = (parts.next(), parts.next(), parts.next()) else {
        return None;
    };
    Some((valreview_core::time::days_from_civil(y, m, d), m, d))
}

/// `Monday 19 October` and how long ago it was (`yesterday`), for a
/// `YYYY-MM-DD` date and today's day number (days since 1970-01-01).
fn day_heading(ymd: &str, today: i64) -> Option<(String, Option<String>)> {
    let (day, m, d) = day_parts(ymd)?;
    let weekday = WEEKDAYS[(day + 4).rem_euclid(7) as usize];
    let ago = match today - day {
        0 => Some("today".to_owned()),
        1 => Some("yesterday".to_owned()),
        n @ 2..=6 => Some(format!("{n} days ago")),
        _ => None,
    };
    Some((format!("{weekday} {d} {}", MONTHS[m as usize - 1]), ago))
}

/// `2026-10-19T21:04:11Z` -> `Mon 19 Oct 21:04`.
pub fn short_when(t: &str) -> String {
    match day_parts(t) {
        Some((day, m, d)) => format!(
            "{} {d} {} {}",
            &WEEKDAYS[(day + 4).rem_euclid(7) as usize][..3],
            &MONTHS[m as usize - 1][..3],
            t.get(11..16).unwrap_or("")
        )
        .trim_end()
        .to_owned(),
        None => t.to_owned(),
    }
}

/// My matches, newest first, with how each went.
pub(crate) fn my_matches(data_dir: &Path) -> Vec<MyMatch> {
    let db = paths::db_path(data_dir);
    let me = Config::load(data_dir).ok().and_then(|c| c.me).unwrap_or_default();
    if !db.is_file() {
        return vec![];
    }
    Store::open(&db).and_then(|s| s.my_matches(&me)).unwrap_or_default()
}

/// `/matches`: the newest matches under a heading for their day.
fn print_matches(data_dir: &Path, all: bool) -> Result<()> {
    let matches = my_matches(data_dir);
    if matches.is_empty() {
        outln!("No matches yet. Download one in the client and it shows up here.");
        return Ok(());
    }
    let shown = if all {
        matches.len()
    } else {
        matches.len().min(MATCHES_SHOWN)
    };
    let today = valreview_core::time::now_unix().div_euclid(86_400);
    let mut day: Option<String> = None;
    for m in &matches[..shown] {
        let played = m.played_at.clone().unwrap_or_default();
        let date = played.get(..10).unwrap_or("").to_owned();
        if day.as_ref() != Some(&date) {
            if day.is_some() {
                outln!();
            }
            let mut heading = vec![(Kind::Bold, "Date unknown".to_owned())];
            if let Some((name, ago)) = day_heading(&date, today) {
                heading = vec![(Kind::Bold, name)];
                heading.extend(ago.map(|a| (Kind::Dim, format!(" · {a}"))));
            }
            out::row(heading);
            day = Some(date);
        }
        let time = played.get(11..16).unwrap_or("--:--");
        let map = m.map.as_deref().map(map_label).unwrap_or("?");
        let mut row = vec![(Kind::Dim, format!("  {time}  ")), (Kind::Out, format!("{map:<10}"))];
        if let Some(agent) = &m.agent {
            let (res, color) = review_view::result_style(m.won, m.lost);
            row.push((Kind::Out, format!("{agent:<10}")));
            row.push((Kind::Paint(color, true), format!("{res} {}–{}", m.won, m.lost)));
        }
        out::row(row);
    }
    outln!();
    let older = matches.len() - shown;
    if older > 0 {
        out::emit(
            Kind::Dim,
            format!(
                "{older} older match{} · /matches --all shows them all",
                if older == 1 { "" } else { "es" }
            ),
        );
    }
    out::emit(Kind::Dim, "/review then pick a match from the list to open it".into());
    Ok(())
}

/// `valreview review` outside the shell: the Debrief as text.
fn print_review(d: &ReviewData) {
    let b = &d.debrief;
    outln!(
        "{} on {} · {} {}–{} · {}/{}",
        b.agent,
        b.map,
        b.result(),
        b.won,
        b.lost,
        b.kills,
        b.deaths_n
    );
    outln!();
    outln!("{}", b.summary);
    outln!();
    outln!("Fix first");
    for (i, f) in b.fixes.iter().take(3).enumerate() {
        outln!("  {}. {} ({})", i + 1, f.title, f.cost);
        outln!("     {}", f.short);
        outln!("     Try: {}", f.try_short);
    }
    if b.fixes.is_empty() {
        outln!("  nothing repeated");
    }
    outln!();
    outln!("Keep doing");
    for k in &b.keep {
        outln!("  + {k}");
    }
    for n in &d.notes {
        errln!("note: {n}");
    }
}

/// `valreview plan` outside the shell.
fn print_plan(d: &coach_views::PlanData) {
    let p = &d.plan;
    outln!("{} · {}", p.map, plural(p.matches, "match"));
    for (name, side) in [("Attack", &p.attack), ("Defense", &p.defense)] {
        outln!();
        outln!("{name}");
        for s in &side.sites {
            outln!("  {:<4} won {:>2} of {:<3} {}", s.site, s.won, s.of, s.note);
        }
        for t in &side.do_this {
            outln!("  ✓ {t}");
        }
        for t in &side.avoid {
            outln!("  ✗ {t}");
        }
        outln!("  {}", side.after);
    }
    outln!();
    outln!("{}", p.one_line);
}

/// One line when the live game has moved past what vrfkit decodes. Never fails.
fn startup_check(data_dir: &Path) {
    let Ok(cat) = Catalog::load(&paths::catalog_path(data_dir)) else {
        return;
    };
    if let Some(msg) = cat.game_version.as_deref().and_then(check_game_version) {
        errln!("note: {msg}");
    }
}

/// `valreview update`: installs the newest release, or repairs vrfkit when up to date.
fn update_cmd(data_dir: &Path) -> Result<()> {
    let layout = Layout::new()?;
    if std::env::current_exe().is_ok_and(|e| layout.contains(&e)) {
        install::cleanup(&layout, VERSION);
    }
    let manifest = update::latest().context("checking for updates failed")?;
    if !install::is_newer(&manifest.version, VERSION) {
        update::State::record(data_dir, format!("up to date ({VERSION})"));
        if install::ensure_vrfkit(&layout)? {
            outln!("installed vrfkit {}", PINNED.tag);
        }
        outln!("{APP_NAME} is up to date ({VERSION})");
        return Ok(());
    }
    match update::apply(&layout, &manifest, data_dir) {
        Ok(_) => {
            update::State::record(data_dir, format!("installed {}", manifest.version));
            out::row(vec![
                (Kind::Paint(review_view::GREEN, true), "✓ ".into()),
                (Kind::Bold, "New version installed".into()),
            ]);
            out::emit(
                Kind::Dim,
                format!(
                    "  Restart {APP_NAME} to use it: /exit, then run valreview again. What's new shows when it opens."
                ),
            );
            Ok(())
        }
        Err(e) => {
            update::State::record(data_dir, format!("failed: {e:#}"));
            Err(e)
        }
    }
}

/// `valreview install`: this exe into `versions`, its vrfkit, and `bin`.
fn install_cmd(data_dir: &Path, quiet: bool) -> Result<()> {
    let layout = Layout::new()?;
    let exe = std::env::current_exe().context("finding this exe")?;
    let installed = install::install(&layout, VERSION, &exe)?;
    // An older install.ps1 or `init` may have pinned a path inside the
    // install; the installed release is found without it, so drop it.
    let mut cfg = Config::load(data_dir)?;
    if cfg.vrfkit.as_deref().is_some_and(|p| p.starts_with(&layout.root)) {
        cfg.vrfkit = None;
        cfg.save(data_dir)?;
    }
    if quiet {
        return Ok(());
    }
    if installed.vrfkit_downloaded {
        outln!("installed vrfkit {} (SHA-256 verified)", PINNED.tag);
    }
    outln!("installed {APP_NAME} {VERSION} to {}", layout.launcher().display());
    let on_path = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d == layout.bin()));
    if !on_path {
        outln!(
            "add {} to PATH to run `valreview` from any terminal",
            layout.bin().display()
        );
    }
    Ok(())
}

fn load_catalog(data_dir: &Path) -> Catalog {
    let (cat, source) = Catalog::load_or_builtin(&paths::catalog_path(data_dir));
    match source {
        CatalogSource::Cache => {}
        CatalogSource::BuiltinNoCache => errln!("note: no VALORANT data yet; using built-in agent names only"),
        CatalogSource::BuiltinBadCache(e) => {
            errln!("warning: ignoring unreadable VALORANT data ({e}); using built-in agent names")
        }
    }
    cat
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_headings() {
        let today = valreview_core::time::days_from_civil(2026, 10, 20);
        let h = |d: &str| day_heading(d, today);
        let day = |name: &str, ago: Option<&str>| Some((name.to_owned(), ago.map(str::to_owned)));
        assert_eq!(h("2026-10-20"), day("Tuesday 20 October", Some("today")));
        assert_eq!(h("2026-10-19"), day("Monday 19 October", Some("yesterday")));
        assert_eq!(h("2026-10-14"), day("Wednesday 14 October", Some("6 days ago")));
        assert_eq!(h("2026-10-13"), day("Tuesday 13 October", None));
        assert_eq!(h(""), None);
        assert_eq!(short_when("2026-10-19T21:04:11Z"), "Mon 19 Oct 21:04");
    }
}
