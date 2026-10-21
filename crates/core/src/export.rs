//! `export.md` (for an AI reviewer), `deaths.jsonl` and `rounds.jsonl`.
//!
//! Everything an export shows is collected into a [`MatchReview`], which is
//! serialisable: `scan` stores one per player, so `export --match` and
//! `trends` work after the raw export is deleted.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::analysis::{Analysis, slot_key, slot_name};
use crate::builds::{BuildSupport, check_build};
use crate::catalog::Catalog;
use crate::clusters::{self, DeathCluster, cluster_deaths};
use crate::context::MatchContext;
use crate::deaths::{self, DeathFact, MAX_SAMPLE_AGE_MS, death_facts, is_traded};
use crate::facts::{RoundFact, range_band, round_facts};
use crate::game::Game;
use crate::items::ItemKind;
use crate::vrfkit_export::Player;

pub const EXPORT_SCHEMA_VERSION: u32 = 2;
/// Version of the stored [`MatchReview`] JSON.
pub const REVIEW_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// A kill between the reviewed player and an enemy, from the player's side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Duel {
    pub round: Option<u32>,
    pub won: bool,
    /// What the reviewed player had in hand.
    pub weapon: String,
    pub range: Option<String>,
    pub enemy_agent: String,
    /// Where the reviewed player stood: minimap uv and nearest callout
    /// (missing in reviews stored before they were recorded).
    #[serde(default)]
    pub uv: Option<[f64; 2]>,
    #[serde(default)]
    pub callout: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RosterEntry {
    pub agent: String,
    pub pawn: u32,
    pub kills: usize,
    pub deaths: usize,
}

/// Everything one player's export shows for one match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchReview {
    pub schema_version: u32,
    pub subject: String,
    pub pawn: u32,
    pub agent: String,
    pub agent_id: Option<String>,
    /// Ability names by key (C, Q, E, X) for this agent, when the catalog knows them.
    pub abilities: BTreeMap<String, String>,
    pub context: MatchContext,
    /// Map path from the replay, for when the catalog does not know the map.
    pub map_path: Option<String>,
    pub map_has_callouts: bool,
    pub catalog_synced: bool,
    pub warnings: Vec<String>,
    pub kills: usize,
    pub deaths: usize,
    pub teammates: Vec<RosterEntry>,
    pub enemies: Vec<RosterEntry>,
    /// Cross-team kills in the match (both teams), and how many were traded.
    pub match_kills: usize,
    pub match_kills_traded: usize,
    /// (traded, deaths with a teammate alive) for me and for my teammates.
    pub my_trades: (usize, usize),
    pub team_trades: (usize, usize),
    pub death_facts: Vec<DeathFact>,
    pub round_facts: Vec<RoundFact>,
    pub duels: Vec<Duel>,
    pub clusters: Vec<DeathCluster>,
    pub summary: DeathSummary,
}

