//! Groups B to F: per-death context and per-round facts for one player.
//!
//! Tags in the doc comments: R = read from the replay, D = derived with a
//! caveat (the export states each caveat once).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::analysis::{Analysis, Side, slot_key, slot_name};
use crate::deaths::{MAX_SAMPLE_AGE_MS, round_to};
use crate::game::Kill;
use crate::items::ItemKind;

/// Standard round timers (s): the action phase after the barrier, and the spike after a plant.
pub const ROUND_TIMER_S: f64 = 100.0;
pub const SPIKE_TIMER_S: f64 = 45.0;
/// Speed above which a shot counts as moving (m/s). Walking is about 3, running 5.4.
pub const MOVING_SHOT_M_S: f64 = 1.5;
/// Casts this long before a death count as "used just before dying".
pub const CAST_BEFORE_DEATH_MS: u32 = 5000;
/// A death within this distance of the player's own earlier kill spot in the round.
pub const SAME_SPOT_M: f64 = 5.0;
/// Range bands for duel records, metres.
pub const RANGE_BANDS: [(f64, &str); 4] = [
    (10.0, "0-10 m"),
    (20.0, "10-20 m"),
    (30.0, "20-30 m"),
    (f64::INFINITY, "30+ m"),
];

pub fn region_name(r: Option<i64>) -> &'static str {
    match r {
        Some(0) => "body",
        Some(1) => "head",
        Some(2) => "legs",
        _ => "other",
    }
}

pub fn range_band(m: f64) -> &'static str {
    RANGE_BANDS
        .iter()
        .find(|(max, _)| m < *max)
        .map(|(_, n)| *n)
        .unwrap_or("30+ m")
}

/// Group B to F context of one death.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeathContext {
    /// R. Attack or defense this round (side from who carried the spike).
    pub side: Option<Side>,
    /// R. Seconds since the barrier dropped (live-round time).
    pub since_barrier_s: Option<f64>,
    /// R. Seconds until the round was decided.
    pub before_round_end_s: Option<f64>,
    /// D. Round clock left: 100 s after the barrier, or 45 s after a plant.
    pub clock_left_s: Option<f64>,
    /// R. The spike was planted before this death.
    pub post_plant: bool,
    /// R. The victim owned the spike.
    pub carrying_spike: bool,
    /// R. More enemies alive than the victim's team (victim included).
    pub outnumbered: bool,
    /// R. Weapon or ability that dealt the killing blow.
    pub killer_weapon: Option<String>,
    /// R. head, body, legs or other.
    pub killing_blow_region: Option<String>,
    pub killing_blow_wallbang: bool,
    /// R. What the victim had in hand.
    pub weapon_in_hand: Option<String>,
    pub knife_out: bool,
    /// R. Rounds left in the in-hand gun's magazine, and its size.
    pub magazine_ammo: Option<i64>,
    pub magazine_size: Option<u32>,
    /// R. Damage between victim and killer this round, up to the death.
    pub duel_damage_dealt: f64,
    pub duel_damage_taken: f64,
    pub duel_hits_dealt: u32,
    pub duel_hits_taken: u32,
    /// R. Whether the victim hit the killer first (None if neither hit before the kill).
    pub hit_killer_first: Option<bool>,
    /// R. From the first hit between the two to the death.
    pub duel_ms: Option<u32>,
    /// D. Crosshair pitch minus the elevation to the killer (degrees; + = aiming above).
    pub vertical_aim_offset_deg: Option<f64>,
    /// R/D. Ult points at death, and whether that was the most this player ever held (ult ready).
    pub ult_points: Option<i64>,
    pub ult_ready: Option<bool>,
    /// R. Credit value of guns and armour owned at death.
    pub inventory_value: Option<u32>,
    /// D. Died within 5 m of the spot of one's own kill earlier this round.
    pub same_spot_as_own_kill: bool,
    /// R. Own ability casts in the 5 s before death.
    pub casts_last_5s: u32,
    /// D. Casts in the last 5 s and no damage dealt after them.
    pub utility_unused_value: bool,
    /// R. Different enemies who damaged the victim in the 3 s before death
    /// (missing in facts stored before it was recorded).
    #[serde(default)]
    pub attackers_last_3s: u32,
}

