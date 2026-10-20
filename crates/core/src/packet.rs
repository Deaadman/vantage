//! `packet.md` (for an AI reviewer) and `deaths.jsonl` (stable schema).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Serialize;

use crate::agents::agent_name;
use crate::builds::{BuildSupport, check_build};
use crate::clusters::{self, DeathCluster, cluster_deaths};
use crate::deaths::{self, DeathFact, death_facts, is_traded};
use crate::export::Player;
use crate::game::Game;

pub const PACKET_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct DeathSummary {
    pub deaths: usize,
    pub first_deaths: usize,
    pub first_death_rounds: Vec<u32>,
    pub no_teammate_alive: usize,
    pub with_teammate_alive: usize,
    pub with_teammate_alive_traded: usize,
    pub teammate_within_10m: usize,
    pub teammate_within_10m_traded: usize,
    pub nearest_teammate_beyond_15m: usize,
    pub facing_killer_within_45deg: usize,
    pub facing_known: usize,
    pub side_or_behind_rounds: Vec<u32>,
    /// (killer pawn, agent, deaths), most first.
    pub killers: Vec<(u32, String, usize)>,
    pub median_since_round_start_s: Option<f64>,
}

impl DeathSummary {
    pub fn from_facts(facts: &[DeathFact]) -> Self {
        let tm_alive: Vec<_> = facts.iter().filter(|f| f.teammates_alive > 0).collect();
        let near10: Vec<_> = facts
            .iter()
            .filter(|f| f.nearest_teammate_m().is_some_and(|d| d <= 10.0))
            .collect();
        let facing: Vec<_> = facts
            .iter()
            .filter_map(|f| f.crosshair_offset_to_killer_deg.map(|a| (f, a)))
            .collect();

        let mut killers: BTreeMap<u32, (String, usize)> = BTreeMap::new();
        for f in facts.iter().filter(|f| !f.self_kill) {
            killers
                .entry(f.killer_pawn)
                .or_insert_with(|| (f.killer_agent.clone(), 0))
                .1 += 1;
        }
        let mut killers: Vec<_> = killers.into_iter().map(|(p, (a, n))| (p, a, n)).collect();
        killers.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));

        let mut times: Vec<f64> = facts.iter().filter_map(|f| f.since_round_start_s).collect();
        times.sort_by(f64::total_cmp);
        let median = match times.len() {
            0 => None,
            n if n % 2 == 1 => Some(times[n / 2]),
            n => Some((times[n / 2 - 1] + times[n / 2]) / 2.0),
        };

        DeathSummary {
            deaths: facts.len(),
            first_deaths: facts.iter().filter(|f| f.first_death_of_round).count(),
            first_death_rounds: facts
                .iter()
                .filter(|f| f.first_death_of_round)
                .filter_map(|f| f.round)
                .collect(),
            no_teammate_alive: facts.iter().filter(|f| f.teammates_alive == 0).count(),
            with_teammate_alive: tm_alive.len(),
            with_teammate_alive_traded: tm_alive.iter().filter(|f| f.traded_within_3s).count(),
            teammate_within_10m: near10.len(),
            teammate_within_10m_traded: near10.iter().filter(|f| f.traded_within_3s).count(),
            nearest_teammate_beyond_15m: facts
                .iter()
                .filter(|f| f.nearest_teammate_m().is_some_and(|d| d > 15.0))
                .count(),
            facing_killer_within_45deg: facing.iter().filter(|(_, a)| *a <= 45.0).count(),
            facing_known: facing.len(),
            side_or_behind_rounds: facing
                .iter()
                .filter(|(_, a)| *a > 45.0)
                .filter_map(|(f, _)| f.round)
                .collect(),
            killers,
            median_since_round_start_s: median.map(|m| deaths::round_to(m, 1)),
        }
    }
}

/// Everything the packet shows, computed once.
pub struct Review<'a> {
    pub game: &'a Game,
    pub me: &'a Player,
    pub facts: Vec<DeathFact>,
    pub clusters: Vec<DeathCluster>,
    pub summary: DeathSummary,
    /// Cross-team kills in the match, and how many were traded.
    pub match_kills: usize,
    pub match_kills_traded: usize,
}

impl<'a> Review<'a> {
    pub fn new(game: &'a Game, me: &'a Player) -> Self {
        let mut facts = death_facts(game, me.pawn);
        let clusters = cluster_deaths(&mut facts);
        let summary = DeathSummary::from_facts(&facts);
        let cross: Vec<_> = game.kills.iter().filter(|k| !k.is_self_kill()).collect();
        Review {
            game,
            me,
            match_kills_traded: cross.iter().filter(|k| is_traded(game, k)).count(),
            match_kills: cross.len(),
            facts,
            clusters,
            summary,
        }
    }

