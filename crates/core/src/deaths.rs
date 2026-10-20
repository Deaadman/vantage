//! Group A: per-death facts for one player (groups B to F add `facts::DeathContext`).

use serde::{Deserialize, Serialize};

use crate::analysis::Analysis;
use crate::catalog::{Catalog, MapInfo};
use crate::facts::{DeathContext, death_context};
use crate::game::{Game, Kill};

/// Positions are sampled this long before the death event.
pub const SAMPLE_BEFORE_DEATH_MS: u32 = 50;
/// A sample older than this (relative to the sample time) counts as missing.
pub const MAX_SAMPLE_AGE_MS: u32 = 1500;
/// A teammate kills the killer within this window, in the same round.
pub const TRADE_WINDOW_MS: u32 = 3000;

pub const DEATHS_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NearTeammate {
    pub pawn: u32,
    pub agent: String,
    pub dist_m: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeathFact {
    pub schema_version: u32,
    pub match_id: String,
    /// `roundStarted` number (0-based, as in the replay).
    pub round: Option<u32>,
    pub time_ms: u32,
    /// Seconds since `roundStarted`, which fires at the start of the buy phase.
    pub since_round_start_s: Option<f64>,
    pub victim_pawn: u32,
    pub killer_pawn: u32,
    pub killer_agent: String,
    pub self_kill: bool,
    pub first_death_of_round: bool,
    /// Teammates and enemies alive just before this death, from kill events only (revives not modelled).
    pub teammates_alive: u32,
    pub enemies_alive: u32,
    /// Up to two nearest living teammates with a known position, planar distance.
    pub nearest_teammates: Vec<NearTeammate>,
    pub dist_to_killer_m: Option<f64>,
    /// Angle between the victim's yaw and the bearing to the killer, 0 = looking straight at them.
    pub crosshair_offset_to_killer_deg: Option<f64>,
    pub speed_m_s: Option<f64>,
    pub traded_within_3s: bool,
    pub pos_cm: Option<[f64; 3]>,
    /// Minimap position in [0, 1]² (valorant-api.com projection); needs a synced catalog.
    pub minimap_uv: Option<[f64; 2]>,
    /// Nearest valorant-api.com callout point on the plane, e.g. `A Site`.
    pub nearest_callout: Option<String>,
    /// Index into the export's repeated-death-spot list, if this death is in one.
    pub cluster_id: Option<usize>,
    #[serde(flatten)]
    pub context: DeathContext,
}

impl DeathFact {
    pub fn nearest_teammate_m(&self) -> Option<f64> {
        self.nearest_teammates.first().map(|t| t.dist_m)
    }
}

/// A cross-team kill is traded when a teammate of the victim kills the killer
/// within `TRADE_WINDOW_MS` in the same round.
pub fn is_traded(game: &Game, kill: &Kill) -> bool {
    if kill.is_self_kill() {
        return false;
    }
    game.kills_in_round(kill.round_idx).any(|k| {
        k.victim == kill.killer
            && k.killer != kill.killer
            && game.teams.same_team(k.killer, kill.victim)
            && k.time_ms >= kill.time_ms
            && k.time_ms - kill.time_ms <= TRADE_WINDOW_MS
    })
}

pub fn death_facts(a: &Analysis, pawn: u32) -> Vec<DeathFact> {
    a.game
        .kills
        .iter()
        .filter(|k| k.victim == pawn)
        .map(|k| {
            let mut f = death_fact(a.game, a.catalog, a.map, k);
            f.context = death_context(a, k);
            f
        })
        .collect()
}

/// Minimap uv and nearest callout label for a world position.
pub fn place(map: Option<&MapInfo>, pos_cm: [f64; 3]) -> (Option<[f64; 2]>, Option<String>) {
    let Some(map) = map else { return (None, None) };
    let uv = map.uv(pos_cm).map(|uv| uv.map(|v| round_to(v, 4)));
    (uv, map.nearest_callout(pos_cm).map(|(c, _)| c.label()))
}

fn death_fact(game: &Game, catalog: &Catalog, map: Option<&MapInfo>, kill: &Kill) -> DeathFact {
    let victim = kill.victim;
    let round = game.round(kill);
    let dead_before: Vec<u32> = game
        .kills_in_round(kill.round_idx)
        .filter(|k| k.index < kill.index)
        .map(|k| k.victim)
        .collect();
    let first_death_of_round = dead_before.is_empty() && kill.round_idx.is_some();

    let alive = |p: &u32| !dead_before.contains(p);
    let teammates: Vec<u32> = game
        .team_of(victim)
        .into_iter()
        .filter(|&p| p != victim)
        .filter(alive)
        .collect();
    let enemies: Vec<u32> = game
        .players()
        .iter()
        .map(|p| p.pawn)
        .filter(|&p| {
            game.teams.side(victim).is_some() && game.teams.side(p).is_some() && !game.teams.same_team(p, victim)
        })
        .filter(alive)
        .collect();

    let t = kill.time_ms.saturating_sub(SAMPLE_BEFORE_DEATH_MS);
    let sample = |p: u32| game.export.tracks.get(&p).and_then(|tr| tr.at(t, MAX_SAMPLE_AGE_MS));
    let me = sample(victim);

    let mut nearest: Vec<NearTeammate> = match me {
        Some(me) => teammates
            .iter()
            .filter_map(|&p| {
                let s = sample(p)?;
                Some(NearTeammate {
                    pawn: p,
                    agent: catalog.agent_name(game.agent_id(p)),
                    dist_m: planar_cm(me.pos, s.pos) / 100.0,
                })
            })
            .collect(),
        None => Vec::new(),
    };
    nearest.sort_by(|a, b| a.dist_m.total_cmp(&b.dist_m));
    nearest.truncate(2);
    for n in &mut nearest {
        n.dist_m = round_to(n.dist_m, 2);
    }

    let killer_sample = (!kill.is_self_kill()).then(|| sample(kill.killer)).flatten();
    let (dist_to_killer_m, crosshair) = match (me, killer_sample) {
        (Some(me), Some(k)) => {
            let bearing = (k.pos[1] as f64 - me.pos[1] as f64)
                .atan2(k.pos[0] as f64 - me.pos[0] as f64)
                .to_degrees();
            (
                Some(round_to(planar_cm(me.pos, k.pos) / 100.0, 2)),
                Some(round_to(angle_diff_deg(me.yaw as f64, bearing), 1)),
            )
        }
        _ => (None, None),
    };

    let pos_cm = me.map(|s| s.pos.map(|v| round_to(v as f64, 0)));
    let (minimap_uv, nearest_callout) = pos_cm.map(|p| place(map, p)).unwrap_or_default();

    DeathFact {
        schema_version: DEATHS_SCHEMA_VERSION,
        match_id: game.export.manifest.match_id.clone(),
        round: round.map(|r| r.number),
        time_ms: kill.time_ms,
        since_round_start_s: round.map(|r| round_to((kill.time_ms - r.start_ms) as f64 / 1000.0, 3)),
        victim_pawn: victim,
        killer_pawn: kill.killer,
        killer_agent: catalog.agent_name(game.agent_id(kill.killer)),
        self_kill: kill.is_self_kill(),
        first_death_of_round,
        teammates_alive: teammates.len() as u32,
        enemies_alive: enemies.len() as u32,
        nearest_teammates: nearest,
        dist_to_killer_m,
        crosshair_offset_to_killer_deg: crosshair,
        speed_m_s: me.map(|s| round_to((s.vel[0] as f64).hypot(s.vel[1] as f64) / 100.0, 2)),
        traded_within_3s: is_traded(game, kill),
        pos_cm,
        minimap_uv,
        nearest_callout,
        cluster_id: None,
        context: DeathContext::default(),
    }
}

fn planar_cm(a: [f32; 3], b: [f32; 3]) -> f64 {
    (a[0] as f64 - b[0] as f64).hypot(a[1] as f64 - b[1] as f64)
}

/// Smallest absolute difference between two headings, in [0, 180].
fn angle_diff_deg(a: f64, b: f64) -> f64 {
    ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs()
}

pub(crate) fn round_to(v: f64, places: i32) -> f64 {
    let f = 10f64.powi(places);
    (v * f).round() / f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles() {
        assert_eq!(angle_diff_deg(350.0, 10.0), 20.0);
        assert_eq!(angle_diff_deg(10.0, -170.0), 180.0);
        assert_eq!(angle_diff_deg(90.0, 90.0), 0.0);
    }
}
