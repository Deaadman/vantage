//! Checks Phase 1 against the hand-verified Lotus replay (plan section 8).
//!
//! Needs the vrfkit export of replay `11a38e1c-0cb2-4afd-b0ec-97a5e8f1d221`,
//! found at `$VALREVIEW_ORACLE_EXPORT` or `tests/data/oracle_export` at the
//! workspace root. Skips (passes with a note) when neither exists, because the
//! export is personal data and is not committed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use valreview_core::deaths::is_traded;
use valreview_core::select::select_player;
use valreview_core::{Catalog, Export, Game, MatchReview};

fn oracle_dir() -> Option<PathBuf> {
    let candidates = [
        std::env::var_os("VALREVIEW_ORACLE_EXPORT").map(PathBuf::from),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/oracle_export")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|p| p.join("manifest.json").is_file())
}

#[test]
fn lotus_oracle() {
    let Some(dir) = oracle_dir() else {
        eprintln!("skipping: set VALREVIEW_ORACLE_EXPORT to the oracle export dir");
        return;
    };
    let game = Game::new(Export::load(&dir).unwrap());
    // Built-in table only: the oracle must not depend on network or cache.
    let catalog = Catalog::builtin();
    let m = &game.export.manifest;
    assert_eq!(m.map_path.as_deref(), Some("/Game/Maps/Jam/Jam"));
    assert_eq!(m.replay_build, "++Ares-Core+release-13.08");
    assert_eq!(game.rounds.len(), 26);
    assert_eq!(game.rounds.first().unwrap().number, 0);
    assert_eq!(game.rounds.last().unwrap().number, 25);

    // Whole match.
    assert_eq!(game.players().len(), 10);
    assert_eq!(game.kills.len(), 203);
    let self_kills: Vec<_> = game.kills.iter().filter(|k| k.is_self_kill()).collect();
    assert_eq!(self_kills.len(), 1);
    assert_eq!(self_kills[0].victim, 760);

    let expected_kd: BTreeMap<u32, (usize, usize)> = [
        (760, (15, 22)),
        (1170, (24, 19)),
        (1070, (17, 18)),
        (554, (20, 21)),
        (864, (29, 18)),
        (660, (16, 23)),
        (1276, (18, 21)),
        (1482, (24, 21)),
        (968, (27, 17)),
        (1376, (12, 23)),
    ]
    .into();
    let kd: BTreeMap<u32, (usize, usize)> = game.players().iter().map(|p| (p.pawn, game.kd(p.pawn))).collect();
    assert_eq!(kd, expected_kd);
    assert_eq!(kd.values().map(|(k, _)| k).sum::<usize>(), 202);

    let mut a = game.team_of(1276);
    a.sort();
    assert_eq!(a, [660, 968, 1276, 1376, 1482]);
    let mut b = game.team_of(760);
    b.sort();
    assert_eq!(b, [554, 760, 864, 1070, 1170]);
    assert!(game.warnings.is_empty(), "{:?}", game.warnings);

    let cross: Vec<_> = game.kills.iter().filter(|k| !k.is_self_kill()).collect();
    let traded = cross.iter().filter(|k| is_traded(&game, k)).count();
    let rate = traded as f64 / cross.len() as f64;
    assert!((0.235..0.245).contains(&rate), "match trade rate {rate}");

    // Sova, pawn 1276, selected three ways.
    for sel in ["1276", "43082880", "sova"] {
        assert_eq!(select_player(game.players(), &catalog, sel).unwrap().pawn, 1276);
    }
    let me = select_player(game.players(), &catalog, "sova").unwrap();
    assert_eq!(me.agent_id.as_deref(), Some("320b2a48-4d9b-a075-30f1-1f93a9b638fa"));
    let r = MatchReview::from_game(&game, &catalog, me);
    let s = &r.summary;
    assert_eq!(s.deaths, 21);
    assert_eq!((s.first_deaths, s.first_death_rounds.as_slice()), (1, &[13][..]));
    assert_eq!(s.no_teammate_alive, 6);
    assert_eq!((s.with_teammate_alive, s.with_teammate_alive_traded), (15, 6));
    assert_eq!((s.teammate_within_10m, s.teammate_within_10m_traded), (10, 4));
    assert_eq!(s.nearest_teammate_beyond_15m, 3);
    assert_eq!((s.facing_killer_within_45deg, s.facing_known), (17, 21));
    assert_eq!(s.side_or_behind_rounds, [1, 15, 21, 25]);
    let killers: Vec<(u32, usize)> = s.killers.iter().map(|(p, _, n)| (*p, *n)).collect();
    assert_eq!(killers, [(864, 9), (554, 5), (1170, 3), (760, 2), (1070, 2)]);
    let med = s.median_since_round_start_s.unwrap();
    assert!((med - 64.6).abs() < 0.05, "median {med}");

    let spots: Vec<(usize, [f64; 3], Vec<u32>)> = r
        .clusters
        .iter()
        .map(|c| (c.deaths, c.centroid_cm, c.rounds.clone()))
        .collect();
    assert_eq!(spots.len(), 3, "{spots:?}");
    let near = |c: [f64; 3], e: [f64; 3]| c.iter().zip(e).all(|(a, b)| (a - b).abs() <= 1.0);
    assert_eq!(spots[0].0, 6);
    assert!(near(spots[0].1, [7944.0, 5708.0, 402.0]), "{spots:?}");
    assert_eq!(spots[0].2, [1, 7, 8, 9, 13, 15]);
    let three: Vec<_> = spots[1..].iter().map(|(n, c, _)| (*n, *c)).collect();
    assert!(three.iter().all(|(n, _)| *n == 3));
    assert!(
        three.iter().any(|(_, c)| near(*c, [6179.0, -4423.0, 404.0])),
        "{spots:?}"
    );
    assert!(three.iter().any(|(_, c)| near(*c, [6755.0, 89.0, 258.0])), "{spots:?}");

    // Every jsonl line parses and carries the schema version.
    for line in r.deaths_jsonl().lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["schema_version"], 3);
    }
}

