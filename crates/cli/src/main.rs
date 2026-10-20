#[macro_use]
mod out;
mod editor;
mod help;
mod review_view;
mod shell;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use out::Kind;
use valreview_core::builds::{BuildSupport, check_build, check_game_version};
use valreview_core::catalog::{self, CatalogSource};
use valreview_core::config::Config;
use valreview_core::install::{self, Layout};
use valreview_core::minimap::Minimap;
use valreview_core::paths;
use valreview_core::scan::{Outcome, ScanOptions, scan};
use valreview_core::select::select_player;
use valreview_core::store::Store;
use valreview_core::update;
use valreview_core::vrfkit::{PINNED, Vrfkit};
use valreview_core::{Catalog, Export, Game, MatchReview};
use valreview_core::{review, summary};

/// This build's version; updates compare against it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(
    name = "valreview",
    version,
    about = "Facts about your own Valorant replays, for review",
    after_help = "Run without a command for the interactive shell: it scans your replays and takes /commands."
)]
struct Cli {
    /// Where catalog.json and later the database live
    /// (default: $VALREVIEW_DATA_DIR, else %LOCALAPPDATA%\valreview\data).
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Export and store every replay not processed yet.
    Scan {
        /// Replay folder for this run (default: from config).
        #[arg(long)]
        demos_dir: Option<PathBuf>,
        /// Keep each raw vrfkit export (about 45 MB per match) under the data dir.
        #[arg(long)]
        keep_raw: bool,
        /// Try files that failed in an earlier scan again.
        #[arg(long)]
        retry_failed: bool,
    },
    /// Write export.md, deaths.jsonl and rounds.jsonl for one player of one match.
    Export {
        /// Directory written by `vrfkit export` (default: the newest scanned match).
        #[arg(conflicts_with = "match_id")]
        export_dir: Option<PathBuf>,
        /// A scanned match id (a unique prefix is enough; default: the newest).
        #[arg(long = "match")]
        match_id: Option<String>,
        /// Account-UUID prefix, pawn id, or agent name (default: `me` from config).
        #[arg(long)]
        me: Option<String>,
        /// Output directory (default: current directory).
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
    /// Show how a match went for you: stats against the lobby, what to work on, rounds, deaths, map.
    Review {
        /// A scanned match id (a unique prefix is enough; default: the newest).
        #[arg(long = "match")]
        match_id: Option<String>,
        /// Account-UUID prefix, pawn id, or agent name (default: `me` from config).
        #[arg(long)]
        me: Option<String>,
    },
    /// Summarise all your matches on one map: what keeps repeating, and what goes well.
    Summary {
        /// Map name, e.g. Pearl (default: the map of your newest match).
        #[arg(long)]
        map: Option<String>,
        /// Account UUID or prefix (default: `me` from config).
        #[arg(long)]
        me: Option<String>,
    },
    /// List scanned matches, newest first.
    Matches,
    /// Aggregate your facts across scanned matches and compare the last N with the N before.
    Trends {
        /// Window size: compare the last N matches with the N before them.
        #[arg(long, default_value_t = 20)]
        last: usize,
        /// Account UUID or prefix (default: `me` from config).
        #[arg(long)]
        me: Option<String>,
        /// Output directory for trends.md (default: current directory).
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
    // Hidden: the shell syncs on launch once the cache is a day old.
    /// Download agents, maps, weapons and seasons from valorant-api.com into the local cache.
    #[command(hide = true)]
    Sync,
    /// Check vrfkit, the catalog, the database and the replay folder, and say what to update.
    Status,
    /// Write config.toml: vrfkit path, replay folder, and which account is you.
    Init {
        /// Your account UUID, or a prefix of one seen in scanned matches.
        /// Without it, init detects you once 2+ matches are scanned.
        #[arg(long)]
        me: Option<String>,
        /// Path to vrfkit.exe (default: the release `valreview update` installs).
        #[arg(long)]
        vrfkit: Option<PathBuf>,
        /// Replay folder (default: %LOCALAPPDATA%\VALORANT\Saved\Demos).
        #[arg(long)]
        demos_dir: Option<PathBuf>,
    },
    // Hidden: the shell updates in the background on launch.
    /// Download and install the newest valreview and the vrfkit it needs (restart to use it).
    #[command(hide = true)]
    Update,
    /// Install this exe and its vrfkit into %LOCALAPPDATA%\valreview (install.ps1 runs this).
    #[command(hide = true)]
    Install {
        /// Run by an older valreview applying an update: print nothing on success.
        #[arg(long)]
        update: bool,
    },
    /// List the players in a vrfkit export with their pawn, agent, team and K/D.
    Players {
        /// Directory written by `vrfkit export`.
        export_dir: PathBuf,
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
                Command::Sync | Command::Status | Command::Update | Command::Install { .. }
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
        Command::Status => status(&data_dir)?,
        Command::Update => update_cmd(&data_dir)?,
        Command::Install { update } => install_cmd(&data_dir, update)?,
        Command::Players { export_dir } => {
            let game = load(&export_dir)?;
            let catalog = load_catalog(&data_dir);
            outln!(
                "{:<6} {:<5} {:<10} {:<38} {:>3}/{:<3}",
                "pawn",
                "team",
                "agent",
                "account",
                "K",
                "D"
            );
            for p in game.players() {
                let (k, d) = game.kd(p.pawn);
                let team = game
                    .teams
                    .side(p.pawn)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "?".into());
                let agent = catalog.agent_name(p.agent_id.as_deref());
                outln!(
                    "{:<6} {:<5} {:<10} {:<38} {:>3}/{:<3}",
                    p.pawn,
                    team,
                    agent,
                    p.subject,
                    k,
                    d
                );
            }
        }
        Command::Export {
            export_dir,
            match_id,
            me,
            out,
        } => {
            let cfg = Config::load(&data_dir)?;
            let review = match (export_dir, match_id) {
                (Some(dir), _) => {
                    let game = load(&dir)?;
                    let catalog = load_catalog(&data_dir);
                    let sel = me.or(cfg.me).context("pass --me (or set it with `valreview init`)")?;
                    let me = select_player(game.players(), &catalog, &sel)?;
                    let review = MatchReview::from_game(&game, &catalog, me);
                    if review.context.map_name.is_none() && !catalog.is_builtin() {
                        errln!(
                            "warning: map {} is not in the catalog; try `valreview sync`",
                            review.map_name()
                        );
                    }
                    review
                }
                (None, id) => {
                    let id = match id {
                        Some(id) => id,
                        None => newest_match(&data_dir)?,
                    };
                    stored_review(&data_dir, &id, me.or(cfg.me).as_deref())?
                }
            };
            std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
            let out = std::path::absolute(&out).unwrap_or(out);
            let md = out.join("export.md");
            let jsonl = out.join("deaths.jsonl");
            let rounds = out.join("rounds.jsonl");
            std::fs::write(&md, review.export_md()).with_context(|| format!("writing {}", md.display()))?;
            std::fs::write(&jsonl, review.deaths_jsonl()).with_context(|| format!("writing {}", jsonl.display()))?;
            std::fs::write(&rounds, review.rounds_jsonl()).with_context(|| format!("writing {}", rounds.display()))?;
            outln!(
                "wrote {}, {} and {} ({} deaths, {} rounds)",
                md.display(),
                jsonl.display(),
                rounds.display(),
                review.death_facts.len(),
                review.round_facts.len()
            );
        }
        Command::Review { match_id, me } => print_review(&load_review(&data_dir, match_id, me)?),
        Command::Summary { map, me } => print_summary(&load_summary(&data_dir, map, me)?),
        Command::Matches => {
            let store = Store::open(&paths::db_path(&data_dir))?;
            let matches = store.matches()?;
            if matches.is_empty() {
                outln!("no scanned matches; run `valreview scan`");
            }
            for (id, played, map) in matches {
                outln!(
                    "{id}  {}  {}",
                    played.as_deref().unwrap_or("?"),
                    map.as_deref().unwrap_or("?")
                );
            }
        }
        Command::Trends { last, me, out } => {
            let cfg = Config::load(&data_dir)?;
            let store = Store::open(&paths::db_path(&data_dir))?;
            let sel = me.or(cfg.me).context("pass --me or set it with `valreview init`")?;
            let subject = resolve_subject(&store, &sel)?;
            let reviews = store.reviews_of(&subject)?;
            if reviews.is_empty() {
                anyhow::bail!("no stored matches for {subject}; run `valreview scan`");
            }
            let report = valreview_core::trends::trends(&subject, &reviews, last);
            std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
            let out = std::path::absolute(&out).unwrap_or(out);
            let path = out.join("trends.md");
            std::fs::write(&path, report.markdown()).with_context(|| format!("writing {}", path.display()))?;
            let compared = report.findings.iter().filter(|f| f.comparable).count();
            outln!(
                "wrote {} ({} matches; {compared} of {} metrics have enough data to compare the last {last} with the {last} before)",
                path.display(),
                report.matches,
                report.findings.len()
            );
        }
        Command::Init { me, vrfkit, demos_dir } => init(&data_dir, me, vrfkit, demos_dir)?,
        Command::Scan {
            demos_dir,
            keep_raw,
            retry_failed,
        } => scan_cmd(&data_dir, demos_dir, keep_raw, retry_failed, true)?,
        Command::Sync => {
            let path = paths::catalog_path(&data_dir);
            let cat =
                catalog::fetch().context("valorant-api.com sync failed; the existing cache (if any) is unchanged")?;
            cat.save(&path)?;
            let callouts: usize = cat.maps.iter().map(|m| m.callouts.len()).sum();
            outln!(
                "synced game version {}: {} agents, {} maps ({} with a minimap projection, {callouts} callouts), {} weapons, {} seasons",
                cat.game_version.as_deref().unwrap_or("?"),
                cat.agents.len(),
                cat.maps.len(),
                cat.maps.iter().filter(|m| m.projection.is_some()).count(),
                cat.weapons.len(),
                cat.seasons.len()
            );
            outln!("saved {}", path.display());
        }
    }
    Ok(())
}

fn init(data_dir: &Path, me: Option<String>, vrfkit: Option<PathBuf>, demos_dir: Option<PathBuf>) -> Result<()> {
    let mut cfg = Config::load(data_dir)?;
    if let Some(p) = vrfkit {
        cfg.vrfkit = Some(std::path::absolute(&p).unwrap_or(p));
    }
    match Vrfkit::locate(cfg.vrfkit.as_deref(), cfg.vrfkit_sha256.as_deref()) {
        Ok(v) => {
            outln!("vrfkit {} at {} (SHA-256 verified)", v.version, v.path.display());
            cfg.vrfkit = Some(v.path);
        }
        Err(e) => errln!("warning: {e:#}"),
    }

    if let Some(d) = demos_dir {
        cfg.demos_dir = Some(d);
    }
    match cfg.demos_dir() {
        Some(d) if d.is_dir() => {
            let n = valreview_core::scan::find_replays(&d).map(|v| v.len()).unwrap_or(0);
            outln!("replay folder {} ({n} replays)", d.display());
        }
        Some(d) => errln!(
            "warning: replay folder {} does not exist yet (download a replay in the client)",
            d.display()
        ),
        None => errln!("warning: no replay folder; pass --demos-dir"),
    }

    let store = Store::open(&paths::db_path(data_dir))?;
    if let Some(sel) = me {
        let sel = sel.trim().to_ascii_lowercase();
        let known: Vec<String> = store
            .known_subjects()?
            .into_iter()
            .filter(|s| s.starts_with(&sel))
            .collect();
        cfg.me = Some(match known.as_slice() {
            [one] => one.clone(),
            [] if is_uuid(&sel) => sel,
            [] => anyhow::bail!("--me `{sel}` is not a full account UUID and matches no scanned player"),
            _ => anyhow::bail!("--me `{sel}` matches several scanned players: {}", known.join(", ")),
        });
        outln!("me = {}", cfg.me.as_deref().unwrap_or_default());
    } else if let Some(me) = &cfg.me {
        outln!("me = {me} (already set; pass --me to change)");
    } else {
        match store.subjects_in_every_match()? {
            (n, _) if n < 2 => {
                outln!("me: not set. Scan at least 2 matches, then run init again to detect it, or pass --me")
            }
            (n, s) if s.len() == 1 => {
                outln!("me = {} (the only account in all {n} scanned matches)", s[0]);
                cfg.me = Some(s[0].clone());
            }
            (n, s) if s.is_empty() => outln!("me: no account appears in all {n} matches; pass --me"),
            (n, s) => outln!(
                "me: {} accounts appear in all {n} matches ({}); pass --me",
                s.len(),
                s.join(", ")
            ),
        }
    }

    cfg.save(data_dir)?;
    outln!("saved {}", Config::path(data_dir).display());
    Ok(())
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

/// `tip`: suggest `init` when `me` could be detected (the shell sets it itself).
fn scan_cmd(data_dir: &Path, demos_dir: Option<PathBuf>, keep_raw: bool, retry_failed: bool, tip: bool) -> Result<()> {
    let cfg = Config::load(data_dir)?;
    let vrfkit = Vrfkit::locate(cfg.vrfkit.as_deref(), cfg.vrfkit_sha256.as_deref())?;
    let demos_dir = demos_dir
        .or_else(|| cfg.demos_dir())
        .context("no replay folder; run `valreview init --demos-dir <dir>`")?;
    let catalog = load_catalog(data_dir);
    let mut store = Store::open(&paths::db_path(data_dir))?;
    let opts = ScanOptions {
        demos_dir,
        data_dir: data_dir.to_path_buf(),
        keep_raw,
        retry_failed,
    };
    let results = scan(&mut store, &vrfkit, &catalog, &opts, |_| {})?;

    // One summary line, then only what needs attention.
    let count = |pred: fn(&Outcome) -> bool| results.iter().filter(|r| pred(&r.outcome)).count();
    let stored = count(|o| matches!(o, Outcome::Stored { .. }));
    let seen = count(|o| matches!(o, Outcome::AlreadyStored | Outcome::Duplicate { .. }));
    let old_failed = count(|o| matches!(o, Outcome::PreviouslyFailed { .. }));
    let unsupported = count(|o| matches!(o, Outcome::Unsupported { .. } | Outcome::PreviouslyUnsupported { .. }));
    let n = results.len();
    out::row(vec![
        (
            Kind::Bold,
            format!("Scanned {n} replay{}", if n == 1 { "" } else { "s" }),
        ),
        (Kind::Dim, format!("  {stored} new · {seen} already done")),
    ]);
    for f in &results {
        match &f.outcome {
            Outcome::Stored {
                match_id,
                map,
                warnings,
            } => {
                out::row(vec![
                    (Kind::Good, "  + ".into()),
                    (Kind::Out, map.clone()),
                    (
                        Kind::Dim,
                        format!("  {} · {}", &match_id[..match_id.len().min(8)], f.file),
                    ),
                ]);
                for w in warnings {
                    out::emit(Kind::Dim, format!("      warning: {w}"));
                }
            }
            Outcome::Failed { error } => {
                out::row(vec![
                    (Kind::Error, "  ✕ ".into()),
                    (Kind::Out, f.file.clone()),
                    (Kind::Dim, "  failed".into()),
                ]);
                out::emit(Kind::Dim, format!("      {error}"));
            }
            _ => {}
        }
    }
    if unsupported > 0 {
        out::row(vec![
            (Kind::Warn, "  ! ".into()),
            (
                Kind::Out,
                format!("{unsupported} on a game build vrfkit can't read yet"),
            ),
            (Kind::Dim, " · tried again after an update".into()),
        ]);
    }
    if old_failed > 0 {
        let retry = if out::in_shell() {
            "/scan --retry-failed"
        } else {
            "valreview scan --retry-failed"
        };
        out::row(vec![
            (Kind::Warn, "  ! ".into()),
            (
                Kind::Out,
                format!(
                    "{old_failed} skipped, {} failed before",
                    if old_failed == 1 { "it" } else { "they" }
                ),
            ),
            (Kind::Dim, format!(" · {retry} tries again")),
        ]);
    }
    if tip
        && cfg.me.is_none()
        && let (n, s) = store.subjects_in_every_match()?
        && n >= 2
        && s.len() == 1
    {
        outln!(
            "tip: {} is in all {n} matches; `valreview init` will set it as you",
            s[0]
        );
    }
    Ok(())
}

fn newest_match(data_dir: &Path) -> Result<String> {
    let store = Store::open(&paths::db_path(data_dir))?;
    let (id, played, map) = store
        .matches()?
        .into_iter()
        .next()
        .context("no scanned matches; run `valreview scan`, or pass an export directory")?;
    outln!(
        "newest match: {id} ({}, {})",
        played.as_deref().unwrap_or("?"),
        map.as_deref().unwrap_or("?")
    );
    Ok(id)
}

/// Full account UUID for a selector: a UUID prefix seen in the store.
fn resolve_subject(store: &Store, sel: &str) -> Result<String> {
    let sel = sel.trim().to_ascii_lowercase();
    let hits: Vec<String> = store
        .known_subjects()?
        .into_iter()
        .filter(|s| s.starts_with(&sel))
        .collect();
    match hits.as_slice() {
        [one] => Ok(one.clone()),
        [] => anyhow::bail!("no scanned match has a player whose account starts with `{sel}`"),
        many => anyhow::bail!("`{sel}` matches several accounts: {}", many.join(", ")),
    }
}

/// A stored review: match id prefix, and a player by UUID prefix, pawn id or agent name.
fn stored_review(data_dir: &Path, match_prefix: &str, me: Option<&str>) -> Result<MatchReview> {
    let store = Store::open(&paths::db_path(data_dir))?;
    let ids: Vec<String> = store
        .matches()?
        .into_iter()
        .map(|m| m.0)
        .filter(|id| id.starts_with(match_prefix.trim()))
        .collect();
    let id = match ids.as_slice() {
        [one] => one.clone(),
        [] => anyhow::bail!("no scanned match id starts with `{match_prefix}`; see `valreview matches`"),
        many => anyhow::bail!("`{match_prefix}` matches {} matches; use a longer prefix", many.len()),
    };
    let sel = me
        .context("pass --me (or set it with `valreview init`)")?
        .trim()
        .to_ascii_lowercase();
    let players = store.players_of(&id)?;
    let hits: Vec<&(String, u32, String)> = players
        .iter()
        .filter(|(subject, pawn, agent)| {
            (sel.len() >= 4 && subject.starts_with(&sel)) || sel == pawn.to_string() || agent.eq_ignore_ascii_case(&sel)
        })
        .collect();
    let subject = match hits.as_slice() {
        [one] => one.0.clone(),
        [] => anyhow::bail!("`{sel}` matches no player of match {id}"),
        _ => anyhow::bail!("`{sel}` matches several players of match {id}; use an account-UUID prefix or pawn id"),
    };
    store.review(&id, &subject)?.with_context(|| {
        format!(
            "match {id} was stored by an older valreview without reviews; its raw export is gone, so it can't be exported"
        )
    })
}

/// Everything `/review` shows for one match.
pub struct ReviewData {
    pub review: MatchReview,
    pub overview: review::Overview,
    pub map: Option<catalog::MapInfo>,
    pub minimap: std::result::Result<Minimap, String>,
}

/// Loads my review of a stored match, the whole lobby's, and the map.
pub(crate) fn load_review(data_dir: &Path, match_id: Option<String>, me: Option<String>) -> Result<ReviewData> {
    let cfg = Config::load(data_dir)?;
    let id = match match_id {
        Some(id) => id,
        None => newest_match(data_dir)?,
    };
    let review = stored_review(data_dir, &id, me.or(cfg.me).as_deref())?;
    let store = Store::open(&paths::db_path(data_dir))?;
    let match_id = &review.context.match_id;
    let mut lobby = Vec::new();
    for (subject, _, _) in store.players_of(match_id)? {
        if let Some(r) = store.review(match_id, &subject)? {
            lobby.push(r);
        }
    }
    let overview = review::overview(&review, &lobby);
    let catalog = load_catalog(data_dir);
    let map = review
        .context
        .map_url
        .as_deref()
        .and_then(|u| catalog.map_by_url(u))
        .cloned();
    let minimap = match &map {
        Some(m) => Minimap::load(data_dir, m).map_err(|e| format!("{e:#}")),
        None => Err("the catalog does not know this map; run `valreview sync`".into()),
    };
    Ok(ReviewData {
        review,
        overview,
        map,
        minimap,
    })
}

/// Everything `/summary` shows for one map.
pub struct SummaryData {
    pub summary: summary::Summary,
    /// My reviews on the map, oldest first.
    pub reviews: Vec<MatchReview>,
    pub map: Option<catalog::MapInfo>,
    pub minimap: std::result::Result<Minimap, String>,
}

/// Loads my reviews on one map (default: my newest match's), each with its lobby.
pub(crate) fn load_summary(data_dir: &Path, map: Option<String>, me: Option<String>) -> Result<SummaryData> {
    let cfg = Config::load(data_dir)?;
    let store = Store::open(&paths::db_path(data_dir))?;
    let sel = me.or(cfg.me).context("pass --me or set it with `valreview init`")?;
    let subject = resolve_subject(&store, &sel)?;
    let all = store.reviews_of(&subject)?;
    let name = |r: &MatchReview| r.map_name().trim_matches('`').to_owned();
    let map_name = match map {
        Some(m) => m,
        None => all
            .last()
            .map(name)
            .context("no stored matches; run `valreview scan`")?,
    };
    let mine: Vec<MatchReview> = all
        .into_iter()
        .filter(|r| name(r).eq_ignore_ascii_case(map_name.trim()))
        .collect();
    let Some(first) = mine.first() else {
        anyhow::bail!("you have no stored matches on {map_name}; see `valreview matches`");
    };
    let map_name = name(first);
    let mut played = Vec::new();
    for r in &mine {
        let id = &r.context.match_id;
        let mut lobby = Vec::new();
        for (s, _, _) in store.players_of(id)? {
            if let Some(l) = store.review(id, &s)? {
                lobby.push(l);
            }
        }
        played.push(summary::Played { me: r.clone(), lobby });
    }
    let catalog = load_catalog(data_dir);
    let map = first
        .context
        .map_url
        .as_deref()
        .and_then(|u| catalog.map_by_url(u))
        .cloned();
    let minimap = match &map {
        Some(m) => Minimap::load(data_dir, m).map_err(|e| format!("{e:#}")),
        None => Err("the catalog does not know this map; run `valreview sync`".into()),
    };
    Ok(SummaryData {
        summary: summary::summary(&map_name, &played),
        reviews: mine,
        map,
        minimap,
    })
}

/// `valreview summary` outside the shell.
fn print_summary(d: &SummaryData) {
    let s = &d.summary;
    outln!(
        "{} · {} match{} · {}",
        s.map,
        s.matches.len(),
        if s.matches.len() == 1 { "" } else { "es" },
        s.agents
    );
    if let Some(note) = &s.note {
        outln!("{note}");
    }
    outln!();
    for st in &s.stats {
        outln!("  {:<24} {}", st.label, st.value);
    }
    outln!();
    outln!("Matches");
    for m in &s.matches {
        outln!(
            "  {}  {:<10} {}–{}  {}/{}  {:.0} dmg/round{}",
            m.label,
            m.agent,
            m.won,
            m.lost,
            m.kills,
            m.deaths,
            m.adr,
            m.adr_rank
                .map(|r| format!(" ({} of {})", review::ordinal(r), m.of))
                .unwrap_or_default()
        );
    }
    outln!();
    outln!("Work on");
    for (i, f) in s.work_on.iter().enumerate() {
        outln!("  {}. {}", i + 1, f.title);
        for e in &f.evidence {
            outln!("     {e}");
        }
    }
    if s.work_on.is_empty() {
        outln!("  nothing repeats yet");
    }
    outln!();
    outln!("Went well");
    for f in &s.went_well {
        outln!("  - {}", f.title);
    }
    if s.went_well.is_empty() {
        outln!("  nothing stood out");
    }
}

/// `valreview review` outside the shell: the Overview and Work on tabs as text.
fn print_review(d: &ReviewData) {
    let o = &d.overview;
    outln!(
        "{} on {} · {} {}–{} · {}/{}",
        o.agent,
        o.map,
        if o.won > o.lost {
            "won"
        } else if o.won < o.lost {
            "lost"
        } else {
            "drew"
        },
        o.won,
        o.lost,
        o.kills,
        o.deaths
    );
    outln!();
    for s in &o.stats {
        let rank = s
            .rank
            .map(|r| format!("{} of {}", review::ordinal(r), s.of))
            .unwrap_or_default();
        outln!("  {:<24} {:<16} {rank}", s.label, s.value);
    }
    outln!();
    outln!("Work on");
    for (i, f) in o.work_on.iter().enumerate() {
        outln!("  {}. {}", i + 1, f.title);
        for e in &f.evidence {
            outln!("     {e}");
        }
    }
    if o.work_on.is_empty() {
        outln!("  nothing stood out");
    }
    outln!();
    outln!("Went well");
    for f in &o.went_well {
        outln!("  - {}", f.title);
    }
    if o.went_well.is_empty() {
        outln!("  nothing stood out");
    }
    for w in &d.review.warnings {
        errln!("note: {w}");
    }
    if let Some(note) = &d.review.context.build_note {
        errln!("note: {note}");
    }
}

fn load(dir: &Path) -> Result<Game> {
    let export = Export::load(dir)?;
    match check_build(&export.manifest.replay_build) {
        BuildSupport::Supported => {}
        BuildSupport::Caveat(msg) | BuildSupport::Unsupported(msg) => errln!("warning: {msg}"),
    }
    let game = Game::new(export);
    for w in &game.warnings {
        errln!("warning: {w}");
    }
    Ok(game)
}

/// Never fails: falls back to built-in agent names with a one-line note.
/// One line when the live game has moved past what vrfkit decodes, and one
/// when the catalog is old. Never fails.
fn startup_check(data_dir: &Path) {
    let Ok(cat) = Catalog::load(&paths::catalog_path(data_dir)) else {
        return;
    };
    if let Some(msg) = cat.game_version.as_deref().and_then(check_game_version) {
        errln!("note: {msg}");
    }
    if let Some(days) = cat.age_days(valreview_core::time::now_unix()).filter(|&d| d > 30) {
        errln!("note: the valorant-api.com catalog is {days} days old; run `valreview sync` for new agents and maps");
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
        outln!("valreview is up to date ({VERSION})");
        return Ok(());
    }
    outln!("updating valreview {VERSION} to {}…", manifest.version);
    match update::apply(&layout, &manifest, data_dir) {
        Ok(_) => {
            update::State::record(data_dir, format!("installed {}", manifest.version));
            outln!(
                "Updated valreview from {VERSION} to {}. Restart valreview to use it.",
                manifest.version
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
    outln!("installed valreview {VERSION} to {}", layout.launcher().display());
    let on_path = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d == layout.bin()));
    if !on_path {
        outln!(
            "add {} to PATH to run `valreview` from any terminal",
            layout.bin().display()
        );
    }
    Ok(())
}

fn status(data_dir: &Path) -> Result<()> {
    outln!("data dir: {}", data_dir.display());
    let cfg = Config::load(data_dir)?;
    let exe = std::env::current_exe().ok();
    match Layout::new() {
        Ok(l) if exe.as_deref().is_some_and(|e| l.contains(e)) => {
            outln!("valreview: {VERSION}, installed in {}", l.root.display())
        }
        _ => outln!(
            "valreview: {VERSION}, not installed (running from {})",
            exe.as_deref().map_or("?".into(), |e| e.display().to_string())
        ),
    }
    let last = update::State::load(data_dir).map_or_else(
        || "no check yet".to_owned(),
        |s| format!("last check {}: {}", s.checked_at, s.result),
    );
    match update::auto_disabled(cfg.auto_update) {
        Some(why) => outln!("updates: {why} ({last})"),
        None => outln!("updates: automatic in the shell ({last})"),
    }
    outln!(
        "me: {}",
        cfg.me
            .as_deref()
            .unwrap_or("not set (run `valreview init` after scanning 2+ matches)")
    );
    match Vrfkit::locate(cfg.vrfkit.as_deref(), cfg.vrfkit_sha256.as_deref()) {
        Ok(v) => outln!(
            "vrfkit: {} at {} (SHA-256 verified); decodes builds {}",
            v.version,
            v.path.display(),
            valreview_core::builds::SUPPORTED_RANGE
        ),
        Err(e) => outln!("vrfkit: PROBLEM: {e:#}"),
    }
    match Catalog::load(&paths::catalog_path(data_dir)) {
        Ok(c) => {
            let age = c
                .age_days(valreview_core::time::now_unix())
                .map(|d| format!("{d} days old"))
                .unwrap_or_default();
            outln!(
                "catalog: game version {}, {} agents, {} maps, {} weapons ({age})",
                c.game_version.as_deref().unwrap_or("?"),
                c.agents.len(),
                c.maps.len(),
                c.weapons.len()
            );
            if let Some(msg) = c.game_version.as_deref().and_then(check_game_version) {
                outln!("  UPDATE: {msg}");
            }
        }
        Err(e) => outln!("catalog: none or unreadable ({e:#}); run `valreview sync`"),
    }
    let db = paths::db_path(data_dir);
    if db.is_file() {
        let store = Store::open(&db)?;
        let c = store.counts()?;
        outln!(
            "database: schema {}, {} matches, {} deaths, {} replays logged as failed, unsupported or duplicate",
            valreview_core::store::STORE_SCHEMA_VERSION,
            c.matches,
            c.deaths,
            c.logged
        );
    } else {
        outln!("database: none yet (run `valreview scan`)");
    }
    match cfg.demos_dir() {
        Some(d) if d.is_dir() => {
            let n = valreview_core::scan::find_replays(&d).map(|v| v.len()).unwrap_or(0);
            outln!("replays: {n} in {}", d.display());
        }
        Some(d) => outln!(
            "replays: folder {} does not exist (download a replay in the client)",
            d.display()
        ),
        None => outln!("replays: no folder set (`valreview init --demos-dir`)"),
    }
    Ok(())
}

fn load_catalog(data_dir: &Path) -> Catalog {
    let (cat, source) = Catalog::load_or_builtin(&paths::catalog_path(data_dir));
    match source {
        CatalogSource::Cache => {}
        CatalogSource::BuiltinNoCache => {
            errln!(
                "note: no catalog cache; using built-in agent names only (run `valreview sync` for maps and callouts)"
            )
        }
        CatalogSource::BuiltinBadCache(e) => {
            errln!("warning: ignoring unreadable catalog ({e}); using built-in agent names. Run `valreview sync`")
        }
    }
    cat
}
