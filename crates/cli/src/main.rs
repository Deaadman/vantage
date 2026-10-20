use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use valreview_core::agents::agent_name;
use valreview_core::builds::{BuildSupport, check_build};
use valreview_core::select::select_player;
use valreview_core::{Export, Game, Review};

#[derive(Parser)]
#[command(
    name = "valreview",
    version,
    about = "Facts about your own Valorant replays, for review"
)]
struct Cli {
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
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Players { export_dir } => {
            let game = load(&export_dir)?;
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
                println!(
                    "{:<6} {:<5} {:<10} {:<38} {:>3}/{:<3}",
                    p.pawn,
                    team,
                    agent_name(p.agent_id.as_deref()),
                    p.subject,
                    k,
                    d
                );
            }
        }
        Command::Packet { export_dir, me, out } => {
            let game = load(&export_dir)?;
            let me = select_player(game.players(), &me)?;
            let review = Review::new(&game, me);
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
    }
    Ok(())
}

fn load(dir: &std::path::Path) -> Result<Game> {
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