#[test]
fn store_and_detect_me() {
    use valreview_core::store::{FileState, Store};
    let Some(dir) = oracle_dir() else {
        eprintln!("skipping: set VALREVIEW_ORACLE_EXPORT to the oracle export dir");
        return;
    };
    let catalog = Catalog::builtin();
    let game = Game::new(Export::load(&dir).unwrap());
    let mut store = Store::open_in_memory().unwrap();
    let ctx = store.insert_match(&game, &catalog, "a.vrf", 100, "v0.3.0").unwrap();
    assert_eq!(ctx.rounds, 26);
    assert_eq!(ctx.played_at.as_deref(), Some("2026-10-18T00:22:42Z"));
    let sova = ctx.player("43082880-1822-5e84-a3f2-337d8a51a958").unwrap();
    assert_eq!(
        (sova.agent.as_str(), sova.role.as_deref(), sova.ping_median),
        ("Sova", Some("Initiator"), Some(28))
    );
    assert!(ctx.teams.iter().all(|t| t.missing_roles.is_empty()));
    let counts = store.counts().unwrap();
    assert_eq!((counts.matches, counts.kills, counts.deaths), (1, 203, 203));
    assert!(matches!(
        store.file_state("a.vrf", 100).unwrap(),
        FileState::Stored { .. }
    ));
    // One match is not enough to guess who you are.
    assert_eq!(store.subjects_in_every_match().unwrap(), (1, vec![]));

    // A second "match" where everyone except Sova is a different account.
    let mut export = game.export.clone();
    export.manifest.match_id = "second".into();
    for p in &mut export.manifest.players {
        if p.pawn != 1276 {
            p.subject = format!("ffffffff-{}", &p.subject[9..]);
        }
    }
    store
        .insert_match(&Game::new(export), &catalog, "b.vrf", 200, "v0.3.0")
        .unwrap();
    assert_eq!(
        store.subjects_in_every_match().unwrap(),
        (2, vec!["43082880-1822-5e84-a3f2-337d8a51a958".to_owned()])
    );
}

