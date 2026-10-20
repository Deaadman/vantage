//! Every panel and tab drawn at the shell's usual sizes: nothing panics,
//! every panel is exactly as tall as asked, and rows fit 112 and 140 columns.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vantage_core::analysis::Side;
use vantage_core::clips::{Matched, ShotStyle};
use vantage_core::coach::{self, sample};
use vantage_core::{ai, habits, plan, standing};

use crate::coach_views::{HabitsPanel, PlanData, PlanPanel, RankPanel};
use crate::review_view::ReviewPanel;
use crate::shell::{Row, row_width};
use crate::{ClipFolder, Clips, ReviewData};

const SIZES: [(usize, usize); 3] = [(88, 30), (112, 34), (140, 40)];

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn match_review() -> vantage_core::MatchReview {
    let rounds: Vec<(bool, Side)> = (0..24)
        .map(|i| (i % 3 != 0, if i < 12 { Side::Attack } else { Side::Defense }))
        .collect();
    let mut deaths = Vec::new();
    for round in [0, 3, 6, 9] {
        let mut d = sample::death(round, "A Rubble");
        d.first_death_of_round = true;
        d.context.casts_last_5s = 0;
        d.nearest_teammates.clear();
        deaths.push(d);
    }
    for round in [12, 15, 18] {
        let mut d = sample::death(round, "B Main");
        d.context.same_spot_as_own_kill = true;
        d.teammates_alive = 0;
        d.enemies_alive = 4;
        deaths.push(d);
    }
    sample::review(&rounds, deaths)
}

fn check(name: &str, rows: &[Row], w: usize, h: usize) {
    assert_eq!(rows.len(), h, "{name} at {w}x{h} is {} rows", rows.len());
    if w >= 112 {
        for r in rows {
            let text: String = r.iter().map(|(_, s)| s.as_str()).collect();
            assert!(row_width(r) <= w, "{name} at {w}: {} wide: {text}", row_width(r));
        }
    }
}

fn review_data(clips: Clips, ai: Option<ai::Saved>) -> ReviewData {
    let r = match_review();
    ReviewData {
        debrief: coach::debrief(&r, &[]),
        focus: habits::check(habits::focus(None), &[r.clone(), r]),
        notes: vec!["the replay was cut short".into()],
        map: None,
        minimap: Err("no minimap in tests".into()),
        clips,
        ai,
        ai_provider: ai::provider("anthropic"),
    }
}

fn saved() -> ai::Saved {
    ai::Saved {
        output: ai::Output {
            schema: ai::SCHEMA.into(),
            summary: "Most of your deaths came from where you stood, not how you aimed.".into(),
            findings: vec![ai::Finding {
                category: "exposure".into(),
                title: "Standing where two angles could see you".into(),
                rounds: vec![1, 4],
                screenshots: vec!["R01.jpg".into()],
                saw: "You stood in the open at A Rubble.".into(),
                why: "Two players could see you.".into(),
                try_this: "Pick a spot only one angle can see.".into(),
                confidence: "high".into(),
            }],
            keep: vec![],
            unclear: vec![ai::Unclear {
                screenshot: "R04.jpg".into(),
                reason: "The clip starts after the fight.".into(),
            }],
        },
        meta: ai::Meta {
            provider: "Claude (claude-sonnet-5-5)".into(),
            model: "claude-sonnet-5-5".into(),
            saved_at: "2026-10-19T21:52:00Z".into(),
            format: ai::FORMAT_VERSION,
        },
    }
}

#[test]
fn review_tabs_fit() {
    let folders = vec![
        ClipFolder {
            name: "Medal".into(),
            about: "found 31 VALORANT clips in C:\\Users\\Player\\Videos\\Medal\\Clips\\VALORANT".into(),
            found: Some(31),
            path: Some("C:\\Videos\\Medal".into()),
        },
        ClipFolder {
            name: "Another folder…".into(),
            about: "type the path to your clips".into(),
            found: None,
            path: None,
        },
    ];
    let set = || Clips::Set {
        dir: "C:\\Users\\Player\\Videos\\Medal\\Clips\\VALORANT".into(),
        per_death: (0..7)
            .map(|i| {
                (i % 2 == 0).then(|| Matched {
                    path: format!("VALORANT 2026.10.19 - 21.0{i}.10.mp4").into(),
                    before_end_s: 12.0,
                })
            })
            .collect(),
        shots_dir: "C:\\Users\\Player\\Pictures\\Vantage\\Clips\\2026-10-19 Lotus".into(),
        shots: vec![(1, vec!["R01.jpg".into()]), (7, vec!["R07.jpg".into()])],
        shot_error: None,
        style: ShotStyle::Strip,
    };
    for data in [
        review_data(Clips::NotSet(folders), None),
        review_data(set(), None),
        review_data(set(), Some(saved())),
    ] {
        let mut p = ReviewPanel::new(data);
        for tab in 0..8 {
            for (w, h) in SIZES {
                check(&format!("review tab {tab}"), &p.rows(w, h), w, h);
            }
            p.key(key(KeyCode::Down));
            p.key(key(KeyCode::Enter));
            p.key(key(KeyCode::Char(char::from(b'1' + tab as u8))));
            p.key(key(KeyCode::Right));
        }
        // The AI tab's format view.
        p.key(key(KeyCode::Char('8')));
        p.key(key(KeyCode::Char('j')));
        for (w, h) in SIZES {
            check("review ai json", &p.rows(w, h), w, h);
        }
    }
}

