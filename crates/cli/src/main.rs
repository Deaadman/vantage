use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use valreview_core::builds::{BuildSupport, check_build};
use valreview_core::catalog::{self, CatalogSource};
use valreview_core::config::Config;
use valreview_core::paths;
use valreview_core::scan::{Outcome, ScanOptions, scan};
use valreview_core::select::select_player;
use valreview_core::store::Store;
use valreview_core::vrfkit::Vrfkit;
use valreview_core::{Catalog, Export, Game, Review};

#[derive(Parser)]
#[command(
    name = "valreview",
    version,
    about = "Facts about your own Valorant replays, for review"
)]
struct Cli {
    /// Where catalog.json and later the database live
    /// (default: $VALREVIEW_DATA_DIR, else %LOCALAPPDATA%\valreview\data).
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the players in a vrfkit export with their pawn, agent, team and K/D.
    Players {
        /// Directory written by `vrfkit export`.
        export_dir: PathBuf,
    },
    /// Write packet.md and deaths.jsonl for one player.
    Packet {
        /// Directory written by `vrfkit export`.
        export_dir: PathBuf,
        /// Account-UUID prefix, pawn id, or agent name.
        #[arg(long)]
        me: String,
        /// Output directory (default: current directory).
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
    /// Download agents, maps, weapons and seasons from valorant-api.com into the local cache.
    Sync,
    /// Write config.toml: vrfkit path, replay folder, and which account is you.
    Init {
        /// Your account UUID, or a prefix of one seen in scanned matches.
        /// Without it, init detects you once 2+ matches are scanned.
        #[arg(long)]
        me: Option<String>,
        /// Path to vrfkit.exe (v0.3.0 Windows release).
        #[arg(long)]
        vrfkit: Option<PathBuf>,
        /// Replay folder (default: %LOCALAPPDATA%\VALORANT\Saved\Demos).
        #[arg(long)]
        demos_dir: Option<PathBuf>,
    },
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
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let data_dir = paths::data_dir(cli.data_dir.as_deref())?;
    match cli.command {
        Command::Players { export_dir } => {
            let game = load(&export_dir)?;
            let catalog = load_catalog(&data_dir);
            println!(
                "{:<6} {:<5} {:<10} {:<38} {:>3}/{:<3}",
                "pawn", "team", "agent", "account", "K", "D"
            );
            for p in game.players() {
                let (k, d) = game.kd(p.pawn);
                let team = game
                    .teams
                    .side(p.pawn)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "?".into());
                let agent = catalog.agent_name(p.agent_id.as_deref());
                println!(
                    "{:<6} {:<5} {:<10} {:<38} {:>3}/{:<3}",
                    p.pawn, team, agent, p.subject, k, d
                );
            }
        }
        Command::Packet { export_dir, me, out } => {
            let game = load(&export_dir)?;
            let catalog = load_catalog(&data_dir);
            let me = select_player(game.players(), &catalog, &me)?;
            let review = Review::new(&game, &catalog, me);
            if review.map.is_none() && !catalog.is_builtin() {
                eprintln!(
                    "warning: map {} is not in the catalog; try `valreview sync`",
                    game.export.manifest.map_path.as_deref().unwrap_or("?")
                );
            }
            std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
            let md = out.join("packet.md");
            let jsonl = out.join("deaths.jsonl");
            std::fs::write(&md, review.packet_md()).with_context(|| format!("writing {}", md.display()))?;
            std::fs::write(&jsonl, review.deaths_jsonl()).with_context(|| format!("writing {}", jsonl.display()))?;
            println!(
                "wrote {} and {} ({} deaths)",
                md.display(),
                jsonl.display(),
                review.facts.len()
            );
        }
        Command::Init { me, vrfkit, demos_dir } => init(&data_dir, me, vrfkit, demos_dir)?,
        Command::Scan {
            demos_dir,
            keep_raw,
            retry_failed,
        } => scan_cmd(&data_dir, demos_dir, keep_raw, retry_failed)?,
        Command::Sync => {
            let path = paths::catalog_path(&data_dir);
            let cat =
                catalog::fetch().context("valorant-api.com sync failed; the existing cache (if any) is unchanged")?;
            cat.save(&path)?;
            let callouts: usize = cat.maps.iter().map(|m| m.callouts.len()).sum();
            println!(
                "synced game version {}: {} agents, {} maps ({} with a minimap projection, {callouts} callouts), {} weapons, {} seasons",
                cat.game_version.as_deref().unwrap_or("?"),
                cat.agents.len(),
                cat.maps.len(),
                cat.maps.iter().filter(|m| m.projection.is_some()).count(),
                cat.weapons.len(),
                cat.seasons.len()
            );
            println!("saved {}", path.display());
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
            println!("vrfkit {} at {} (SHA-256 verified)", v.version, v.path.display());
            cfg.vrfkit = Some(v.path);
        }
        Err(e) => eprintln!("warning: {e:#}"),
    }