#[test]
fn fields_extraction() {
    let Some(dir) = oracle_dir() else {
        eprintln!("skipping: set VALREVIEW_ORACLE_EXPORT to the oracle export dir");
        return;
    };
    let export = Export::load(&dir).unwrap();
    let f = &export.fields;
    let pawns: Vec<u32> = export.manifest.players.iter().map(|p| p.pawn).collect();
    // Same counts as the pandas prototype and vrfkit's damage-events view.
    assert_eq!(f.damage.len(), 1112);
    assert_eq!(
        f.damage
            .iter()
            .filter(|d| d.killed && pawns.contains(&d.victim))
            .count(),
        203
    );
    assert_eq!(f.casts.len(), 539);
    assert_eq!(f.casts.iter().filter(|c| c.slot == 9).count(), 27);
    assert_eq!(f.round_results.len(), 26);
    assert_eq!(f.planted_at_site.len(), 5);
    // The game's own cumulative counters agree with the kill events.
    assert_eq!(f.aggregate_kills.values().sum::<i64>(), 202);
    assert_eq!(f.aggregate_deaths.values().sum::<i64>(), 203);
    assert!(f.phases.iter().any(|p| p.value == 4));
    assert!(!f.ammo.is_empty() && !f.in_hand.is_empty() && !f.owners.is_empty());
}

#[test]
fn analysis_timelines() {
    use valreview_core::analysis::Analysis;
    let Some(dir) = oracle_dir() else {
        eprintln!("skipping: set VALREVIEW_ORACLE_EXPORT to the oracle export dir");
        return;
    };
    let game = Game::new(Export::load(&dir).unwrap());
    let catalog = Catalog::builtin();
    let a = Analysis::new(&game, &catalog);
    // CastTime is measured from the barrier drop: buy_end_ms + CastTime lands
    // on the replay time the cast was first seen.
    let mut residual: Vec<i64> = a
        .casts
        .iter()
        .zip(game.export.fields.casts.iter())
        .filter_map(|(c, raw)| c.time_ms.map(|t| raw.seen_ms as i64 - t as i64))
        .collect();
    residual.sort_unstable();
    let median = residual[residual.len() / 2];
    eprintln!("cast residual median {median} ms over {}", residual.len());
    assert!(median.abs() < 1000, "median cast residual {median} ms");
    // Every round has a barrier drop and a side.
    assert!(
        a.rounds
            .iter()
            .all(|r| r.buy_end_ms.is_some() && r.attacker_side.is_some())
    );
    // Wins per side add up to the rounds played.
    let wins: usize = a.rounds.iter().filter(|r| r.winner_side.is_some()).count();
    assert_eq!(wins, 26);
}

#[test]
fn phase4_facts() {
    use valreview_core::analysis::Analysis;
    use valreview_core::facts::round_facts;
    let Some(dir) = oracle_dir() else {
        eprintln!("skipping: set VALREVIEW_ORACLE_EXPORT to the oracle export dir");
        return;
    };
    let game = Game::new(Export::load(&dir).unwrap());
    let catalog = Catalog::builtin();
    let a = Analysis::new(&game, &catalog);
    let f = &game.export.fields;

    // The game's own per-player counters (BasicCombatStatsComponent on the
    // player state) agree with the kill events for every player.
    for p in game.players() {
        let (k, d) = game.kd(p.pawn);
        assert_eq!(
            f.aggregate_kills.get(&p.actor_net_guid),
            Some(&(k as i64)),
            "kills of {}",
            p.pawn
        );
        assert_eq!(
            f.aggregate_deaths.get(&p.actor_net_guid),
            Some(&(d as i64)),
            "deaths of {}",
            p.pawn
        );
    }

    let rf = round_facts(&a, 1276);
    assert_eq!(rf.len(), 26);
    assert_eq!(rf.iter().map(|r| r.kills).sum::<u32>(), 18);
    assert_eq!(rf.iter().filter(|r| r.died).count(), 21);
    // Damage dealt equals Sova's CombatReport (OwnerExclusivePlayerInfo 102): 3846.60 in total.
    let dealt: f64 = rf.iter().map(|r| r.damage_dealt).sum();
    assert!((dealt - 3846.6).abs() < 0.5, "damage dealt {dealt}");
    // Round 15 alone: CombatReport says 520.
    assert!((rf[15].damage_dealt - 520.0).abs() < 0.05);
    // Score and sides: 12-14, first half on attack.
    assert_eq!(rf.iter().filter(|r| r.won == Some(true)).count(), 12);
    assert_eq!(rf[0].side, Some(valreview_core::analysis::Side::Attack));
    assert_eq!(rf[12].side, Some(valreview_core::analysis::Side::Defense));
    // Casts by key, deduplicated: Recon Bolt (E) 22, Owl Drone (C) 12, Shock Bolt (Q) 5, ult 1.
    let casts = |k: &str| rf.iter().filter_map(|r| r.casts.get(k)).sum::<u32>();
    assert_eq!((casts("E"), casts("C"), casts("Q"), casts("X")), (22, 12, 5, 1));
    assert!(rf[0].pistol && rf[12].pistol && !rf[24].pistol);

    // Every death has a killing blow with a weapon, and the in-hand weapon is known.
    let deaths = valreview_core::deaths::death_facts(&a, 1276);
    assert!(
        deaths
            .iter()
            .all(|d| d.context.killer_weapon.is_some() && d.context.weapon_in_hand.is_some())
    );
    assert_eq!(deaths[0].context.weapon_in_hand.as_deref(), Some("Bandit"));
    assert_eq!(deaths[0].context.magazine_ammo, Some(3));
}