#[test]
fn plan_habits_and_rank_fit() {
    let reviews: Vec<_> = (0..4).map(|_| match_review()).collect();
    for n in [1, 4] {
        let mut p = PlanPanel::new(PlanData {
            plan: plan::plan("Lotus", &reviews[..n]),
            rank: Some("Gold 2".into()),
            fix: Some(("First into A Rubble alone".into(), "Use an ability first.".into())),
            map: None,
            minimap: Err("none".into()),
        });
        for tab in 0..3 {
            for (w, h) in SIZES {
                check(&format!("plan {n} tab {tab}"), &p.rows(w, h), w, h);
            }
            p.key(key(KeyCode::Right));
        }
    }
    let mut h = HabitsPanel::new(habits::progress(&reviews), "untraded", reviews.len());
    h.key(key(KeyCode::Down));
    h.key(key(KeyCode::Enter));
    for (w, hh) in SIZES {
        check("habits", &h.rows(w, hh), w, hh);
    }
    let s = standing::Standing {
        better: vec!["You do more damage than most players in your games.".into()],
        behind: vec!["You die first in a round more often than almost anyone in your games.".into()],
        games: 4,
    };
    for rank in ["Iron 1", "Gold 2", "Ascendant 3", "Radiant"] {
        let mut r = RankPanel::new(rank.into(), s.clone(), "Dying where nobody can trade");
        for _ in 0..2 {
            for (w, h) in SIZES {
                check("rank", &r.rows(w, h), w, h);
            }
            r.key(key(KeyCode::Right));
        }
    }
}

#[test]
fn settings_fit() {
    let dir = std::env::temp_dir().join(format!("vantage-render-{}", std::process::id()));
    let mut p = crate::settings::SettingsPanel::new(&dir);
    for (w, h) in SIZES {
        assert!(p.rows(w, h).len() <= h);
    }
    p.key(key(KeyCode::Right));
    // Every setting, and every picker opened.
    for _ in 0..9 {
        p.key(key(KeyCode::Enter));
        for (w, h) in SIZES {
            let rows = p.rows(w, h);
            assert!(rows.len() <= h);
        }
        p.key(key(KeyCode::Esc));
        p.key(key(KeyCode::Down));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every panel loaded from a real data folder, when `VANTAGE_SMOKE_DATA` names one.
#[test]
fn real_data_panels_open_and_fit() {
    let Some(dir) = std::env::var_os("VANTAGE_SMOKE_DATA").map(std::path::PathBuf::from) else {
        eprintln!("skipping: set VANTAGE_SMOKE_DATA to a data folder with scanned matches");
        return;
    };
    for m in crate::my_matches(&dir) {
        let mut p = ReviewPanel::new(crate::load_review(&dir, Some(m.id.clone())).unwrap());
        for tab in 0..8 {
            p.key(key(KeyCode::Char(char::from(b'1' + tab as u8))));
            for (w, h) in SIZES {
                check(&format!("{} tab {tab}", m.id), &p.rows(w, h), w, h);
            }
        }
    }
    let mut p = PlanPanel::new(crate::load_plan(&dir, None).unwrap());
    let (progress, focus, n) = crate::load_habits(&dir).unwrap();
    let mut hp = HabitsPanel::new(progress, focus, n);
    let (rank, standing, f) = crate::load_rank(&dir).unwrap();
    let mut rp = RankPanel::new(rank, standing, f);
    for (w, h) in SIZES {
        check("plan", &p.rows(w, h), w, h);
        check("habits", &hp.rows(w, h), w, h);
        check("rank", &rp.rows(w, h), w, h);
    }
}