    if let Some(d) = demos_dir {
        cfg.demos_dir = Some(d);
    }
    match cfg.demos_dir() {
        Some(d) if d.is_dir() => {
            let n = valreview_core::scan::find_replays(&d).map(|v| v.len()).unwrap_or(0);
            println!("replay folder {} ({n} replays)", d.display());
        }
        Some(d) => eprintln!(
            "warning: replay folder {} does not exist yet (download a replay in the client)",
            d.display()
        ),
        None => eprintln!("warning: no replay folder; pass --demos-dir"),
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
        println!("me = {}", cfg.me.as_deref().unwrap_or_default());
    } else if let Some(me) = &cfg.me {
        println!("me = {me} (already set; pass --me to change)");
    } else {
        match store.subjects_in_every_match()? {
            (n, _) if n < 2 => {
                println!("me: not set. Scan at least 2 matches, then run init again to detect it, or pass --me")
            }
            (n, s) if s.len() == 1 => {
                println!("me = {} (the only account in all {n} scanned matches)", s[0]);
                cfg.me = Some(s[0].clone());
            }
            (n, s) if s.is_empty() => println!("me: no account appears in all {n} matches; pass --me"),
            (n, s) => println!(
                "me: {} accounts appear in all {n} matches ({}); pass --me",
                s.len(),
                s.join(", ")
            ),
        }
    }

    cfg.save(data_dir)?;
    println!("saved {}", Config::path(data_dir).display());
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

fn scan_cmd(data_dir: &Path, demos_dir: Option<PathBuf>, keep_raw: bool, retry_failed: bool) -> Result<()> {
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
    println!("scanning {} with vrfkit {}", opts.demos_dir.display(), vrfkit.version);

    let results = scan(&mut store, &vrfkit, &catalog, &opts, |f| match &f.outcome {
        Outcome::Stored {
            match_id,
            map,
            warnings,
        } => {
            println!("  stored  {} -> {match_id} ({map})", f.file);
            for w in warnings {
                println!("          warning: {w}");
            }
        }
        Outcome::Failed { error } => println!("  FAILED  {}: {error}", f.file),
        Outcome::Duplicate { match_id } => println!("  dup     {} (same match as stored {match_id})", f.file),
        Outcome::AlreadyStored | Outcome::PreviouslyFailed { .. } => {}
    })?;

    let count = |pred: fn(&Outcome) -> bool| results.iter().filter(|r| pred(&r.outcome)).count();
    let stored = count(|o| matches!(o, Outcome::Stored { .. }));
    let failed = count(|o| matches!(o, Outcome::Failed { .. }));
    let seen = count(|o| matches!(o, Outcome::AlreadyStored | Outcome::Duplicate { .. }));
    let old_failed = count(|o| matches!(o, Outcome::PreviouslyFailed { .. }));
    println!(
        "{} replays: {stored} new, {seen} already done, {failed} failed",
        results.len()
    );
    if old_failed > 0 {
        println!("{old_failed} skipped because they failed before (--retry-failed to try again)");
    }
    if cfg.me.is_none()
        && let (n, s) = store.subjects_in_every_match()?
        && n >= 2
        && s.len() == 1
    {
        println!(
            "tip: {} is in all {n} matches; `valreview init` will set it as you",
            s[0]
        );
    }
    Ok(())
}

fn load(dir: &Path) -> Result<Game> {
    let export = Export::load(dir)?;
    match check_build(&export.manifest.replay_build) {
        BuildSupport::Supported => {}
        BuildSupport::Caveat(msg) | BuildSupport::Unsupported(msg) => eprintln!("warning: {msg}"),
    }
    let game = Game::new(export);
    for w in &game.warnings {
        eprintln!("warning: {w}");
    }
    Ok(game)
}

/// Never fails: falls back to built-in agent names with a one-line note.
fn load_catalog(data_dir: &Path) -> Catalog {
    let (cat, source) = Catalog::load_or_builtin(&paths::catalog_path(data_dir));
    match source {
        CatalogSource::Cache => {}
        CatalogSource::BuiltinNoCache => {
            eprintln!(
                "note: no catalog cache; using built-in agent names only (run `valreview sync` for maps and callouts)"
            )
        }
        CatalogSource::BuiltinBadCache(e) => {
            eprintln!("warning: ignoring unreadable catalog ({e}); using built-in agent names. Run `valreview sync`")
        }
    }
    cat
}