pub fn death_context(a: &Analysis, kill: &Kill) -> DeathContext {
    let g = a.game;
    let v = kill.victim;
    let t = kill.time_ms;
    let round = kill.round_idx.map(|i| &a.rounds[i]);
    let start = round.map(|r| r.start_ms).unwrap_or(0);
    let mut c = DeathContext {
        side: round.and_then(|r| r.side_of(a.side(v))),
        since_barrier_s: round
            .and_then(|r| r.buy_end_ms)
            .filter(|&b| t >= b)
            .map(|b| secs(t - b)),
        before_round_end_s: round.filter(|r| r.end_ms >= t).map(|r| secs(r.end_ms - t)),
        post_plant: round.and_then(|r| r.plant.as_ref()).is_some_and(|p| p.time_ms < t),
        clock_left_s: round.and_then(|r| match r.plant.as_ref().filter(|p| p.time_ms < t) {
            Some(p) => Some(round_to(SPIKE_TIMER_S - secs(t - p.time_ms), 1)),
            None => r
                .buy_end_ms
                .filter(|&b| t >= b)
                .map(|b| round_to(ROUND_TIMER_S - secs(t - b), 1)),
        }),
        carrying_spike: round.is_some_and(|r| {
            a.items
                .iter()
                .any(|(&gid, it)| it.kind == ItemKind::Spike && a.owner_at(gid, t.saturating_sub(1)) == Some(v))
                && r.spike_holders.iter().any(|h| h.1 == v)
        }),
        ..Default::default()
    };

    let dead_before: Vec<u32> = g
        .kills_in_round(kill.round_idx)
        .filter(|k| k.index < kill.index)
        .map(|k| k.victim)
        .collect();
    let alive = |p: &u32| !dead_before.contains(p);
    let my_team = g.team_of(v).into_iter().filter(alive).count();
    let enemies = g
        .players()
        .iter()
        .map(|p| p.pawn)
        .filter(|&p| a.side(p).is_some() && a.side(p) != a.side(v))
        .filter(alive)
        .count();
    // With a player of unknown team the counts are short; don't claim it.
    c.outnumbered = g.teams_complete() && enemies > my_team;

    // Killing blow: the damage invocation on the victim that killed, at the death.
    if let Some(h) = a
        .hits
        .iter()
        .rfind(|h| h.killed && h.victim == Some(v) && h.time_ms.abs_diff(t) <= 150)
    {
        c.killer_weapon = a.item_label(h.item, kill.killer);
        c.killing_blow_region = Some(region_name(h.region).to_owned());
        c.killing_blow_wallbang = h.wallbang;
    }

    if let Some(item) = a.in_hand_before(v, t, start) {
        let it = a.item(item);
        c.weapon_in_hand = it.map(|i| i.name.clone());
        c.knife_out = it.is_some_and(|i| i.kind == ItemKind::Melee);
        if it.is_some_and(|i| i.is_gun()) {
            c.magazine_ammo = a.magazine_at(item, t);
            c.magazine_size = it.and_then(|i| i.magazine);
        }
    }

    if !kill.is_self_kill() {
        let duel: Vec<_> = a
            .hits
            .iter()
            .filter(|h| h.round_idx == kill.round_idx && h.time_ms <= t)
            .filter(|h| {
                (h.attacker == Some(v) && h.victim == Some(kill.killer))
                    || (h.attacker == Some(kill.killer) && h.victim == Some(v))
            })
            .collect();
        for h in &duel {
            if h.attacker == Some(v) {
                c.duel_damage_dealt += h.damage;
                c.duel_hits_dealt += 1;
            } else {
                c.duel_damage_taken += h.damage;
                c.duel_hits_taken += 1;
            }
        }
        c.duel_damage_dealt = round_to(c.duel_damage_dealt, 1);
        c.duel_damage_taken = round_to(c.duel_damage_taken, 1);
        if let Some(first) = duel.first() {
            c.hit_killer_first = Some(first.attacker == Some(v));
            c.duel_ms = Some(t.saturating_sub(first.time_ms));
        }

        let sample = |p: u32| {
            g.export
                .tracks
                .get(&p)
                .and_then(|tr| tr.at(t.saturating_sub(50), MAX_SAMPLE_AGE_MS))
        };
        if let (Some(me), Some(k)) = (sample(v), sample(kill.killer)) {
            let dist = (k.pos[0] as f64 - me.pos[0] as f64).hypot(k.pos[1] as f64 - me.pos[1] as f64);
            if dist > 1.0 {
                let elevation = (k.pos[2] as f64 - me.pos[2] as f64).atan2(dist).to_degrees();
                let pitch = if me.pitch > 180.0 {
                    me.pitch as f64 - 360.0
                } else {
                    me.pitch as f64
                };
                c.vertical_aim_offset_deg = Some(round_to(pitch - elevation, 1));
            }
        }
    }

    c.ult_points = a.ult_points_at(v, t);
    c.ult_ready = match (c.ult_points, a.ult_cost.get(&v)) {
        (Some(p), Some(&cost)) if cost > 0 => Some(p >= cost),
        _ => None,
    };
    c.inventory_value = Some(
        a.owned_at(v, t)
            .iter()
            .filter_map(|&i| a.item(i))
            .filter_map(|i| i.cost)
            .sum(),
    );

    // Same spot as own kill earlier in the round.
    let my_pos = g
        .export
        .tracks
        .get(&v)
        .and_then(|tr| tr.at(t.saturating_sub(50), MAX_SAMPLE_AGE_MS))
        .map(|s| s.pos);
    if let Some(p) = my_pos {
        c.same_spot_as_own_kill = g
            .kills_in_round(kill.round_idx)
            .filter(|k| k.killer == v && k.time_ms < t && !k.is_self_kill())
            .any(|k| {
                g.export
                    .tracks
                    .get(&v)
                    .and_then(|tr| tr.at(k.time_ms, MAX_SAMPLE_AGE_MS))
                    .is_some_and(|s| {
                        (s.pos[0] as f64 - p[0] as f64).hypot(s.pos[1] as f64 - p[1] as f64) / 100.0 <= SAME_SPOT_M
                            && (s.pos[2] - p[2]).abs() <= 150.0
                    })
            });
    }

    let recent: Vec<u32> = a
        .casts
        .iter()
        .filter(|cast| cast.pawn == v && cast.slot != 9)
        .filter_map(|cast| cast.time_ms)
        .filter(|&ct| ct <= t && t - ct <= CAST_BEFORE_DEATH_MS)
        .collect();
    c.casts_last_5s = recent.len() as u32;
    if let Some(&first) = recent.iter().min() {
        c.utility_unused_value = !a
            .hits
            .iter()
            .any(|h| h.attacker == Some(v) && h.time_ms >= first && h.time_ms <= t && h.victim.is_some());
    }

    let mut attackers: Vec<u32> = a
        .hits
        .iter()
        .filter(|h| h.victim == Some(v) && h.time_ms <= t && t - h.time_ms <= ATTACKERS_WINDOW_MS)
        .filter_map(|h| h.attacker)
        .filter(|&p| p != v && a.side(p).is_some() && a.side(p) != a.side(v))
        .collect();
    attackers.sort_unstable();
    attackers.dedup();
    c.attackers_last_3s = attackers.len() as u32;
    c
}