#[test]
fn reviews_and_trends() {
    let Some(dir) = oracle_dir() else {
        eprintln!("skipping: set VALREVIEW_ORACLE_EXPORT to the oracle export dir");
        return;
    };
    let game = Game::new(Export::load(&dir).unwrap());
    let catalog = Catalog::builtin();
    let me = game.players().iter().find(|p| p.pawn == 1276).unwrap();
    let review = MatchReview::from_game(&game, &catalog, me);

    // A stored review renders the same packet as the live export.
    let json = serde_json::to_string(&review).unwrap();
    let back: MatchReview = serde_json::from_str(&json).unwrap();
    assert_eq!(back.packet_md(), review.packet_md());
    assert_eq!(back.deaths_jsonl(), review.deaths_jsonl());

    // 40 synthetic matches: the same match, except the last 20 lose every trade.
    let reviews: Vec<MatchReview> = (0..40)
        .map(|i| {
            let mut r = back.clone();
            r.context.match_id = format!("{i:08}-synthetic");
            r.context.played_at = Some(format!("2026-10-{:02}T00:00:00Z", 1 + i / 2));
            if i >= 20 {
                for d in &mut r.death_facts {
                    d.traded_within_3s = false;
                }
            }
            r
        })
        .collect();
    let report = valreview_core::trends::trends(&review.subject, &reviews, 20);
    let top = &report.findings[0];
    assert_eq!(
        top.key,
        "trade_rate",
        "{:?}",
        report.findings.iter().map(|f| f.key).collect::<Vec<_>>()
    );
    assert!(top.comparable);
    assert_eq!(top.last.value, Some(0.0));
    assert!((top.previous.value.unwrap() - 0.4).abs() < 1e-9);
    assert!(top.score.unwrap() < -5.0);
    assert_eq!(top.consistency, Some(1.0));
    // Everything else is identical across windows, so nothing else is a change.
    assert!(report.findings[1..].iter().all(|f| f.score.unwrap_or(0.0).abs() < 1e-9));
    let md = report.markdown();
    assert!(md.contains("1. **My deaths traded within 3 s, when a teammate was alive**"));
    assert!(md.contains("00000020 (0.0% rounds without it 0,1,2,"), "{md}");
    assert_eq!(report.ranked().count(), 1, "{md}");
    // The A Hut spot repeats in every match.
    assert!(report.spots.iter().any(|s| s.matches == 40 && s.deaths == 240));

    // Below the floor, nothing is ranked.
    let small = valreview_core::trends::trends(&review.subject, &reviews[..10], 20);
    assert!(small.findings.iter().all(|f| !f.comparable));
    assert!(small.markdown().contains("Not enough data"));
}
