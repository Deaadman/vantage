//! Joins the raw replay rows into per-match timelines that the fact groups
//! (B to F) read: rounds, hits, inventory, ammo, shots, casts, ultimates,
//! money. Built once per match.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::catalog::{Catalog, MapInfo};
use crate::export::EventKind;
use crate::fields::Change;
use crate::game::Game;
use crate::items::{Item, ItemKind, identify};

/// Ability slot numbers in `AbilityCastsThisRound[].Slot`, verified on 13.08
/// by each slot's effect statistics against valorant-api.com's slot names
/// (Phoenix 4 = HealingDone = Hot Hands = Ability1; Neon 5 = TimeSprinting =
/// High Gear = Ability2; Chamber 4 = ShotsFired = Headhunter = Ability1).
pub fn slot_name(slot: i64) -> Option<&'static str> {
    Some(match slot {
        3 => "Grenade",
        4 => "Ability1",
        5 => "Ability2",
        9 => "Ultimate",
        _ => return None,
    })
}

/// Key for a slot as players know it.
pub fn slot_key(slot: &str) -> &'static str {
    match slot {
        "Grenade" => "C",
        "Ability1" => "Q",
        "Ability2" => "E",
        "Ultimate" => "X",
        _ => "?",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Attack,
    Defense,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plant {
    pub time_ms: u32,
    pub pos_cm: Option<[f64; 3]>,
    /// `A`, `B`, `C`: the nearest callout's super region (needs the catalog).
    pub site: Option<String>,
    pub planter: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct RoundInfo {
    pub number: u32,
    /// `roundStarted`: start of the buy phase.
    pub start_ms: u32,
    /// Barrier drop (`MulticastSetPhase` 4); `CastTime` is measured from it.
    pub buy_end_ms: Option<u32>,
    /// Round decided (`MulticastSetPhase` 5), else the next round's start.
    pub end_ms: u32,
    /// `elimination`, `defuse`, ... (`RoundResults`; detonations under-count there, see `exploded_ms`).
    pub result: Option<String>,
    /// Kill-graph side label of the attacking team.
    pub attacker_side: Option<u8>,
    pub winner_side: Option<u8>,
    pub plant: Option<Plant>,
    pub defuse_ms: Option<u32>,
    pub defuser: Option<u32>,
    pub exploded_ms: Option<u32>,
    /// Players who held the spike this round, in order.
    pub spike_holders: Vec<(u32, u32)>,
}

impl RoundInfo {
    pub fn side_of(&self, team_side: Option<u8>) -> Option<Side> {
        match (self.attacker_side, team_side) {
            (Some(a), Some(s)) => Some(if a == s { Side::Attack } else { Side::Defense }),
            _ => None,
        }
    }
}

/// A damage invocation resolved to players and items.
#[derive(Debug, Clone)]
pub struct Hit {
    pub time_ms: u32,
    pub round_idx: Option<usize>,
    /// Player pawn, if a player was hit.
    pub victim: Option<u32>,
    /// Player pawn of the damager.
    pub attacker: Option<u32>,
    pub item: Option<u32>,
    /// Full damage (CombatReport's sum); `damage_applied` is what it removed.
    pub damage: f64,
    pub damage_applied: f64,
    /// 0 body, 1 head, 2 legs, 5 not a body hit.
    pub region: Option<i64>,
    pub wallbang: bool,
    pub killed: bool,
}

/// Rounds fired, from a weapon's magazine counting down.
#[derive(Debug, Clone, Copy)]
pub struct Shot {
    pub time_ms: u32,
    pub pawn: u32,
    pub weapon: u32,
    /// Usually 1; more when replication batched several shots.
    pub count: u32,
}

#[derive(Debug, Clone)]
pub struct Cast {
    pub pawn: u32,
    pub round_idx: Option<usize>,
    pub slot: i64,
    /// `buy_end_ms + CastTime`, when the round's barrier drop is known.
    pub time_ms: Option<u32>,
    pub cast_time_s: f64,
    pub location_cm: Option<[f64; 3]>,
}

pub struct Analysis<'a> {
    pub game: &'a Game,
    pub catalog: &'a Catalog,
    pub map: Option<&'a MapInfo>,
    pub items: HashMap<u32, Item>,
    pub state_pawn: HashMap<u32, u32>,
    pub rounds: Vec<RoundInfo>,
    pub hits: Vec<Hit>,
    pub shots: Vec<Shot>,
    pub casts: Vec<Cast>,
    /// Ultimate activations (slot 9 casts) per pawn, as times when known.
    pub ult_uses: Vec<(u32, Option<usize>, Option<u32>)>,
    /// Max `NumUltimatePoints` seen per pawn: the ult cost if they ever had it ready.
    pub ult_cost: HashMap<u32, i64>,
    ult_points: HashMap<u32, Vec<(u32, i64)>>,
    owners: HashMap<u32, Vec<(u32, u32)>>,
    in_hand: HashMap<u32, Vec<(u32, u32)>>,
    magazine: HashMap<u32, Vec<(u32, i64)>>,
    money: HashMap<u32, Vec<(u32, i64)>>,
    start_money: HashMap<u32, Vec<(u32, i64)>>,
}

impl<'a> Analysis<'a> {
    pub fn new(game: &'a Game, catalog: &'a Catalog) -> Self {
        let ex = &game.export;
        let f = &ex.fields;
        let map = ex.manifest.map_path.as_deref().and_then(|p| catalog.map_by_url(p));

        let items: HashMap<u32, Item> = ex
            .actors
            .class
            .iter()
            .filter(|(_, c)| c.contains("/Equippables/") || c.contains("/Gear/") || c.contains("/Ability_"))
            .map(|(&g, c)| (g, identify(c, catalog)))
            .collect();

        let canonical: HashMap<u32, u32> = game
            .players()
            .iter()
            .flat_map(|p| p.pawns.iter().map(move |&g| (g, p.pawn)))
            .collect();
        let pawn_of = |g: u32| canonical.get(&g).copied();
        let state_pawn: HashMap<u32, u32> = game.players().iter().map(|p| (p.actor_net_guid, p.pawn)).collect();
        let subject_pawn: HashMap<&str, u32> = game.players().iter().map(|p| (p.subject.as_str(), p.pawn)).collect();

        let mut owners: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
        for c in &f.owners {
            if items.contains_key(&c.actor) {
                owners
                    .entry(c.actor)
                    .or_default()
                    .push((c.time_ms, pawn_of(c.value).unwrap_or(0)));
            }
        }
        let mut in_hand: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
        for c in &f.in_hand {
            if let Some(p) = pawn_of(c.actor) {
                in_hand.entry(p).or_default().push((c.time_ms, c.value));
            }
        }
        let by_state = |rows: &[Change<i64>]| {
            let mut m: HashMap<u32, Vec<(u32, i64)>> = HashMap::new();
            for c in rows {
                if let Some(&p) = state_pawn.get(&c.actor) {
                    m.entry(p).or_default().push((c.time_ms, c.value));
                }
            }
            m
        };
        let money = by_state(&f.money);
        let start_money = by_state(&f.start_money);
        let ult_points = by_state(&f.ult_points);
        let ult_cost = ult_points
            .iter()
            .map(|(&p, v)| (p, v.iter().map(|x| x.1).max().unwrap_or(0)))
            .collect();

        let mut a = Analysis {
            game,
            catalog,
            map,
            items,
            state_pawn,
            rounds: Vec::new(),
            hits: Vec::new(),
            shots: Vec::new(),
            casts: Vec::new(),
            ult_uses: Vec::new(),
            ult_cost,
            ult_points,
            owners,
            in_hand,
            magazine: HashMap::new(),
            money,
            start_money,
        };
        a.rounds = a.build_rounds();
        a.hits = f
            .damage
            .iter()
            .map(|d| Hit {
                time_ms: d.time_ms,
                round_idx: a.round_idx_at(d.time_ms),
                victim: pawn_of(d.victim),
                attacker: d.damager_state.and_then(|s| a.state_pawn.get(&s).copied()),
                item: d.equippable,
                damage: d.damage,
                damage_applied: d.damage_applied,
                region: d.region,
                wallbang: d.wallbang,
                killed: d.killed,
            })
            .collect();
        a.build_ammo();
        a.casts = f
            .casts
            .iter()
            .filter_map(|c| {
                let pawn = *subject_pawn.get(c.player.to_ascii_lowercase().as_str())?;
                let round_idx = game.rounds.iter().position(|r| i64::from(r.number) == c.round);
                let time_ms = round_idx
                    .and_then(|i| a.rounds[i].buy_end_ms)
                    .map(|b| b.saturating_add((c.cast_time_s.max(0.0) * 1000.0).round() as u32));
                Some(Cast {
                    pawn,
                    round_idx,
                    slot: c.slot,
                    time_ms,
                    cast_time_s: c.cast_time_s,
                    location_cm: c.location_cm,
                })
            })
            .collect();
        a.ult_uses = a
            .casts
            .iter()
            .filter(|c| c.slot == 9)
            .map(|c| (c.pawn, c.round_idx, c.time_ms))
            .collect();
        a
    }

    pub fn round_idx_at(&self, t: u32) -> Option<usize> {
        self.game.rounds.iter().rposition(|r| r.start_ms <= t)
    }

    pub fn item(&self, guid: u32) -> Option<&Item> {
        self.items.get(&guid)
    }

    pub fn item_name(&self, guid: Option<u32>) -> Option<String> {
        guid.and_then(|g| self.item(g)).map(|i| i.name.clone())
    }

    /// Item name for output. Ability items are named from the holder's agent:
    /// ultimates live under `Ability_X` in every agent's folder, so those get
    /// the catalog's Ultimate name; other ability folders' letters do not match
    /// the keys (Sova's Recon Bolt sits under `Ability_Q`), so they are just
    /// "<Agent> ability".
    pub fn item_label(&self, guid: Option<u32>, holder: u32) -> Option<String> {
        let g = guid?;
        let item = self.item(g)?;
        if item.kind != ItemKind::Ability {
            return Some(item.name.clone());
        }
        let agent_id = self.game.agent_id(holder);
        let agent = self.catalog.agent_name(agent_id);
        let class = self.game.export.actors.class.get(&g).map(String::as_str).unwrap_or("");
        if class.contains("/Ability_X/") {
            if let Some(name) = agent_id.and_then(|id| self.catalog.ability_name(id, "Ultimate")) {
                return Some(format!("{name} (ult)"));
            }
            return Some(format!("{agent} ult"));
        }
        Some(format!("{agent} ability"))
    }

    /// The item a pawn had in hand just before `t` (the inventory clears to 0 at death).
    pub fn in_hand_before(&self, pawn: u32, t: u32, since: u32) -> Option<u32> {
        let v = self.in_hand.get(&pawn)?;
        v.iter()
            .rev()
            .filter(|(time, _)| *time < t && *time >= since)
            .find(|(_, g)| *g != 0)
            .map(|(_, g)| *g)
    }

    /// Items (guns, armour, spike) a pawn owned at `t`.
    pub fn owned_at(&self, pawn: u32, t: u32) -> Vec<u32> {
        let mut out: Vec<u32> = self
            .owners
            .keys()
            .copied()
            .filter(|&g| self.owner_at(g, t) == Some(pawn))
            .collect();
        out.sort_unstable();
        out
    }

    /// Owner of an item at `t`. An item whose actor channel closed by then no
    /// longer exists, whatever its last `Owner` write said.
    pub fn owner_at(&self, item: u32, t: u32) -> Option<u32> {
        if self.game.export.actors.closed_at.get(&item).is_some_and(|&c| c <= t) {
            return None;
        }
        self.owners.get(&item).and_then(|v| last_at(v, t)).filter(|&p| p != 0)
    }

    /// Rounds left in the magazine of `weapon` at `t`.
    pub fn magazine_at(&self, weapon: u32, t: u32) -> Option<i64> {
        self.magazine.get(&weapon).and_then(|v| last_at(v, t))
    }

    pub fn money_at(&self, pawn: u32, t: u32) -> Option<i64> {
        self.money.get(&pawn).and_then(|v| last_at(v, t))
    }

    /// `StartOfRoundMoney` written during the round that contains `t`.
    pub fn start_money_in(&self, pawn: u32, from: u32, to: u32) -> Option<i64> {
        self.start_money
            .get(&pawn)?
            .iter()
            .rev()
            .find(|(t, _)| *t >= from && *t < to)
            .map(|x| x.1)
    }

    pub fn ult_points_at(&self, pawn: u32, t: u32) -> Option<i64> {
        self.ult_points.get(&pawn).and_then(|v| last_at(v, t))
    }

    pub fn side(&self, pawn: u32) -> Option<u8> {
        self.game.teams.side(pawn)
    }

    // ------------------------------------------------------------ rounds

    fn build_rounds(&self) -> Vec<RoundInfo> {
        let g = self.game;
        let f = &g.export.fields;
        let ex = &g.export;
        let n = g.rounds.len();
        let next_start = |i: usize| {
            g.rounds
                .get(i + 1)
                .map(|r| r.start_ms)
                .unwrap_or(ex.manifest.duration_ms as u32)
        };
        let spike_items: Vec<u32> = self
            .items
            .iter()
            .filter(|(_, it)| it.kind == ItemKind::Spike)
            .map(|(&g, _)| g)
            .collect();

        let mut rounds: Vec<RoundInfo> = (0..n)
            .map(|i| {
                let r = g.rounds[i];
                let end = next_start(i);
                let within = |t: u32| t >= r.start_ms && t < end;
                let phase = |p: i64| {
                    f.phases
                        .iter()
                        .find(|c| c.value == p && within(c.time_ms))
                        .map(|c| c.time_ms)
                };
                let mut holders: Vec<(u32, u32)> = Vec::new();
                for &s in &spike_items {
                    for &(t, owner) in self.owners.get(&s).map(Vec::as_slice).unwrap_or_default() {
                        if within(t) && owner != 0 && holders.last().map(|h| h.1) != Some(owner) {
                            holders.push((t, owner));
                        }
                    }
                }
                holders.sort_by_key(|h| h.0);
                let ev = |kind: EventKind| {
                    ex.events
                        .iter()
                        .find(|e| e.kind == kind && within(e.time_ms))
                        .map(|e| e.time_ms)
                };
                let plant = ev(EventKind::SpikePlanted).map(|t| {
                    let pos = ex
                        .actors
                        .spawns
                        .iter()
                        .filter(|(st, _, c, _)| c.contains("TimedBomb") && st.abs_diff(t) <= 2000)
                        .min_by_key(|(st, ..)| st.abs_diff(t))
                        .map(|s| s.3);
                    let site = pos
                        .and_then(|p| self.map.and_then(|m| m.nearest_callout(p)))
                        .map(|(c, _)| c.super_region.clone())
                        .filter(|s| !s.is_empty());
                    let planter = spike_items.iter().find_map(|&s| self.owner_at(s, t.saturating_sub(1)));
                    Plant {
                        time_ms: t,
                        pos_cm: pos,
                        site,
                        planter,
                    }
                });
                let defuse_ms = ev(EventKind::SpikeDefused);
                let defuser = defuse_ms
                    .and_then(|t| {
                        f.defused_by
                            .iter()
                            .filter(|c| c.time_ms.abs_diff(t) <= 2000)
                            .map(|c| c.value)
                            .next()
                    })
                    .and_then(|p| g.player(p).map(|pl| pl.pawn));
                let result = f.round_results.iter().find(|x| x.number == Some(i64::from(r.number)));
                RoundInfo {
                    number: r.number,
                    start_ms: r.start_ms,
                    buy_end_ms: phase(4),
                    end_ms: phase(5).unwrap_or(end),
                    result: result.and_then(|x| x.result.clone()),
                    attacker_side: holders.first().and_then(|(_, p)| g.teams.side(*p)),
                    winner_side: None,
                    plant,
                    defuse_ms,
                    defuser,
                    exploded_ms: ev(EventKind::SpikeExploded),
                    spike_holders: holders,
                }
            })
            .collect();

        // Rounds where nobody touched the spike: carry the side over, flipping at switchTeams.
        let switches: Vec<u32> = ex
            .events
            .iter()
            .filter(|e| e.kind == EventKind::SwitchTeams)
            .map(|e| e.time_ms)
            .collect();
        let flipped_between =
            |a: u32, b: u32| switches.iter().filter(|&&t| t > a.min(b) && t <= a.max(b)).count() % 2 == 1;
        for i in 0..n {
            if rounds[i].attacker_side.is_some() {
                continue;
            }
            let known = (0..n)
                .filter(|&j| rounds[j].attacker_side.is_some())
                .min_by_key(|&j| j.abs_diff(i));
            if let Some(j) = known {
                let s = rounds[j].attacker_side.expect("known");
                rounds[i].attacker_side = Some(if flipped_between(rounds[j].start_ms, rounds[i].start_ms) {
                    1 - s
                } else {
                    s
                });
            }
        }
        for r in &mut rounds {
            let role = f
                .round_results
                .iter()
                .find(|x| x.number == Some(i64::from(r.number)))
                .and_then(|x| x.winning_role.clone());
            r.winner_side = match (role.as_deref(), r.attacker_side) {
                (Some("attacker"), Some(a)) => Some(a),
                (Some("defender"), Some(a)) => Some(1 - a),
                _ => None,
            };
        }
        rounds
    }

    // ------------------------------------------------------------ ammo and shots

    fn build_ammo(&mut self) {
        let f = &self.game.export.fields;
        // Each weapon has two AmmoComponents: the magazine and the reserve. The
        // magazine is the one with the smaller maximum (Classic 12 vs 36).
        let mut comps: HashMap<u32, HashMap<u32, i64>> = HashMap::new();
        for a in &f.ammo {
            let m = comps
                .entry(a.weapon)
                .or_default()
                .entry(a.component)
                .or_insert(a.amount);
            *m = (*m).max(a.amount);
        }
        let mag_comp: HashMap<u32, u32> = comps
            .iter()
            .filter(|(_, c)| c.len() >= 2)
            .filter_map(|(&w, c)| c.iter().min_by_key(|(_, max)| **max).map(|(&comp, _)| (w, comp)))
            .collect();
        let mut magazine: HashMap<u32, Vec<(u32, i64)>> = HashMap::new();
        for a in &f.ammo {
            if mag_comp.get(&a.weapon) == Some(&a.component) {
                magazine.entry(a.weapon).or_default().push((a.time_ms, a.amount));
            }
        }
        let mut shots = Vec::new();
        for (&weapon, v) in &magazine {
            if !self.items.get(&weapon).is_some_and(Item::is_gun) {
                continue;
            }
            for w in v.windows(2) {
                let (t, now) = w[1];
                let drop = w[0].1 - now;
                // A drop of a whole magazine or more is a weapon reset, not fire.
                if drop > 0
                    && drop < 10
                    && let Some(pawn) = self.owner_at(weapon, t)
                {
                    shots.push(Shot {
                        time_ms: t,
                        pawn,
                        weapon,
                        count: drop as u32,
                    });
                }
            }
        }
        shots.sort_by_key(|s| s.time_ms);
        self.magazine = magazine;
        self.shots = shots;
    }
}

/// Last value at or before `t` in a time-sorted list.
fn last_at<T: Copy>(v: &[(u32, T)], t: u32) -> Option<T> {
    let i = v.partition_point(|(time, _)| *time <= t);
    i.checked_sub(1).map(|i| v[i].1)
}