/// How far back `attackers_last_3s` looks.
const ATTACKERS_WINDOW_MS: u32 = 3000;

// ---------------------------------------------------------------- rounds

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoundEconomy {
    /// R. Credits at the start of the buy phase.
    pub credits_start: Option<i64>,
    /// R. Credits when the barrier dropped.
    pub credits_after_buy: Option<i64>,
    /// D. start - after buy: includes buying for teammates, excludes drops received.
    pub spent: Option<i64>,
    /// R. Credit value of guns and armour owned 2 s after the barrier dropped.
    pub loadout_value: u32,
    pub primary: Option<String>,
    pub armor: Option<String>,
    /// D. pistol, full, half, force, save or eco; see `buy_type`.
    pub buy_type: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoundFact {
    pub schema_version: u32,
    pub match_id: String,
    pub subject: String,
    pub round: u32,
    pub side: Option<Side>,
    pub won: Option<bool>,
    pub result: Option<String>,
    pub pistol: bool,
    pub overtime: bool,
    pub economy: RoundEconomy,
    /// D. My team's and the enemy's buy types this round.
    pub team_buys: BTreeMap<String, u32>,
    pub enemy_buys: BTreeMap<String, u32>,
    pub kills: u32,
    pub died: bool,
    pub damage_dealt: f64,
    pub damage_taken: f64,
    /// R. Hits dealt on players by region.
    pub hits_head: u32,
    pub hits_body: u32,
    pub hits_legs: u32,
    pub wallbang_hits: u32,
    /// D. Shots from magazine counts; gun hits on players excluding shotguns.
    pub shots: u32,
    pub gun_hits_no_shotgun: u32,
    pub shots_no_shotgun: u32,
    /// D. Shots fired faster than 1.5 m/s.
    pub moving_shots: u32,
    /// R. The round's first cross-team kill was mine / my team's first death
    /// to an enemy was mine (falls and own utility aren't opening duels).
    pub first_kill: bool,
    pub first_death: bool,
    /// R. My team got the round's first kill (man advantage).
    pub team_first_kill: Option<bool>,
    /// R. Seconds after the barrier of the round's first hit, and of mine.
    pub first_contact_s: Option<f64>,
    pub my_first_contact_s: Option<f64>,
    /// D. My rank among living teammates by distance to the nearest enemy at my team's first contact (1 = most forward).
    pub forward_rank: Option<u32>,
    pub team_alive_at_contact: Option<u32>,
    /// R. Last alive on my team against this many enemies, and whether we won.
    pub clutch_vs: Option<u32>,
    pub carried_spike: bool,
    pub planted: bool,
    pub defused: bool,
    pub plant_site: Option<String>,
    /// R. Ability casts by key (C, Q, E, X).
    pub casts: BTreeMap<String, u32>,
    /// R. Casts before the round's first hit.
    pub casts_before_contact: u32,
    pub ult_used: bool,
    /// D. Seconds alive in each callout super region (A, B, Mid, ...) after the barrier.
    pub region_s: BTreeMap<String, f64>,
    /// D. Defense with a plant: seconds from plant until within 15 m of the spike.
    pub rotation_s: Option<f64>,
}

pub const ROUNDS_SCHEMA_VERSION: u32 = 1;

pub fn buy_type(loadout: u32, spent: Option<i64>, credits_after: Option<i64>, pistol: bool) -> &'static str {
    if pistol {
        return "pistol";
    }
    let spent = spent.unwrap_or(0);
    if loadout >= 3900 {
        "full"
    } else if spent <= 400 && loadout < 2000 {
        "save"
    } else if loadout < 2000 {
        "eco"
    } else if credits_after.is_some_and(|c| c < 1000) {
        "force"
    } else {
        "half"
    }
}