impl MatchReview {
    /// Builds a review from a loaded export.
    pub fn new(a: &Analysis, context: &MatchContext, me: &Player) -> Self {
        let g = a.game;
        let catalog = a.catalog;
        let mut facts = death_facts(a, me.pawn);
        let clusters = cluster_deaths(&mut facts, a.map);
        let summary = DeathSummary::from_facts(&facts);
        let cross: Vec<_> = g.kills.iter().filter(|k| !k.is_self_kill()).collect();
        let roster = |mine: bool| {
            g.players()
                .iter()
                .filter(|p| p.pawn != me.pawn && g.teams.same_team(p.pawn, me.pawn) == mine)
                .map(|p| {
                    let (kills, deaths) = g.kd(p.pawn);
                    RosterEntry {
                        agent: catalog.agent_name(p.agent_id.as_deref()),
                        pawn: p.pawn,
                        kills,
                        deaths,
                    }
                })
                .collect()
        };
        let abilities = [3, 4, 5, 9]
            .iter()
            .filter_map(|&s| {
                let slot = slot_name(s)?;
                let name = catalog.ability_name(me.agent_id.as_deref()?, slot)?;
                Some((slot_key(slot).to_owned(), name.to_owned()))
            })
            .collect();
        let (kills, deaths) = g.kd(me.pawn);
        MatchReview {
            schema_version: REVIEW_SCHEMA_VERSION,
            subject: me.subject.clone(),
            pawn: me.pawn,
            agent: catalog.agent_name(me.agent_id.as_deref()),
            agent_id: me.agent_id.clone(),
            abilities,
            context: context.clone(),
            map_path: g.export.manifest.map_path.clone(),
            map_has_callouts: a.map.is_some_and(|m| !m.callouts.is_empty()),
            catalog_synced: !catalog.is_builtin(),
            warnings: g.warnings.clone(),
            kills,
            deaths,
            teammates: roster(true),
            enemies: roster(false),
            match_kills_traded: cross.iter().filter(|k| is_traded(g, k)).count(),
            match_kills: cross.len(),
            my_trades: trades_with_teammate_alive(g, me.pawn),
            team_trades: g
                .team_of(me.pawn)
                .into_iter()
                .filter(|&p| p != me.pawn)
                .map(|p| trades_with_teammate_alive(g, p))
                .fold((0, 0), |x, y| (x.0 + y.0, x.1 + y.1)),
            round_facts: round_facts(a, me.pawn),
            duels: duels(a, me.pawn),
            death_facts: facts,
            clusters,
            summary,
        }
    }

    /// Convenience for a single export: analyse and review one player.
    pub fn from_game(game: &Game, catalog: &Catalog, me: &Player) -> Self {
        let a = Analysis::new(game, catalog);
        let ctx = MatchContext::new(game, catalog);
        Self::new(&a, &ctx, me)
    }

    pub fn deaths_jsonl(&self) -> String {
        jsonl(&self.death_facts)
    }

    pub fn rounds_jsonl(&self) -> String {
        jsonl(&self.round_facts)
    }

    pub fn map_name(&self) -> String {
        match (&self.context.map_name, &self.map_path) {
            (Some(name), _) => name.clone(),
            (None, Some(path)) => format!("`{path}`"),
            (None, None) => "unknown map".into(),
        }
    }

