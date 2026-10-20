//! Packet sections for groups B to F, written from the reviewed player's
//! death facts and round facts.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::analysis::{Side, slot_key, slot_name};
use crate::facts::RoundFact;
use crate::packet::{Duel, MatchReview};

/// Caveats for the derived (D) facts in these sections, stated once in the packet.
pub const PHASE4_CAVEATS: &[&str] = &[
    "Shots are counted from magazine decrements; replication can batch shots, and shotguns are left out of accuracy because one shot hits many pellets.",
    "A shot is \"moving\" above 1.5 m/s horizontal speed at the time of the shot.",
    "Duel records count only kills between me and an enemy (a fight with no kill is not a duel); the weapon is the one I had in hand.",
    "Vertical aim offset is crosshair pitch minus the elevation to the killer from pawn positions; it assumes UE pitch (up positive) and equal eye heights.",
    "Ult ready means holding the most ult points this player held all match (the cost is not in valorant-api.com).",
    "Ult uses are counted from ultimate casts; the bUltimateActive flag also toggles when Chamber re-equips his ult gun.",
    "Buy types (per player): full = loadout >= 3900; save = spent <= 400 and loadout < 2000; eco = loadout < 2000; force = rest with < 1000 credits left; half = rest. Loadout counts guns and armour owned 2 s after the barrier, not abilities.",
    "Round clock at death assumes the standard 100 s round and 45 s spike timers.",
    "Damage is each hit's full damage as the game's CombatReport sums it, so overkill counts.",
    "Area time, rotations and callouts use the nearest valorant-api.com callout point, not region outlines.",
    "Forward rank is my place among living teammates by distance to the nearest enemy at my team's first contact (1 = closest to an enemy).",
    "Abilities left unused at death are not reported: charge state is not decoded for 13.08.",
];

pub fn render(r: &MatchReview, o: &mut String) {
    gunfights(r, o);
    positioning(r, o);
    utility(r, o);
    economy(r, o);
    rounds(r, o);
}

fn pct(n: usize, d: usize) -> String {
    if d == 0 {
        "n/a".into()
    } else {
        format!("{:.0}%", 100.0 * n as f64 / d as f64)
    }
}

fn wl(rs: &[&RoundFact]) -> String {
    let w = rs.iter().filter(|f| f.won == Some(true)).count();
    let l = rs.iter().filter(|f| f.won == Some(false)).count();
    format!("{w}-{l}")
}