/// Facts for every played round from one player's point of view.
pub fn round_facts(a: &Analysis, pawn: u32) -> Vec<RoundFact> {
    let g = a.game;
    let Some(me) = g.player(pawn) else { return Vec::new() };
    let my_side = a.side(pawn);
    let switches: Vec<u32> = g
        .export
        .events
        .iter()
        .filter(|e| e.kind == crate::vrfkit_export::EventKind::SwitchTeams)
        .map(|e| e.time_ms)
        .collect();
    let first_half_end = switches.first().copied();

    (0..a.rounds.len())
        .map(|i| {
            let r = &a.rounds[i];
            // First round of each half; overtime rounds are not pistol rounds.
            let pistol = i == 0 || first_half_end.is_some_and(|s| r.start_ms > s && a.rounds[i - 1].start_ms < s);
            let econ = |p: u32| economy(a, i, p, pistol);
            let mine = econ(pawn);
            let mut team_buys = BTreeMap::new();
            let mut enemy_buys = BTreeMap::new();
            for pl in g.players() {
                if pl.pawn == pawn {
                    continue;
                }
                let bt = econ(pl.pawn).buy_type;
                let m = if a.side(pl.pawn) == my_side {
                    &mut team_buys
                } else {
                    &mut enemy_buys
                };
                *m.entry(bt).or_insert(0) += 1;
            }

            let kills: Vec<&Kill> = g.kills_in_round(Some(i)).collect();
            let hits: Vec<_> = a.hits.iter().filter(|h| h.round_idx == Some(i)).collect();
            let mut f = RoundFact {
                schema_version: ROUNDS_SCHEMA_VERSION,
                match_id: g.export.manifest.match_id.clone(),
                subject: me.subject.clone(),
                round: r.number,
                side: r.side_of(my_side),
                won: r.winner_side.zip(my_side).map(|(w, s)| w == s),
                result: r.result.clone(),
                pistol,
                overtime: r.number >= 24,
                economy: mine,
                team_buys,
                enemy_buys,
                kills: kills.iter().filter(|k| k.killer == pawn && g.is_cross_team(k)).count() as u32,
                died: kills.iter().any(|k| k.victim == pawn),
                ..Default::default()
            };
            for h in hits.iter().filter(|h| h.victim.is_some()) {
                // Hits on teammates (and on myself) aren't damage dealt.
                if h.attacker == Some(pawn) && h.victim.is_some_and(|v| v != pawn && !g.teams.same_team(v, pawn)) {
                    f.damage_dealt += h.damage;
                    match h.region {
                        Some(0) => f.hits_body += 1,
                        Some(1) => f.hits_head += 1,
                        Some(2) => f.hits_legs += 1,
                        _ => {}
                    }
                    f.wallbang_hits += u32::from(h.wallbang);
                    let it = h.item.and_then(|x| a.item(x));
                    if it.is_some_and(|it| it.is_gun() && it.category.as_deref() != Some("shotgun")) {
                        f.gun_hits_no_shotgun += 1;
                    }
                }
                if h.victim == Some(pawn) {
                    f.damage_taken += h.damage;
                }
            }
            f.damage_dealt = round_to(f.damage_dealt, 1);
            f.damage_taken = round_to(f.damage_taken, 1);

            let end = a.rounds.get(i + 1).map(|n| n.start_ms).unwrap_or(u32::MAX);
            let track = g.export.tracks.get(&pawn);
            for s in a
                .shots
                .iter()
                .filter(|s| s.pawn == pawn && s.time_ms >= r.start_ms && s.time_ms < end)
            {
                f.shots += s.count;
                let shotgun = a.item(s.weapon).and_then(|i| i.category.as_deref()) == Some("shotgun");
                if !shotgun {
                    f.shots_no_shotgun += s.count;
                }
                let speed = track
                    .and_then(|tr| tr.at(s.time_ms, 300))
                    .map(|x| (x.vel[0] as f64).hypot(x.vel[1] as f64) / 100.0);
                if speed.is_some_and(|v| v > MOVING_SHOT_M_S) {
                    f.moving_shots += s.count;
                }
            }

            // Self-kills and team kills are nobody's opening duel.
            let cross: Vec<&&Kill> = kills.iter().filter(|k| g.is_cross_team(k)).collect();
            if let Some(k) = cross.first() {
                f.team_first_kill = my_side.map(|s| a.side(k.killer) == Some(s));
                f.first_kill = k.killer == pawn;
            }
            f.first_death = my_side.is_some()
                && cross
                    .iter()
                    .find(|k| a.side(k.victim) == my_side)
                    .is_some_and(|k| k.victim == pawn);

            let barrier = r.buy_end_ms.unwrap_or(r.start_ms);
            let player_hits = || {
                hits.iter()
                    .filter(|h| h.victim.is_some() && h.attacker.is_some() && h.attacker != h.victim)
            };
            let first_contact_ms = player_hits().next().map(|h| h.time_ms);
            f.first_contact_s = first_contact_ms.map(|t| secs(t.saturating_sub(barrier)));
            f.my_first_contact_s = player_hits()
                .find(|h| h.attacker == Some(pawn) || h.victim == Some(pawn))
                .map(|h| secs(h.time_ms.saturating_sub(barrier)));

            // Forward rank at my team's first contact.
            if let Some(h) = player_hits()
                .find(|h| h.attacker.and_then(|p| a.side(p)) == my_side || h.victim.and_then(|p| a.side(p)) == my_side)
            {
                let tc = h.time_ms;
                let dead: Vec<u32> = kills.iter().filter(|k| k.time_ms < tc).map(|k| k.victim).collect();
                let pos = |p: u32| {
                    g.export
                        .tracks
                        .get(&p)
                        .and_then(|tr| tr.at(tc, MAX_SAMPLE_AGE_MS))
                        .map(|s| s.pos)
                };
                let enemies: Vec<[f32; 3]> = g
                    .players()
                    .iter()
                    .filter(|p| a.side(p.pawn) != my_side && !dead.contains(&p.pawn))
                    .filter_map(|p| pos(p.pawn))
                    .collect();
                let nearest_enemy = |p: [f32; 3]| {
                    enemies
                        .iter()
                        .map(|e| (e[0] as f64 - p[0] as f64).hypot(e[1] as f64 - p[1] as f64))
                        .fold(f64::INFINITY, f64::min)
                };
                let mut team: Vec<(u32, f64)> = g
                    .team_of(pawn)
                    .into_iter()
                    .filter(|p| !dead.contains(p))
                    .filter_map(|p| pos(p).map(|x| (p, nearest_enemy(x))))
                    .collect();
                if !enemies.is_empty() && team.iter().any(|(p, _)| *p == pawn) {
                    team.sort_by(|x, y| x.1.total_cmp(&y.1));
                    f.forward_rank = team.iter().position(|(p, _)| *p == pawn).map(|i| i as u32 + 1);
                    f.team_alive_at_contact = Some(team.len() as u32);
                }
            }

            // Clutch: the moment I become the last one alive on my team.
            let mut alive_team: Vec<u32> = g.team_of(pawn);
            let mut alive_enemy: Vec<u32> = g
                .players()
                .iter()
                .map(|p| p.pawn)
                .filter(|&p| a.side(p).is_some() && a.side(p) != my_side)
                .collect();
            // A player of unknown team could still be alive on either side: no clutch claimed.
            let teams_known = g.teams_complete();
            for k in &kills {
                alive_team.retain(|&p| p != k.victim);
                alive_enemy.retain(|&p| p != k.victim);
                if teams_known && f.clutch_vs.is_none() && alive_team == [pawn] && !alive_enemy.is_empty() {
                    f.clutch_vs = Some(alive_enemy.len() as u32);
                }
            }

            f.carried_spike = r.spike_holders.iter().any(|h| h.1 == pawn);
            if let Some(p) = &r.plant {
                f.planted = p.planter == Some(pawn);
                f.plant_site = p.site.clone();
            }
            f.defused = r.defuser == Some(pawn);

            for c in a.casts.iter().filter(|c| c.pawn == pawn && c.round_idx == Some(i)) {
                if let Some(name) = slot_name(c.slot) {
                    *f.casts.entry(slot_key(name).to_owned()).or_insert(0) += 1;
                }
                if c.slot == 9 {
                    f.ult_used = true;
                }
                if c.slot != 9 && c.time_ms.zip(first_contact_ms).is_some_and(|(ct, fc)| ct < fc) {
                    f.casts_before_contact += 1;
                }
            }

            // Region time and rotation (D: callout naming).
            let death = kills.iter().find(|k| k.victim == pawn).map(|k| k.time_ms);
            let stop = death.unwrap_or(r.end_ms).min(r.end_ms);
            if let (Some(map), Some(tr)) = (a.map, track) {
                let mut t = barrier;
                while t < stop {
                    if let Some(s) = tr.at(t, MAX_SAMPLE_AGE_MS) {
                        let p = [s.pos[0] as f64, s.pos[1] as f64, s.pos[2] as f64];
                        if let Some((c, _)) = map.nearest_callout(p) {
                            *f.region_s.entry(c.super_region.clone()).or_insert(0.0) += 0.5;
                        }
                    }
                    t += 500;
                }
                if f.side == Some(Side::Defense)
                    && let Some(p) = r.plant.as_ref().and_then(|p| p.pos_cm.map(|pos| (p.time_ms, pos)))
                {
                    let mut t = p.0;
                    while t < stop {
                        if let Some(s) = tr.at(t, MAX_SAMPLE_AGE_MS)
                            && (s.pos[0] as f64 - p.1[0]).hypot(s.pos[1] as f64 - p.1[1]) <= 1500.0
                        {
                            f.rotation_s = Some(secs(t - p.0));
                            break;
                        }
                        t += 250;
                    }
                }
            }
            f
        })
        .collect()
}