    pub fn export_md(&self) -> String {
        let c = &self.context;
        let map_name = self.map_name();
        let s = &self.summary;
        let mut o = String::new();

        let _ = writeln!(o, "# Death review: {} (pawn {}) on {map_name}", self.agent, self.pawn);
        let _ = writeln!(o);
        let _ = writeln!(
            o,
            "export_schema: {EXPORT_SCHEMA_VERSION} · deaths_schema: {} · rounds_schema: {} · match: `{}`",
            deaths::DEATHS_SCHEMA_VERSION,
            crate::facts::ROUNDS_SCHEMA_VERSION,
            c.match_id
        );
        let _ = writeln!(o);

        o.push_str(HOW_TO_READ);
        let _ = writeln!(o, "## Caveats");
        let _ = writeln!(o);
        if let BuildSupport::Unsupported(m) = check_build(&c.replay_build, &crate::vrfkit::active().decodes()) {
            let _ = writeln!(o, "- Replay build: {m}.");
        }
        for cv in CAVEATS.iter().chain(crate::sections::PHASE4_CAVEATS) {
            let _ = writeln!(o, "- {cv}");
        }
        if self.map_has_callouts {
            let _ = writeln!(o, "- {CALLOUT_CAVEAT}");
        } else if c.map_name.is_none() {
            let _ = writeln!(
                o,
                "- Map `{}` is not in the valorant-api.com catalog{}; no minimap positions or callouts.",
                self.map_path.as_deref().unwrap_or("?"),
                if self.catalog_synced {
                    ""
                } else {
                    " (no catalog synced: run `vantage sync`)"
                }
            );
        }
        for w in &self.warnings {
            let _ = writeln!(o, "- Data warning: {w}");
        }
        let _ = writeln!(o);

        let _ = writeln!(o, "## Match");
        let _ = writeln!(o);
        let _ = writeln!(
            o,
            "- Map: {map_name} · build `{}` · {} rounds · {:.0} min · played {}",
            c.replay_build,
            c.rounds,
            c.duration_ms as f64 / 60_000.0,
            c.played_at.as_deref().unwrap_or("unknown")
        );
        let me_ctx = c.player(&self.subject);
        let role = me_ctx
            .and_then(|p| p.role.as_deref())
            .map(|r| format!(" ({r})"))
            .unwrap_or_default();
        let ping = match me_ctx.map(|p| (p.ping_median, p.ping_p95)) {
            Some((Some(med), Some(p95))) => format!(", ping median {med} / p95 {p95}"),
            _ => String::new(),
        };
        let reconnects = match me_ctx.map(|p| p.reconnects) {
            Some(n) if n > 0 => format!(", reconnected {n}×"),
            _ => String::new(),
        };
        let _ = writeln!(
            o,
            "- Reviewed player: {}{role}, pawn {}, kills/deaths {}/{}{ping}{reconnects}",
            self.agent, self.pawn, self.kills, self.deaths
        );
        let roster = |v: &[RosterEntry]| {
            v.iter()
                .map(|p| format!("{} ({}, {}/{})", p.agent, p.pawn, p.kills, p.deaths))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let _ = writeln!(o, "- Teammates: {}", roster(&self.teammates));
        let _ = writeln!(o, "- Enemies: {}", roster(&self.enemies));
        let my_side = c
            .teams
            .iter()
            .find(|t| t.players.iter().any(|p| p.subject == self.subject))
            .map(|t| t.side);
        for team in &c.teams {
            let label = if Some(team.side) == my_side {
                "Your composition"
            } else {
                "Enemy composition"
            };
            let _ = writeln!(o, "- {label}: {}", c.composition_line(team));
        }
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
            for cl in &self.clusters {
                let [x, y, z] = cl.centroid_cm;
                let near = cl
                    .nearest_callout
                    .as_deref()
                    .map(|n| format!("near {n}, "))
                    .unwrap_or_default();
                let uv = cl
                    .minimap_uv
                    .map(|[u, v]| format!(", minimap uv ({u}, {v})"))
                    .unwrap_or_default();
                let _ = writeln!(
                    o,
                    "- Spot {}: {} deaths {near}around ({x}, {y}, z {z}) cm{uv}, rounds {}",
                    cl.id,
                    cl.deaths,
                    list(&cl.rounds)
                );
            }
        }
        let _ = writeln!(o);

        crate::sections::render(self, &mut o);

        let _ = writeln!(o, "## Deaths");
        let _ = writeln!(o);
        let _ = writeln!(
            o,
            "| rnd | t s | killer | killed with | in hand | 1st | tm/en alive | nearest teammates m | killer m | xhair ° | speed m/s | traded | spot | callout | pos cm |"
        );
        let _ = writeln!(o, "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
        for f in &self.death_facts {
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
                "| {} | {} | {} | {} | {} | {} | {}/{} | {} | {} | {} | {} | {} | {} | {} | {} |",
                opt(f.round),
                opt(f.since_round_start_s),
                killer,
                f.context.killer_weapon.as_deref().unwrap_or("-"),
                f.context.weapon_in_hand.as_deref().unwrap_or("-"),
                yes(f.first_death_of_round),
                f.teammates_alive,
                f.enemies_alive,
                near,
                opt(f.dist_to_killer_m),
                opt(f.crosshair_offset_to_killer_deg),
                opt(f.speed_m_s),
                yes(f.traded_within_3s),
                opt(f.cluster_id),
                f.nearest_callout.as_deref().unwrap_or("-"),
                f.pos_cm
                    .map(|[x, y, z]| format!("{x}, {y}, {z}"))
                    .unwrap_or_else(|| "-".into()),
            );
        }
        let _ = writeln!(o);
        crate::sections::rounds_table(self, &mut o);
        o
    }
}

