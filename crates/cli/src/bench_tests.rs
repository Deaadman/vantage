//! Timings of the work behind each screen, on made-up matches, for a
//! baseline to compare against. Ignored by `cargo test`; run it with
//!
//! `cargo test --release -p vantage bench -- --ignored --nocapture`
//!
//! and set `BENCH_OUT` to a file to also get the results as JSON.

use std::hint::black_box;
use std::time::{Duration, Instant};

use crossterm::event::KeyCode;
use vantage_core::clips::{Matched, ShotStyle};
use vantage_core::{MatchReview, clusters, coach, habits, plan};

use crate::Clips;
use crate::coach_views::HabitsPanel;
use crate::panel::MapCells;
use crate::render_tests::{key, match_review, review_data, saved};
use crate::review_view::ReviewPanel;

/// One timing: the median and 95th percentile of many runs, in microseconds.
struct Timing {
    name: &'static str,
    runs: usize,
    median_us: f64,
    p95_us: f64,
}

/// Runs `f` for about half a second (at least 20 times, after a warm-up).
fn time(name: &'static str, mut f: impl FnMut()) -> Timing {
    for _ in 0..3 {
        f();
    }
    let mut runs: Vec<Duration> = Vec::new();
    let start = Instant::now();
    while runs.len() < 20 || (start.elapsed() < Duration::from_millis(500) && runs.len() < 100_000) {
        let t = Instant::now();
        f();
        runs.push(t.elapsed());
    }
    runs.sort();
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    Timing {
        name,
        runs: runs.len(),
        median_us: us(runs[runs.len() / 2]),
        p95_us: us(runs[runs.len() * 95 / 100]),
    }
}

fn reviews(n: usize) -> Vec<MatchReview> {
    (0..n).map(|_| match_review()).collect()
}

#[test]
#[ignore = "a benchmark: run with --ignored --release"]
fn bench() {
    let one = match_review();
    let eight = reviews(habits::WINDOW);
    let json = serde_json::to_string(&one).unwrap();
    let clips = || Clips::Set {
        dir: "clips".into(),
        per_death: (0..7)
            .map(|i| {
                (i % 2 == 0).then(|| Matched {
                    path: format!("MedalTVValorant20261019210{i}10.mp4").into(),
                    before_end_s: 12.0,
                    length_s: Some(30.0),
                    bytes: Some(50_544_640),
                })
            })
            .collect(),
        shots_dir: "shots".into(),
        shots: vec![(1, vec!["R01.jpg".into()])],
        shot_error: None,
        waiting: None,
        stamp: 0,
        style: ShotStyle::Grid,
    };
    let mut review = ReviewPanel::new(review_data(clips(), Some(saved())));
    let mut habits_panel = HabitsPanel::new(habits::progress(&eight), "untraded", eight.len());
    let px = vec![[90u8, 90, 90, 255]; 128 * 128];
    let points: Vec<Option<[f64; 3]>> = (0..1000)
        .map(|i| Some([(i * 7919 % 20_000) as f64, (i * 104_729 % 20_000) as f64, 0.0]))
        .collect();

    let mut results = vec![
        time("Read a stored match (JSON)", || {
            black_box(serde_json::from_str::<MatchReview>(black_box(&json)).unwrap());
        }),
        time("Save a match (JSON)", || {
            black_box(serde_json::to_string(black_box(&one)).unwrap());
        }),
        time("Debrief of one match", || {
            black_box(coach::debrief(black_box(&one), &[]));
        }),
        time("Habits, the last 8 matches", || {
            black_box(habits::progress(black_box(&eight)));
        }),
        time("Map plan from 8 matches", || {
            black_box(plan::plan("Lotus", black_box(&eight)));
        }),
        time("Death spots, 1000 deaths", || {
            black_box(clusters::leader_clusters(black_box(&points)));
        }),
        time("Minimap, turned and drawn", || {
            black_box(MapCells::new(black_box(&px), 128, None));
        }),
        time("/habits screen, 140x40", || {
            black_box(habits_panel.rows(140, 40));
        }),
    ];
    for (tab, name) in [
        (1, "/review Debrief, 140x40"),
        (2, "/review Fixes, 140x40"),
        (4, "/review Clips, 140x40"),
        (6, "/review AI, 140x40"),
    ] {
        review.key(key(KeyCode::Char(char::from(b'0' + tab))));
        results.push(time(name, || {
            black_box(review.rows(140, 40));
        }));
    }

    println!("{:<34}{:>12}{:>12}{:>10}", "", "median µs", "p95 µs", "runs");
    for t in &results {
        println!("{:<34}{:>12.1}{:>12.1}{:>10}", t.name, t.median_us, t.p95_us, t.runs);
    }
    if let Some(path) = std::env::var_os("BENCH_OUT") {
        let rows: Vec<serde_json::Value> = results
            .iter()
            .map(|t| serde_json::json!({"name": t.name, "median_us": t.median_us, "p95_us": t.p95_us, "runs": t.runs}))
            .collect();
        std::fs::write(path, serde_json::to_string_pretty(&rows).unwrap()).unwrap();
    }
}