    pub fn deaths_jsonl(&self) -> String {
        let mut out = String::new();
        for f in &self.facts {
            out.push_str(&serde_json::to_string(f).expect("death facts serialise"));
            out.push('\n');
        }
        out
    }

    pub fn packet_md(&self) -> String {
        let g = self.game;
        let m = &g.export.manifest;
        let me_agent = agent_name(self.me.agent_id.as_deref());
        let s = &self.summary;
        let mut o = String::new();

        let _ = writeln!(
            o,
            "# Death review: {me_agent} (pawn {}) on {}",
            self.me.pawn,
            map_label(m.map_path.as_deref())
        );
        let _ = writeln!(o);
        let _ = writeln!(
            o,
            "packet_schema: {PACKET_SCHEMA_VERSION} · deaths_schema: {} · match: `{}`",
            deaths::DEATHS_SCHEMA_VERSION,
            m.match_id
        );
        let _ = writeln!(o);

        o.push_str(HOW_TO_READ);
        let _ = writeln!(o, "## Caveats");
        let _ = writeln!(o);
        match check_build(&m.replay_build) {
            BuildSupport::Supported => {}
            BuildSupport::Caveat(c) | BuildSupport::Unsupported(c) => {
                let _ = writeln!(o, "- Replay build: {c}.");
            }
        }
        for c in CAVEATS {
            let _ = writeln!(o, "- {c}");
        }
        for w in &g.warnings {
            let _ = writeln!(o, "- Data warning: {w}");
        }
        let _ = writeln!(o);

        let _ = writeln!(o, "## Match");
        let _ = writeln!(o);
        let (k, d) = g.kd(self.me.pawn);
        let _ = writeln!(
            o,
            "- Map: {} · build `{}` · {} rounds · {:.0} min",
            map_label(m.map_path.as_deref()),
            m.replay_build,
            g.rounds.len(),
            m.duration_ms as f64 / 60_000.0
        );
        let _ = writeln!(
            o,
            "- Reviewed player: {me_agent}, pawn {}, kills/deaths {k}/{d}",
            self.me.pawn
        );
        let _ = writeln!(o, "- Teammates: {}", self.roster(true));
        let _ = writeln!(o, "- Enemies: {}", self.roster(false));
        let _ = writeln!(
            o,
            "- Match-wide trade rate (all {} cross-team kills, both teams): {} traded within 3 s ({})",
            self.match_kills,
            self.match_kills_traded,
            pct(self.match_kills_traded, self.match_kills)
        );
        let _ = writeln!(o);

        let _ = writeln!(o, "## Death summary");
        let _ = writeln!(o);
        let _ = writeln!(o, "- Deaths: {}", s.deaths);
        let _ = writeln!(
            o,
            "- First death of the round (either team): {} (rounds {})",
            s.first_deaths,
            list(&s.first_death_rounds)
        );
        let _ = writeln!(o, "- Died with no teammate alive: {}", s.no_teammate_alive);
        let _ = writeln!(
            o,
            "- Died with a teammate alive: {}; traded within 3 s: {} ({})",
            s.with_teammate_alive,
            s.with_teammate_alive_traded,
            pct(s.with_teammate_alive_traded, s.with_teammate_alive)
        );
        let _ = writeln!(
            o,
            "- Nearest living teammate within 10 m: {}; traded: {} ({})",
            s.teammate_within_10m,
            s.teammate_within_10m_traded,
            pct(s.teammate_within_10m_traded, s.teammate_within_10m)
        );
        let _ = writeln!(
            o,
            "- Nearest living teammate beyond 15 m: {}",
            s.nearest_teammate_beyond_15m
        );
        let _ = writeln!(
            o,
            "- Crosshair within 45° of the killer: {} of {}; from the side or behind in rounds {}",
            s.facing_killer_within_45deg,
            s.facing_known,
            list(&s.side_or_behind_rounds)
        );
        let killers = s
            .killers
            .iter()
            .map(|(p, a, n)| format!("{a} (pawn {p}) ×{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(o, "- Killed by: {killers}");
        if let Some(med) = s.median_since_round_start_s {
            let _ = writeln!(
                o,
                "- Median time of death: {med} s after roundStarted (includes the buy phase)"
            );
        }
        let _ = writeln!(o);

        let _ = writeln!(o, "## Repeated death spots");
        let _ = writeln!(o);
        if self.clusters.is_empty() {
            let _ = writeln!(
                o,
                "None ({}+ deaths within {} m on the same floor).",
                clusters::MIN_DEATHS,
                clusters::RADIUS_CM / 100.0
            );
        } else {
            let _ = writeln!(
                o,
                "Spots with {}+ deaths within {} m of the spot's first death and within {} m vertically (same floor).",
                clusters::MIN_DEATHS,
                clusters::RADIUS_CM / 100.0,
                clusters::FLOOR_TOLERANCE_CM / 100.0
            );
            let _ = writeln!(o);
            for c in &self.clusters {
                let [x, y, z] = c.centroid_cm;
                let _ = writeln!(
                    o,
                    "- Spot {}: {} deaths around ({x}, {y}, z {z}) cm, rounds {}",
                    c.id,
                    c.deaths,
                    list(&c.rounds)
                );
            }
        }
        let _ = writeln!(o);

        let _ = writeln!(o, "## Deaths");
        let _ = writeln!(o);
        let _ = writeln!(
            o,
            "| rnd | t s | killer | 1st | tm/en alive | nearest teammates m | killer m | xhair ° | speed m/s | traded | spot | pos cm |"
        );
        let _ = writeln!(o, "|---|---|---|---|---|---|---|---|---|---|---|---|");
        for f in &self.facts {
            let near = if f.nearest_teammates.is_empty() {
                "-".to_owned()
            } else {
                f.nearest_teammates
                    .iter()
                    .map(|t| format!("{} {}", t.agent, t.dist_m))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let killer = if f.self_kill {
                "self".to_owned()
            } else {
                format!("{} ({})", f.killer_agent, f.killer_pawn)
            };
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} | {}/{} | {} | {} | {} | {} | {} | {} | {} |",
                opt(f.round),
                opt(f.since_round_start_s),
                killer,
                yes(f.first_death_of_round),
                f.teammates_alive,
                f.enemies_alive,
                near,
                opt(f.dist_to_killer_m),
                opt(f.crosshair_offset_to_killer_deg),
                opt(f.speed_m_s),
                yes(f.traded_within_3s),
                opt(f.cluster_id),
                f.pos_cm
                    .map(|[x, y, z]| format!("{x}, {y}, {z}"))
                    .unwrap_or_else(|| "-".into()),
            );
        }
        o
    }

    fn roster(&self, mine: bool) -> String {
        let g = self.game;
        g.players()
            .iter()
            .filter(|p| p.pawn != self.me.pawn)
            .filter(|p| g.teams.same_team(p.pawn, self.me.pawn) == mine)
            .map(|p| {
                let (k, d) = g.kd(p.pawn);
                format!("{} ({}, {k}/{d})", agent_name(p.agent_id.as_deref()), p.pawn)
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

const HOW_TO_READ: &str = "\
## How to read this

Facts about every death of one player in one Valorant match, measured from the
replay. There is no video and no judgement here; interpret the numbers.

- Rounds are numbered from 0, as the replay numbers them.
- `t s` is seconds since `roundStarted`, which fires at the start of the buy
  phase, so it includes the buy phase.
- Positions are sampled 50 ms before the death. Distances are planar (x/y), in
  metres. A position older than 1.5 s counts as unknown (`-`).
- `tm/en alive`: the victim's teammates and enemies alive just before the death.
- `nearest teammates m`: up to two living teammates with a known position.
- `xhair °`: angle between where the player was looking (yaw) and the direction
  to the killer; 0 means looking straight at them, over 45 means the killer was
  off to the side or behind.
- `traded`: a teammate killed the killer within 3 s in the same round.
- `spot`: the repeated-death-spot id from the list above.
- Pawn ids identify players; the replay has no display names.

";

const CAVEATS: &[&str] = &[
    "Alive counts come from kill events only; Sage and Clove revives are not modelled.",
    "Crosshair offset is derived: it assumes yaw is 0-360° measured like atan2(dy, dx). A check against velocity heading looked right but is not proven, and it ignores pitch.",
    "Teams are inferred from the kill graph (each kill joins opposite teams); `self` kills are excluded.",
    "Map name is the replay's level path until the valorant-api.com catalog is added.",
];

fn map_label(path: Option<&str>) -> String {
    match path {
        Some(p) => format!("`{p}`"),
        None => "unknown map".into(),
    }
}

fn pct(n: usize, d: usize) -> String {
    if d == 0 {
        "n/a".into()
    } else {
        format!("{:.0}%", 100.0 * n as f64 / d as f64)
    }
}

fn list(v: &[u32]) -> String {
    if v.is_empty() {
        "none".into()
    } else {
        v.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
    }
}

fn opt<T: ToString>(v: Option<T>) -> String {
    v.map(|v| v.to_string()).unwrap_or_else(|| "-".into())
}

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "" }
}