fn economy(a: &Analysis, i: usize, pawn: u32, pistol: bool) -> RoundEconomy {
    let r = &a.rounds[i];
    let barrier = r.buy_end_ms.unwrap_or(r.start_ms);
    let credits_start = a.start_money_in(pawn, r.start_ms, barrier + 1);
    let credits_after_buy = a.money_at(pawn, barrier);
    let spent = credits_start.zip(credits_after_buy).map(|(s, e)| (s - e).max(0));
    let owned = a.owned_at(pawn, barrier + 2000);
    let items: Vec<_> = owned.iter().filter_map(|&g| a.item(g)).collect();
    let loadout_value = items.iter().filter_map(|i| i.cost).sum();
    let primary = items
        .iter()
        .filter(|i| i.is_gun())
        .max_by_key(|i| i.cost.unwrap_or(0))
        .map(|i| i.name.clone());
    let armor = items
        .iter()
        .filter(|i| i.kind == ItemKind::Armor)
        .max_by_key(|i| i.cost.unwrap_or(0))
        .map(|i| i.name.clone());
    RoundEconomy {
        credits_start,
        credits_after_buy,
        spent,
        loadout_value,
        primary,
        armor,
        buy_type: buy_type(loadout_value, spent, credits_after_buy, pistol).to_owned(),
    }
}

