//! Group G: match context (map, build, date, length, compositions, ping).

use serde::{Deserialize, Serialize};

use crate::builds::{BuildSupport, check_build};
use crate::catalog::Catalog;
use crate::game::Game;
use crate::time::{rfc3339_utc, ue_ticks_to_unix};

pub const ROLES: [&str; 4] = ["Duelist", "Initiator", "Controller", "Sentinel"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchContext {
    pub match_id: String,
    pub map_url: Option<String>,
    pub map_name: Option<String>,
    /// Game mode, e.g. `Swiftplay`. Competitive, Unrated and Premier share
    /// `Comp/Unrated/Premier`: the replay doesn't say which queue it was.
    #[serde(default)]
    pub mode: Option<String>,
    pub replay_build: String,
    /// Set when the build is outside, or at the edge of, what vrfkit verified.
    pub build_note: Option<String>,
    /// UTC, from `manifest.timestamp_ticks` (when the recording was written).
    pub played_at: Option<String>,
    pub duration_ms: u64,
    pub rounds: usize,
    pub teams: Vec<TeamContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamContext {
    /// Kill-graph side label (0 or 1), not attack or defense.
    pub side: Option<u8>,
    pub players: Vec<PlayerContext>,
    /// Roles nobody on the team played.
    pub missing_roles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerContext {
    pub subject: String,
    pub pawn: u32,
    pub agent_id: Option<String>,
    pub agent: String,
    pub role: Option<String>,
    pub kills: usize,
    pub deaths: usize,
    /// Replicated `BombPlayerState.Ping` (assumed ms; the scoreboard value).
    pub ping_median: Option<u32>,
    pub ping_p95: Option<u32>,
    pub ping_max: Option<u32>,
    /// Extra pawns from reconnects. `ConnectionStatus` is not in 13.08 exports,
    /// so a disconnect without a new pawn is invisible.
    pub reconnects: usize,
}

impl MatchContext {
    pub fn new(game: &Game, catalog: &Catalog) -> Self {
        let m = &game.export.manifest;
        let map = m.map_path.as_deref().and_then(|p| catalog.map_by_url(p));
        let build_note = match check_build(&m.replay_build, &crate::vrfkit::active().decodes()) {
            BuildSupport::Supported => None,
            BuildSupport::Unsupported(s) => Some(s),
        };

        let mut sides: Vec<Option<u8>> = game.players().iter().map(|p| game.teams.side(p.pawn)).collect();
        sides.sort();
        sides.dedup();
        let teams = sides
            .into_iter()
            .map(|side| {
                let players: Vec<PlayerContext> = game
                    .players()
                    .iter()
                    .filter(|p| game.teams.side(p.pawn) == side)
                    .map(|p| {
                        let (kills, deaths) = game.kd(p.pawn);
                        let mut pings = game
                            .export
                            .fields
                            .pings
                            .get(&p.actor_net_guid)
                            .cloned()
                            .unwrap_or_default();
                        pings.sort_unstable();
                        let pct = |q: f64| {
                            (!pings.is_empty()).then(|| pings[((pings.len() - 1) as f64 * q).round() as usize])
                        };
                        let agent = p.agent_id.as_deref().and_then(|id| catalog.agent(id));
                        PlayerContext {
                            subject: p.subject.clone(),
                            pawn: p.pawn,
                            agent_id: p.agent_id.clone(),
                            agent: catalog.agent_name(p.agent_id.as_deref()),
                            role: agent.and_then(|a| a.role.clone()),
                            kills,
                            deaths,
                            ping_median: pct(0.5),
                            ping_p95: pct(0.95),
                            ping_max: pings.last().copied(),
                            reconnects: p.pawns.len().saturating_sub(1),
                        }
                    })
                    .collect();
                let missing_roles = ROLES
                    .iter()
                    .filter(|r| !players.iter().any(|p| p.role.as_deref() == Some(**r)))
                    .map(|r| (*r).to_owned())
                    .collect();
                TeamContext {
                    side,
                    players,
                    missing_roles,
                }
            })
            .collect();

        MatchContext {
            match_id: m.match_id.clone(),
            map_url: m.map_path.clone(),
            map_name: map.map(|mp| mp.name.clone()),
            mode: game.mode_folder().map(|f| catalog.mode_name(f)),
            replay_build: m.replay_build.clone(),
            build_note,
            played_at: m.timestamp_ticks.map(|t| rfc3339_utc(ue_ticks_to_unix(t))),
            duration_ms: m.duration_ms,
            rounds: game.rounds.len(),
            teams,
        }
    }

    pub fn player(&self, subject: &str) -> Option<&PlayerContext> {
        self.teams
            .iter()
            .flat_map(|t| &t.players)
            .find(|p| p.subject == subject)
    }

    /// One line per team: `Clove (Controller), Sova (Initiator), ... · no Sentinel`.
    pub fn composition_line(&self, team: &TeamContext) -> String {
        let agents = team
            .players
            .iter()
            .map(|p| format!("{} ({})", p.agent, p.role.as_deref().unwrap_or("?")))
            .collect::<Vec<_>>()
            .join(", ");
        if team.missing_roles.is_empty() {
            agents
        } else {
            format!("{agents} · no {}", team.missing_roles.join(", no "))
        }
    }
}
