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
use valreview_core::{Export, Game, Review};

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
        assert_eq!(select_player(game.players(), sel).unwrap().pawn, 1276);
    }
    let me = select_player(game.players(), "sova").unwrap();
    assert_eq!(me.agent_id.as_deref(), Some("320b2a48-4d9b-a075-30f1-1f93a9b638fa"));
    let r = Review::new(&game, me);
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
        assert_eq!(v["schema_version"], 1);
    }
}