fn counts<I: IntoIterator<Item = String>>(it: I) -> String {
    let mut m: BTreeMap<String, usize> = BTreeMap::new();
    for s in it {
        *m.entry(s).or_insert(0) += 1;
    }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    if v.is_empty() {
        return "none".into();
    }
    v.iter()
        .map(|(k, n)| format!("{k} ×{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

fn record(duels: &[Duel], key: impl Fn(&Duel) -> Option<String>) -> String {
    let mut m: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for d in duels {
        if let Some(k) = key(d) {
            let e = m.entry(k).or_insert((0, 0));
            if d.won { e.0 += 1 } else { e.1 += 1 }
        }
    }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by_key(|(_, (w, l))| std::cmp::Reverse(w + l));
    v.iter()
        .map(|(k, (w, l))| format!("{k} {w}-{l}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn gunfights(r: &MatchReview, o: &mut String) {
    let rf = &r.round_facts;
    let d = &r.death_facts;
    let head: u32 = rf.iter().map(|f| f.hits_head).sum();
    let body: u32 = rf.iter().map(|f| f.hits_body).sum();
    let legs: u32 = rf.iter().map(|f| f.hits_legs).sum();
    let hits = (head + body + legs) as usize;
    let wall: u32 = rf.iter().map(|f| f.wallbang_hits).sum();
    let shots: u32 = rf.iter().map(|f| f.shots_no_shotgun).sum();
    let gun_hits: u32 = rf.iter().map(|f| f.gun_hits_no_shotgun).sum();
    let all_shots: u32 = rf.iter().map(|f| f.shots).sum();
    let moving: u32 = rf.iter().map(|f| f.moving_shots).sum();
    let dmg: f64 = rf.iter().map(|f| f.damage_dealt).sum();
    let taken: f64 = rf.iter().map(|f| f.damage_taken).sum();

    let _ = writeln!(o, "## Gunfights");
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "- Damage dealt {:.0} ({:.0} per round), taken {:.0}. Hits on players: {hits} (head {}, body {}, legs {}); wallbang hits {wall}",
        dmg,
        dmg / rf.len().max(1) as f64,
        taken,
        pct(head as usize, hits),
        pct(body as usize, hits),
        pct(legs as usize, hits)
    );
    let _ = writeln!(
        o,
        "- Accuracy (no shotguns): {gun_hits} hits from {shots} shots ({}). Moving shots: {moving} of {all_shots} ({})",
        pct(gun_hits as usize, shots as usize),
        pct(moving as usize, all_shots as usize)
    );
    let exchanged: Vec<_> = d.iter().filter(|f| f.context.hit_killer_first.is_some()).collect();
    let first = exchanged
        .iter()
        .filter(|f| f.context.hit_killer_first == Some(true))
        .count();
    let no_hit = d
        .iter()
        .filter(|f| !f.self_kill && f.context.duel_hits_dealt == 0)
        .count();
    let _ = writeln!(
        o,
        "- Fatal duels: I hit the killer first in {first} of {}; I never hit my killer in {no_hit} of {} deaths. Median from first hit to death: {} ms; median damage I dealt the killer: {}",
        exchanged.len(),
        d.len(),
        median(
            exchanged
                .iter()
                .filter_map(|f| f.context.duel_ms.map(f64::from))
                .collect()
        )
        .map(|m| format!("{m:.0}"))
        .unwrap_or("n/a".into()),
        median(d.iter().map(|f| f.context.duel_damage_dealt).collect())
            .map(|m| format!("{m:.0}"))
            .unwrap_or("n/a".into())
    );
    let _ = writeln!(
        o,
        "- Weapon in hand at death: {}",
        counts(d.iter().filter_map(|f| f.context.weapon_in_hand.clone()))
    );
    let empty = d.iter().filter(|f| f.context.magazine_ammo == Some(0)).count();
    let low = d
        .iter()
        .filter(|f| {
            f.context
                .magazine_ammo
                .zip(f.context.magazine_size)
                .is_some_and(|(a, s)| a > 0 && (a as f64) <= s as f64 * 0.25)
        })
        .count();
    let _ = writeln!(
        o,
        "- At death: knife out {}, magazine empty {empty}, magazine at 25% or less {low}",
        d.iter().filter(|f| f.context.knife_out).count()
    );
    let _ = writeln!(
        o,
        "- Killed with: {}. Killing blow region: {}",
        counts(d.iter().filter_map(|f| f.context.killer_weapon.clone())),
        counts(d.iter().filter_map(|f| f.context.killing_blow_region.clone()))
    );
    let v: Vec<f64> = d.iter().filter_map(|f| f.context.vertical_aim_offset_deg).collect();
    let off = v.iter().filter(|x| x.abs() > 2.0).count();
    let _ = writeln!(
        o,
        "- Vertical aim at death: median offset {}°, more than 2° off in {off} of {}",
        median(v.clone()).map(|m| format!("{m:+.1}")).unwrap_or("n/a".into()),
        v.len()
    );
    let ds = &r.duels;
    let _ = writeln!(
        o,
        "- Duel record (kills-deaths) by my weapon: {}",
        record(ds, |d| Some(d.weapon.clone()))
    );
    let _ = writeln!(o, "- By range: {}", record(ds, |d| d.range.clone()));
    let _ = writeln!(o, "- By enemy agent: {}", record(ds, |d| Some(d.enemy_agent.clone())));
    let _ = writeln!(o);
}

fn positioning(r: &MatchReview, o: &mut String) {
    let rf = &r.round_facts;
    let _ = writeln!(o, "## Positioning");
    let _ = writeln!(o);

    // Trade rates: my deaths vs my teammates' deaths, both with a teammate alive.
    let ((mt, mn), (tt, tn)) = (r.my_trades, r.team_trades);
    let _ = writeln!(
        o,
        "- Traded within 3 s when a teammate was alive: me {mt}/{mn} ({}), my teammates {tt}/{tn} ({})",
        pct(mt, mn),
        pct(tt, tn)
    );
    let ranks: Vec<&RoundFact> = rf.iter().filter(|f| f.forward_rank.is_some()).collect();
    let _ = writeln!(
        o,
        "- Forward rank at my team's first contact: most forward in {} of {} rounds, median rank {}",
        ranks.iter().filter(|f| f.forward_rank == Some(1)).count(),
        ranks.len(),
        median(ranks.iter().filter_map(|f| f.forward_rank.map(f64::from)).collect())
            .map(|m| format!("{m}"))
            .unwrap_or("n/a".into())
    );
    let same = r
        .death_facts
        .iter()
        .filter(|f| f.context.same_spot_as_own_kill)
        .map(|f| f.round.unwrap_or(0))
        .collect::<Vec<_>>();
    let _ = writeln!(
        o,
        "- Died within 5 m of my own earlier kill that round: {} (rounds {})",
        same.len(),
        join(&same)
    );
    let mut area: BTreeMap<String, f64> = BTreeMap::new();
    for f in rf {
        for (k, v) in &f.region_s {
            *area.entry(k.clone()).or_insert(0.0) += v;
        }
    }
    let total: f64 = area.values().sum();
    if total > 0.0 {
        let mut v: Vec<_> = area.into_iter().collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        let parts = v
            .iter()
            .map(|(k, s)| format!("{k} {:.0}%", 100.0 * s / total))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(o, "- Time alive after the barrier by area: {parts}");
    }
    let rot: Vec<&RoundFact> = rf
        .iter()
        .filter(|f| f.side == Some(Side::Defense) && f.plant_site.is_some())
        .collect();
    if !rot.is_empty() {
        let _ = writeln!(
            o,
            "- Defense rounds with a plant: reached the spike in {} of {} (median {} s after the plant)",
            rot.iter().filter(|f| f.rotation_s.is_some()).count(),
            rot.len(),
            median(rot.iter().filter_map(|f| f.rotation_s).collect())
                .map(|m| format!("{m:.1}"))
                .unwrap_or("n/a".into())
        );
    }
    let _ = writeln!(o);
}

fn utility(r: &MatchReview, o: &mut String) {
    let rf = &r.round_facts;
    let _ = writeln!(o, "## Utility");
    let _ = writeln!(o);
    let mut parts = Vec::new();
    for slot in [3, 4, 5, 9] {
        let key = slot_key(slot_name(slot).unwrap_or(""));
        let n: u32 = rf.iter().filter_map(|f| f.casts.get(key)).sum();
        let name = r.abilities.get(key).map(String::as_str).unwrap_or("?");
        parts.push(format!(
            "{key} {name} {n} ({:.2}/round)",
            f64::from(n) / rf.len().max(1) as f64
        ));
    }
    let _ = writeln!(o, "- Casts: {}", parts.join(", "));
    let ult_ready_deaths = r
        .death_facts
        .iter()
        .filter(|f| f.context.ult_ready == Some(true))
        .count();
    let _ = writeln!(
        o,
        "- Ultimates used: {} (rounds {}); died holding a ready ult: {ult_ready_deaths}",
        rf.iter().filter(|f| f.ult_used).count(),
        join(&rf.iter().filter(|f| f.ult_used).map(|f| f.round).collect::<Vec<_>>())
    );
    let before: u32 = rf.iter().map(|f| f.casts_before_contact).sum();
    let all: u32 = rf
        .iter()
        .map(|f| f.casts.iter().filter(|(k, _)| **k != "X").map(|(_, v)| v).sum::<u32>())
        .sum();
    let _ = writeln!(o, "- Non-ult casts before the round's first hit: {before} of {all}");
    let wasted = r.death_facts.iter().filter(|f| f.context.utility_unused_value).count();
    let _ = writeln!(
        o,
        "- Deaths with a cast in the 5 s before: {}; of those with no damage dealt after the cast: {wasted}",
        r.death_facts.iter().filter(|f| f.context.casts_last_5s > 0).count()
    );
    let _ = writeln!(o);
}

fn economy(r: &MatchReview, o: &mut String) {
    let rf = &r.round_facts;
    let _ = writeln!(o, "## Economy");
    let _ = writeln!(o);
    let by_type: Vec<String> = ["pistol", "full", "half", "force", "eco", "save"]
        .iter()
        .filter_map(|t| {
            let rs: Vec<&RoundFact> = rf.iter().filter(|f| f.economy.buy_type == *t).collect();
            (!rs.is_empty()).then(|| format!("{t} {} ({})", rs.len(), wl(&rs)))
        })
        .collect();
    let _ = writeln!(o, "- My buys (rounds, won-lost): {}", by_type.join(", "));
    fn majority(m: &BTreeMap<String, u32>) -> Option<&str> {
        m.iter().max_by_key(|(_, n)| **n).map(|(k, _)| k.as_str())
    }
    let class = |t: &str| match t {
        "full" | "half" => "buy",
        "force" => "force",
        _ => "save",
    };
    let misaligned: Vec<u32> = rf
        .iter()
        .filter(|f| !f.pistol)
        .filter(|f| majority(&f.team_buys).is_some_and(|m| class(m) != class(&f.economy.buy_type)))
        .map(|f| f.round)
        .collect();
    let _ = writeln!(
        o,
        "- Bought differently from most of my team: {} rounds ({})",
        misaligned.len(),
        join(&misaligned)
    );
    let mut prev_won: Option<bool> = None;
    let mut bonus: Vec<&RoundFact> = Vec::new();
    for f in rf {
        if prev_won == Some(true)
            && !f.pistol
            && f.economy.spent.is_some_and(|s| s < 1000)
            && f.economy.loadout_value >= 2000
        {
            bonus.push(f);
        }
        prev_won = f.won;
    }
    let anti_eco: Vec<&RoundFact> = rf
        .iter()
        .filter(|f| {
            majority(&f.team_buys) == Some("full") && majority(&f.enemy_buys).is_some_and(|m| m == "eco" || m == "save")
        })
        .collect();
    let forces: Vec<&RoundFact> = rf.iter().filter(|f| f.economy.buy_type == "force").collect();
    let _ = writeln!(
        o,
        "- Force buys {} ({}); bonus rounds (won the last round, kept a gun, spent < 1000) {} ({}); anti-eco rounds {} ({})",
        forces.len(),
        wl(&forces),
        bonus.len(),
        wl(&bonus),
        anti_eco.len(),
        wl(&anti_eco)
    );
    let rifle_no_heavy: Vec<u32> = rf
        .iter()
        .filter(|f| f.economy.loadout_value >= 2900 && f.economy.armor.as_deref() != Some("Heavy Armor"))
        .filter(|f| f.economy.primary.is_some())
        .map(|f| f.round)
        .collect();
    let _ = writeln!(
        o,
        "- Rounds with a 2,900+ loadout but no heavy armour: {} ({})",
        rifle_no_heavy.len(),
        join(&rifle_no_heavy)
    );
    let save_worthy: Vec<u32> = r
        .death_facts
        .iter()
        // Down two or more: enemies alive >= my team alive (me included) + 2.
        .filter(|f| f.context.inventory_value.is_some_and(|v| v >= 2900) && f.enemies_alive >= f.teammates_alive + 3)
        .filter_map(|f| f.round)
        .collect();
    let _ = writeln!(
        o,
        "- Died with 2,900+ credits of gear while down two or more players: {} (rounds {})",
        save_worthy.len(),
        join(&save_worthy)
    );
    let _ = writeln!(o);
}

fn rounds(r: &MatchReview, o: &mut String) {
    let rf = &r.round_facts;
    let all: Vec<&RoundFact> = rf.iter().collect();
    let side = |s: Side| rf.iter().filter(|f| f.side == Some(s)).collect::<Vec<_>>();
    let pistols: Vec<&RoundFact> = rf.iter().filter(|f| f.pistol).collect();
    let _ = writeln!(o, "## Rounds");
    let _ = writeln!(o);
    let mut streak = 0;
    let mut longest = 0;
    for f in rf {
        if f.won == Some(false) {
            streak += 1;
            longest = longest.max(streak);
        } else {
            streak = 0;
        }
    }
    let _ = writeln!(
        o,
        "- Score {} (attack {}, defense {}); pistols {}; overtime rounds {}; longest loss streak {longest}",
        wl(&all),
        wl(&side(Side::Attack)),
        wl(&side(Side::Defense)),
        wl(&pistols),
        rf.iter().filter(|f| f.overtime).count()
    );
    let tfk: Vec<&RoundFact> = rf.iter().filter(|f| f.team_first_kill == Some(true)).collect();
    let tfd: Vec<&RoundFact> = rf.iter().filter(|f| f.team_first_kill == Some(false)).collect();
    let _ = writeln!(
        o,
        "- First kill of the round: mine {}, my team's {} rounds ({}); first death mine {}, enemy got the first kill {} rounds ({})",
        rf.iter().filter(|f| f.first_kill).count(),
        tfk.len(),
        wl(&tfk),
        rf.iter().filter(|f| f.first_death).count(),
        tfd.len(),
        wl(&tfd)
    );
    let my_contact: Vec<f64> = rf.iter().filter_map(|f| f.my_first_contact_s).collect();
    let _ = writeln!(
        o,
        "- First contact: round median {} s after the barrier; mine {} s",
        median(rf.iter().filter_map(|f| f.first_contact_s).collect())
            .map(|m| format!("{m:.1}"))
            .unwrap_or("n/a".into()),
        median(my_contact).map(|m| format!("{m:.1}")).unwrap_or("n/a".into())
    );
    let post_plant: Vec<&RoundFact> = rf
        .iter()
        .filter(|f| f.side == Some(Side::Attack) && f.plant_site.is_some())
        .collect();
    let retakes: Vec<&RoundFact> = rf
        .iter()
        .filter(|f| f.side == Some(Side::Defense) && f.plant_site.is_some())
        .collect();
    let _ = writeln!(
        o,
        "- Spike: carried {} rounds, planted {}, defused {}. Post-plant (attack) {}; retakes (defense) {}. Sites planted: {}",
        rf.iter().filter(|f| f.carried_spike).count(),
        rf.iter().filter(|f| f.planted).count(),
        rf.iter().filter(|f| f.defused).count(),
        wl(&post_plant),
        wl(&retakes),
        counts(rf.iter().filter_map(|f| f.plant_site.clone()))
    );
    let clutches: Vec<String> = rf
        .iter()
        .filter_map(|f| {
            f.clutch_vs.map(|n| {
                format!(
                    "r{} 1v{n} {}",
                    f.round,
                    if f.won == Some(true) { "won" } else { "lost" }
                )
            })
        })
        .collect();
    let _ = writeln!(
        o,
        "- Clutches (last alive): {}",
        if clutches.is_empty() {
            "none".into()
        } else {
            clutches.join(", ")
        }
    );
    let d = &r.death_facts;
    let late = d
        .iter()
        .filter(|f| f.context.clock_left_s.is_some_and(|s| s <= 15.0))
        .count();
    let _ = writeln!(
        o,
        "- Deaths carrying the spike {}, after the plant {}, while outnumbered {}, with 15 s or less on the clock {late}",
        d.iter().filter(|f| f.context.carrying_spike).count(),
        d.iter().filter(|f| f.context.post_plant).count(),
        d.iter().filter(|f| f.context.outnumbered).count()
    );
    let _ = writeln!(o);
}

/// One row per round, for the reviewed player.
pub fn rounds_table(r: &MatchReview, o: &mut String) {
    let _ = writeln!(o, "## Rounds table");
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "| rnd | side | result | buy | loadout | K | died | dmg | head/hits | shots | first | casts | spike |"
    );
    let _ = writeln!(o, "|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for f in &r.round_facts {
        let side = match f.side {
            Some(Side::Attack) => "atk",
            Some(Side::Defense) => "def",
            None => "?",
        };
        let result = match f.won {
            Some(true) => format!("W {}", f.result.as_deref().unwrap_or("")),
            Some(false) => format!("L {}", f.result.as_deref().unwrap_or("")),
            None => "?".into(),
        };
        let hits = f.hits_head + f.hits_body + f.hits_legs;
        let first = match (f.first_kill, f.first_death) {
            (true, _) => "FK",
            (_, true) => "FD",
            _ => "",
        };
        let casts = f
            .casts
            .iter()
            .map(|(k, v)| format!("{k}{v}"))
            .collect::<Vec<_>>()
            .join(" ");
        let spike = [
            f.carried_spike.then_some("carried"),
            f.planted.then_some("planted"),
            f.defused.then_some("defused"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
        let gun = f.economy.primary.as_deref().unwrap_or("-");
        let _ = writeln!(
            o,
            "| {} | {side} | {result} | {} {gun} | {} | {} | {} | {:.0} | {}/{hits} | {} | {first} | {casts} | {spike} |",
            f.round,
            f.economy.buy_type,
            f.economy.loadout_value,
            f.kills,
            if f.died { "yes" } else { "" },
            f.damage_dealt,
            f.hits_head,
            f.shots
        );
    }
    let _ = writeln!(o);
}

fn join(v: &[u32]) -> String {
    if v.is_empty() {
        "none".into()
    } else {
        v.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
    }
}