fn jsonl<T: Serialize>(rows: &[T]) -> String {
    let mut out = String::new();
    for r in rows {
        out.push_str(&serde_json::to_string(r).expect("facts serialise"));
        out.push('\n');
    }
    out
}

/// (traded, deaths with a teammate alive) for one player.
fn trades_with_teammate_alive(g: &Game, pawn: u32) -> (usize, usize) {
    let mut n = 0;
    let mut t = 0;
    for k in g.kills.iter().filter(|k| k.victim == pawn && !k.is_self_kill()) {
        let dead_before: Vec<u32> = g
            .kills_in_round(k.round_idx)
            .filter(|x| x.index < k.index)
            .map(|x| x.victim)
            .collect();
        if g.team_of(pawn).iter().any(|p| *p != pawn && !dead_before.contains(p)) {
            n += 1;
            t += usize::from(is_traded(g, k));
        }
    }
    (t, n)
}

fn duels(a: &Analysis, me: u32) -> Vec<Duel> {
    let g = a.game;
    g.kills
        .iter()
        .filter(|k| !k.is_self_kill() && (k.killer == me || k.victim == me))
        .map(|k| {
            let enemy = if k.killer == me { k.victim } else { k.killer };
            let start = k.round_idx.map(|i| a.rounds[i].start_ms).unwrap_or(0);
            let weapon = a
                .in_hand_before(me, k.time_ms + u32::from(k.killer == me), start)
                .and_then(|w| a.item(w))
                .map(|i| {
                    if i.kind == ItemKind::Ability {
                        "ability".to_owned()
                    } else {
                        i.name.clone()
                    }
                })
                .unwrap_or_else(|| "?".into());
            let pos = |p: u32| {
                g.export
                    .tracks
                    .get(&p)
                    .and_then(|tr| tr.at(k.time_ms.saturating_sub(50), MAX_SAMPLE_AGE_MS))
            };
            let range = pos(me).zip(pos(enemy)).map(|(x, y)| {
                range_band((x.pos[0] as f64 - y.pos[0] as f64).hypot(x.pos[1] as f64 - y.pos[1] as f64) / 100.0)
                    .to_owned()
            });
            let (uv, callout) = pos(me).map_or((None, None), |s| {
                deaths::place(a.map, [s.pos[0] as f64, s.pos[1] as f64, s.pos[2] as f64])
            });
            Duel {
                round: g.round(k).map(|r| r.number),
                won: k.killer == me,
                weapon,
                range,
                enemy_agent: a.catalog.agent_name(g.agent_id(enemy)),
                uv,
                callout,
            }
        })
        .collect()
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
- `callout`: the nearest named map location (valorant-api.com), e.g. `A Site`.
- `killed with` / `in hand`: the killer's weapon or ability, and what I was holding.
- Sections Gunfights to Rounds summarise the whole match for me; the rounds table
  has one row per round (`buy` = buy type and best gun, `loadout` = credits of guns
  and armour owned, `head/hits` = headshot hits of all hits on players, `first` =
  FK first kill / FD first death of the round, `casts` by key C/Q/E/X).
- Pawn ids identify players; the replay has no display names.

";

const CALLOUT_CAVEAT: &str = "Callout is derived: the nearest valorant-api.com callout point on the plane, not a region outline, and it ignores floors.";

const CAVEATS: &[&str] = &[
    "Alive counts come from kill events only; Sage and Clove revives are not modelled.",
    "Crosshair offset is derived: it assumes yaw is 0-360° measured like atan2(dy, dx). A check against velocity heading looked right but is not proven, and it ignores pitch.",
    "Ping is the replicated scoreboard value (assumed ms). Disconnects are only visible when they gave a new pawn.",
    "Teams are inferred from the kill graph (each kill joins opposite teams); `self` kills are excluded.",
];

pub(crate) fn pct(n: usize, d: usize) -> String {
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
