use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use valreview_core::builds::{BuildSupport, check_build};
use valreview_core::catalog::{self, CatalogSource};
use valreview_core::paths;
use valreview_core::select::select_player;
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