fn secs(ms: u32) -> f64 {
    round_to(f64::from(ms) / 1000.0, 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::fields::{AbilityCast, Change, DamageHit};
    use crate::game::Game;
    use crate::game::sample::{export, kill, round};
    use crate::vrfkit_export::{EventKind, Export};

    /// Teams 1-5 and 6-10 from round 0, then `events` in round 1 (barrier at 100.5 s).
    fn match_with(players: u32, events: Vec<(u32, EventKind)>) -> Export {
        let mut ev = vec![round(0, 0)];
        let mut t = 1000;
        for i in 1..=5 {
            for (k, v) in [(i, 5 + i), (i, 5 + i % 5 + 1), (5 + i, i)] {
                ev.push(kill(t, k, v));
                t += 1000;
            }
        }
        ev.push(round(100_000, 1));
        ev.extend(events);
        let mut ex = export(players, ev);
        ex.fields.phases.push(Change {
            time_ms: 100_500,
            actor: 0,
            value: 4,
        });
        ex
    }

    fn hit(time_ms: u32, attacker: u32, victim: u32, damage: f64, region: i64) -> DamageHit {
        DamageHit {
            time_ms,
            victim,
            damager_state: Some(100 + attacker),
            damage,
            region: Some(region),
            ..DamageHit::default()
        }
    }

    #[test]
    fn self_kills_and_team_kills_are_not_opening_duels_or_kills() {
        // Round 1: 1 falls to their death, 6 kills 2, 3 kills teammate 4.
        let mut ex = match_with(10, vec![kill(101_000, 1, 1), kill(102_000, 6, 2), kill(104_000, 3, 4)]);
        // 3's first hit is the round's first contact, 1.049 s after the barrier
        // (1.0 s once rounded); 3's Q went out at 1.02 s, before it.
        ex.fields.damage = vec![hit(101_549, 3, 7, 30.0, 0), hit(104_000, 3, 4, 50.0, 1)];
        ex.fields.casts.push(AbilityCast {
            player: "p3".into(),
            slot: 4,
            round: 1,
            cast_time_s: 1.02,
            location_cm: None,
            seen_ms: 101_520,
        });
        let g = Game::new(ex);
        let catalog = Catalog::builtin();
        let a = Analysis::new(&g, &catalog);
        let r = |p: u32| round_facts(&a, p).remove(1);

        assert!(!r(1).first_death, "a fall isn't a lost opening duel");
        assert!(r(2).first_death);
        assert_eq!(r(2).team_first_kill, Some(false));
        assert!(r(6).first_kill);
        let three = r(3);
        assert_eq!(three.kills, 0, "a team kill isn't a kill");
        assert_eq!((three.damage_dealt, three.hits_head, three.hits_body), (30.0, 0, 1));
        assert_eq!(three.first_contact_s, Some(1.0));
        assert_eq!(three.casts_before_contact, 1);
    }

    #[test]
    fn no_clutch_with_a_player_of_unknown_team() {
        // Round 1: 2 to 5 die, leaving 1 against all five.
        let deaths: Vec<_> = (2..=5).map(|v| kill(100_000 + v * 1000, 6, v)).collect();
        let g = Game::new(match_with(10, deaths.clone()));
        let catalog = Catalog::builtin();
        assert_eq!(round_facts(&Analysis::new(&g, &catalog), 1)[1].clutch_vs, Some(5));
        // An 11th player who never fights and has no positions could be alive on either side.
        let g = Game::new(match_with(11, deaths));
        assert_eq!(round_facts(&Analysis::new(&g, &catalog), 1)[1].clutch_vs, None);
    }

    #[test]
    fn buy_types_from_the_loadout_and_spend() {
        assert_eq!(buy_type(4700, Some(4700), Some(300), true), "pistol");
        assert_eq!(buy_type(3900, Some(2900), Some(200), false), "full");
        assert_eq!(buy_type(1200, Some(400), Some(5000), false), "save");
        assert_eq!(buy_type(1200, None, None, false), "save");
        assert_eq!(buy_type(1900, Some(1500), Some(2000), false), "eco");
        assert_eq!(buy_type(3000, Some(2800), Some(900), false), "force");
        assert_eq!(buy_type(3000, Some(2800), Some(1000), false), "half");
        assert_eq!(buy_type(3000, Some(2800), None, false), "half");
    }

    #[test]
    fn range_bands_and_hit_regions() {
        assert_eq!(range_band(0.0), "0-10 m");
        assert_eq!(range_band(9.99), "0-10 m");
        assert_eq!(range_band(10.0), "10-20 m");
        assert_eq!(range_band(29.9), "20-30 m");
        assert_eq!(range_band(30.0), "30+ m");
        assert_eq!(range_band(f64::NAN), "30+ m");
        assert_eq!(region_name(Some(1)), "head");
        assert_eq!(region_name(Some(0)), "body");
        assert_eq!(region_name(Some(2)), "legs");
        assert_eq!(region_name(Some(7)), "other");
        assert_eq!(region_name(None), "other");
    }
}
