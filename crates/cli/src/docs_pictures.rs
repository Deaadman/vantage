//! Redraws the pictures in `docs/images`: the real screens, on made-up
//! matches, written as SVG, and the README's GIF of a launch and a review.
//! Ignored by `cargo test`; run
//! `scripts/redraw-docs-images.ps1` after changing a screen the docs show.
//! It needs the network for the map catalog and Lotus's minimap.

use std::path::{Path, PathBuf};
use std::time::Instant;

use abalone::Logo;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Color;
use vantage_core::analysis::Side;
use vantage_core::clips::{Matched, ShotStyle};
use vantage_core::coach::{self, sample};
use vantage_core::{ai, habits, plan, standing};

use super::*;
use crate::coach_views::{HabitsPanel, PlanData, PlanPanel, RankPanel};
use crate::logo;
use crate::review_view::ReviewPanel;
use crate::{Clips, ReviewData};

/// The terminal size every picture is drawn at.
const W: usize = 112;
const H: usize = 34;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

// ---------------------------------------------------------------- the screen

/// What the shell draws, as rows.
fn screen(shell: &mut Shell<Vantage>) -> Vec<Row> {
    shell.render(W, H).rows
}

/// The header with the logo at `frame`, for made-up settings.
fn header(rank: Option<&str>, frame: usize) -> abalone::Item {
    abalone::Item::Rows(abalone::skin::header(
        Chevrons.rows(frame),
        &header_lines(rank, "VALORANT v13.08", "C:\\Users\\Player\\Documents\\VALORANT\\Demos"),
        W,
    ))
}

fn row(pieces: Vec<(Kind, &str)>) -> abalone::Item {
    abalone::Item::Row(pieces.into_iter().map(|(k, t)| (k, t.to_owned())).collect())
}

/// The shell just after launch: the scan found one new match.
fn launched(shell: &mut Shell<Vantage>) {
    *shell.transcript_mut() = vec![
        header(Some("Gold 2"), logo::FRAMES - 1),
        row(vec![
            (Kind::Bold, "Checked 14 replays"),
            (Kind::Dim, "  1 new · 13 already done"),
        ]),
        row(vec![(Kind::Good, "  + "), (Kind::Out, "Sova on Lotus")]),
        abalone::Item::Line(Kind::Out, String::new()),
        row(vec![
            (Kind::Paint(YELLOW, false), "◆ "),
            (Kind::Out, "Your focus: "),
            (Kind::Bold, "Dying where nobody can trade"),
            (Kind::Paint(crate::review_view::GREEN, false), "  ▇▅▅▃▂"),
            (Kind::Paint(crate::review_view::GREEN, true), "  getting better"),
        ]),
        abalone::Item::Line(Kind::Dim, "  To switch, type /focus and pick a habit.".into()),
    ];
    shell.reset_view();
    shell.pretend_job(None);
    shell.set_notices(vec![]);
    shell.set_footer("14 matches · /help for commands and keys");
    shell.set_placeholder(placeholder(true));
}

// ---------------------------------------------------------------- made-up matches

/// A made-up match on Lotus: `shift` 0 is the oldest; later ones die alone at A less often.
fn match_review(shift: u32) -> vantage_core::MatchReview {
    let entry_rounds: Vec<u32> = [1, 4, 7, 10, 2, 8]
        .into_iter()
        .take(6 - shift.min(4) as usize)
        .collect();
    let mut deaths = Vec::new();
    let uv = [
        [0.28, 0.32],
        [0.33, 0.27],
        [0.24, 0.40],
        [0.30, 0.36],
        [0.26, 0.30],
        [0.31, 0.38],
    ];
    for (k, &round) in entry_rounds.iter().enumerate() {
        let mut d = sample::death(round, "A Rubble");
        d.first_death_of_round = true;
        d.context.casts_last_5s = 0;
        d.nearest_teammates[0].dist_m = 30.0;
        d.minimap_uv = Some(uv[k]);
        deaths.push(d);
    }
    for (k, round) in [13, 16, 19].into_iter().enumerate() {
        let mut d = sample::death(round, "B Main");
        d.context.same_spot_as_own_kill = true;
        d.teammates_alive = 0;
        d.enemies_alive = 4;
        d.context.attackers_last_3s = 2;
        d.minimap_uv = Some([[0.62, 0.70], [0.58, 0.66], [0.66, 0.74]][k]);
        deaths.push(d);
    }
    let mut d = sample::death(21, "C Mound");
    d.killer_agent = "Jett".into();
    d.minimap_uv = Some([0.75, 0.40]);
    deaths.push(d);
    let died: Vec<u32> = deaths.iter().filter_map(|d| d.round).collect();
    let rounds: Vec<(bool, Side)> = (0..22u32)
        .map(|i| {
            let won = if died.contains(&i) {
                i == 21 || i == 16
            } else {
                i % 4 != 3 && i != 9
            };
            (won, if i < 12 { Side::Attack } else { Side::Defense })
        })
        .collect();
    let mut r = sample::review(&rounds, deaths);
    let id = format!("m{shift}");
    r.context.match_id = id.clone();
    r.context.map_name = Some("Lotus".into());
    r.context.played_at = Some(format!("2026-10-{:02}T21:04:11Z", 14 + shift));
    r.kills = 14 + shift as usize;
    for d in &mut r.death_facts {
        d.match_id = id.clone();
    }
    for (i, f) in r.round_facts.iter_mut().enumerate() {
        f.match_id = id.clone();
        f.pistol = i == 0 || i == 12;
        f.economy.buy_type = match i {
            0 | 12 => "pistol",
            1 | 13 => "eco",
            2 | 14 => "force",
            7 | 19 => "half",
            _ => "full",
        }
        .into();
        f.kills = (i as u32 * 7 + shift) % 3;
    }
    r
}

fn saved() -> ai::Saved {
    let finding = |category: &str,
                   title: &str,
                   rounds: Vec<u32>,
                   shots: &[&str],
                   saw: &str,
                   why: &str,
                   try_this: &str,
                   confidence: &str| ai::Finding {
        category: category.into(),
        title: title.into(),
        rounds,
        screenshots: shots.iter().map(|s| (*s).into()).collect(),
        saw: saw.into(),
        why: why.into(),
        try_this: try_this.into(),
        confidence: confidence.into(),
    };
    ai::Saved {
        output: ai::Output {
            schema: ai::SCHEMA.into(),
            summary: "Most of your deaths came from where you stood, not how you aimed: you held open spots two angles could see, and you were often looking at the wrong one.".into(),
            findings: vec![
                finding(
                    "exposure",
                    "Standing where two angles could see you",
                    vec![1, 4, 7],
                    &["R01.jpg", "R04.jpg"],
                    "You stood in the open at A Rubble with Tree and Root both able to see you.",
                    "Two players could see you, so winning the first fight still left you exposed to the second.",
                    "Pick a spot only one angle can see, and clear the other with your recon first.",
                    "high",
                ),
                finding(
                    "crosshair",
                    "Crosshair at the floor while walking in",
                    vec![13, 16],
                    &["R13.jpg"],
                    "Your crosshair was below head height coming through B Main.",
                    "The killer only had to click; you had to flick up first.",
                    "Keep it at head height on the corner before you step out.",
                    "medium",
                ),
            ],
            keep: vec![ai::Keep {
                title: "Your utility timing on retakes was good".into(),
                rounds: vec![10],
                screenshots: vec![],
                saw: "You flashed before swinging on the B retake.".into(),
            }],
            unclear: vec![ai::Unclear {
                screenshot: "R19.jpg".into(),
                reason: "The clip starts after the fight.".into(),
            }],
        },
        meta: ai::Meta {
            provider: "Claude (claude-sonnet-5-5)".into(),
            model: "claude-sonnet-5-5".into(),
            saved_at: "2026-10-19T21:52:00Z".into(),
            format: ai::FORMAT_VERSION,
            app: None,
        },
    }
}

fn clips() -> Clips {
    Clips::Set {
        dir: "C:\\Users\\Player\\Videos\\Medal\\Clips\\VALORANT".into(),
        per_death: (0..8)
            .map(|i| {
                (i % 3 != 2).then(|| Matched {
                    path: format!("VALORANT 2026.10.19 - 21.{:02}.10.mp4", 4 + i * 2).into(),
                    before_end_s: 12.0,
                    length_s: Some(if i == 4 { 45.0 } else { 30.0 }),
                    bytes: Some(50_544_640 + i as u64 * 380_000),
                })
            })
            .collect(),
        shots_dir: "C:\\Users\\Player\\Pictures\\Vantage\\Clips\\2026-10-19 Lotus".into(),
        shots: [1, 4, 7, 13, 16]
            .map(|r| (r, vec![format!("R{r:02}.jpg").into()]))
            .to_vec(),
        shot_error: None,
        style: ShotStyle::Strip,
    }
}

// ---------------------------------------------------------------- SVG

/// One screen as an SVG: each run of text placed on the character grid, so it
/// lines up whatever monospace font the viewer has. `subs` replace this PC's
/// paths and user name with made-up ones.
fn svg(rows: &[Row], subs: &[(String, String)]) -> String {
    svg_rows(rows, subs, false)
}

/// [`svg`], keeping the blank rows at the bottom when `full`, so every frame
/// of a GIF is the same size.
fn svg_rows(rows: &[Row], subs: &[(String, String)], full: bool) -> String {
    const CW: f64 = 8.4;
    const LH: usize = 18;
    const PAD: usize = 16;
    let hex = |c: Option<Color>, default: &str| match c {
        Some(Color::Rgb { r, g, b }) => format!("#{r:02x}{g:02x}{b:02x}"),
        Some(Color::Black) => "#000000".into(),
        Some(Color::White) => "#ffffff".into(),
        _ => default.into(),
    };
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let mut rows: Vec<&Row> = rows.iter().collect();
    while !full && rows.last().is_some_and(|r| r.iter().all(|(_, t)| t.trim().is_empty())) {
        rows.pop();
    }
    let (w, h) = ((W as f64 * CW) as usize + 2 * PAD, rows.len() * LH + 2 * PAD);
    let mut out = vec![
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" font-family="Cascadia Mono, Consolas, Menlo, monospace" font-size="14">"#
        ),
        format!(r##"<rect width="{w}" height="{h}" rx="8" fill="#16161e"/>"##),
    ];
    for (r, row) in rows.iter().enumerate() {
        let y = PAD + r * LH;
        // Clipped like the shell: one short of the width.
        let mut col = 0;
        for (style, text) in row.iter() {
            let mut text = text.clone();
            for (from, to) in subs {
                text = text.replace(from.as_str(), to);
            }
            let text: String = text.chars().take((W - 1).saturating_sub(col)).collect();
            let n = text.chars().count();
            let x = PAD as f64 + col as f64 * CW;
            if let Some(bg) = style.bg {
                out.push(format!(
                    r#"<rect x="{x:.1}" y="{y}" width="{:.1}" height="{LH}" fill="{}"/>"#,
                    n as f64 * CW,
                    hex(Some(bg), "")
                ));
            }
            if !text.trim().is_empty() {
                let mut attrs = format!(r#"x="{x:.1}" y="{}" fill="{}""#, y + LH - 5, hex(style.fg, "#d8d8e0"));
                if style.bold {
                    attrs.push_str(r#" font-weight="bold""#);
                }
                if n > 1 {
                    attrs.push_str(&format!(
                        r#" textLength="{:.1}" lengthAdjust="spacingAndGlyphs""#,
                        n as f64 * CW
                    ));
                }
                out.push(format!(r#"<text {attrs} xml:space="preserve">{}</text>"#, esc(&text)));
            }
            col += n;
        }
    }
    out.push("</svg>".into());
    out.join("\n") + "\n"
}

// ---------------------------------------------------------------- GIF

/// Screens as a looping GIF, each shown for its centiseconds. The SVGs are
/// drawn with this PC's fonts; a screen the same as the one before only
/// lengthens it.
fn gif(frames: &[(Vec<Row>, u16)], subs: &[(String, String)], path: &Path) {
    use resvg::{tiny_skia, usvg};
    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    let mut merged: Vec<(String, u16)> = Vec::new();
    for (rows, cs) in frames {
        let svg = svg_rows(rows, subs, true);
        match merged.last_mut() {
            Some((last, total)) if *last == svg => *total += cs,
            _ => merged.push((svg, *cs)),
        }
    }
    let mut encoder = None;
    for (svg, cs) in merged {
        let tree = usvg::Tree::from_str(&svg, &opt).unwrap();
        let size = tree.size().to_int_size();
        let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height()).unwrap();
        resvg::render(&tree, tiny_skia::Transform::identity(), &mut pixmap.as_mut());
        let (w, h) = (size.width() as u16, size.height() as u16);
        let enc = encoder.get_or_insert_with(|| {
            let mut e = gif::Encoder::new(std::fs::File::create(path).unwrap(), w, h, &[]).unwrap();
            e.set_repeat(gif::Repeat::Infinite).unwrap();
            e
        });
        let mut frame = gif::Frame::from_rgba_speed(w, h, &mut pixmap.take(), 10);
        frame.delay = cs;
        enc.write_frame(&frame).unwrap();
    }
}

/// Launch: the logo plays while the scan runs, the scan finds a match, then
/// `/review` is typed and each tab shown in turn.
fn launch_and_review(shell: &mut Shell<Vantage>, review: ReviewData) -> Vec<(Vec<Row>, u16)> {
    let mut frames = Vec::new();
    launched(shell);
    let scanned = std::mem::take(shell.transcript_mut());
    for tick in 0..logo::FRAMES as u32 + 12 {
        *shell.transcript_mut() = vec![header(Some("Gold 2"), (tick as usize).min(logo::FRAMES - 1))];
        shell.pretend_job(Some((
            "Checking for new replays".into(),
            Instant::now() - TICK * tick - TICK / 2,
        )));
        frames.push((screen(shell), 8));
    }
    shell.pretend_job(None);
    *shell.transcript_mut() = scanned;
    frames.push((screen(shell), 150));
    for c in "/review".chars() {
        shell.editor_mut().insert_str(&c.to_string());
        frames.push((screen(shell), 12));
    }
    frames.push((screen(shell), 60));
    shell.reset_view();
    shell.open(Panel::Review(ReviewPanel::new(review)));
    for tab in '1'..='6' {
        if let Some(Panel::Review(p)) = shell.panel() {
            p.key(key(KeyCode::Char(tab)));
        }
        frames.push((screen(shell), if tab == '6' { 400 } else { 250 }));
    }
    shell.reset_view();
    frames
}

// ---------------------------------------------------------------- the pictures

#[test]
#[ignore = "redraws docs/images: run scripts/redraw-docs-images.ps1"]
fn redraw_docs_images() {
    // Times as written (UTC), so every machine draws the same pictures.
    vantage_core::time::set_fixed_offset(Some(0));
    let out_dir = std::env::var_os("DOCS_IMAGES").map(PathBuf::from).unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("docs")
            .join("images")
    });
    let base = std::env::temp_dir().join(format!("vantage-docs-{}", std::process::id()));
    let data_dir = base.join("data");
    let demos = base.join("Demos");
    let clip_dir = base.join("Clips");
    for d in [&data_dir, &demos, &clip_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut subs = vec![
        (
            demos.display().to_string(),
            "C:\\Users\\Player\\Documents\\VALORANT\\Demos".to_owned(),
        ),
        (
            clip_dir.display().to_string(),
            "C:\\Users\\Player\\Videos\\Medal\\Clips\\VALORANT".to_owned(),
        ),
        (
            data_dir.display().to_string(),
            "C:\\Users\\Player\\AppData\\Local\\vantage\\data".to_owned(),
        ),
    ];
    if let Ok(user) = std::env::var("USERNAME")
        && !user.is_empty()
    {
        subs.push((format!("\\{user}\\"), "\\Player\\".into()));
    }

    // The real catalog and Lotus's minimap: a picture without them would be wrong.
    let catalog = vantage_core::catalog::fetch().expect("the map catalog (needs the network)");
    catalog.save(&paths::catalog_path(&data_dir)).unwrap();
    let lotus = catalog
        .maps
        .iter()
        .find(|m| m.name == "Lotus")
        .cloned()
        .expect("Lotus in the catalog");
    let minimap = || vantage_core::minimap::Minimap::load(&data_dir, &lotus).map_err(|e| format!("{e:#}"));
    assert!(minimap().is_ok(), "Lotus's minimap (needs the network)");

    // Claude, at first with no key: the AI model list below is then the usual
    // one, with no request made. A made-up key is saved before each picture.
    let mut cfg = Config {
        rank: Some("Gold 2".into()),
        demos_dir: Some(demos.clone()),
        clips_dir: Some(clip_dir.clone()),
        focus: Some("untraded".into()),
        ai: Some("anthropic".into()),
        me: Some("8f3a9c21-5d7e-4b10-9a3c-1e2f4d5c6b7a".into()),
        ..Config::default()
    };
    let no_key = cfg.clone();
    cfg.ai_key = Some("sk-ant-example".into());
    cfg.save(&data_dir).unwrap();

    let mut shell = build(&data_dir);
    let mut pictures: Vec<(&str, Vec<Row>)> = Vec::new();

    launched(&mut shell);
    pictures.push(("shell-ready", screen(&mut shell)));
    shell.editor_mut().insert_str("/");
    pictures.push(("shell-commands", screen(&mut shell)));

    // /settings: Status, Config, and the AI model list.
    let (right, down, enter) = (key(KeyCode::Right), key(KeyCode::Down), key(KeyCode::Enter));
    for (name, keys) in [
        ("settings-status", vec![]),
        ("settings-config", vec![right]),
        ("settings-ai-model", vec![right, down, down, down, down, enter]),
    ] {
        launched(&mut shell);
        no_key.save(&data_dir).unwrap();
        let p = shell.open_settings().expect("settings");
        for k in keys {
            p.key(k);
        }
        let start = std::time::Instant::now();
        while p.busy() && start.elapsed() < std::time::Duration::from_secs(20) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // The panel reads the settings as it draws, so the row shows the key.
        cfg.save(&data_dir).unwrap();
        pictures.push((name, screen(&mut shell)));
    }

    // /review, one tab each.
    let all: Vec<_> = (0..6).map(match_review).collect();
    let review = |ai: Option<ai::Saved>| ReviewData {
        debrief: coach::debrief(&match_review(5), &[]),
        focus: habits::check(habits::focus(Some("untraded")), &all),
        focus_trend: habits::progress(&all).into_iter().find(|p| p.habit.slug == "untraded"),
        notes: vec![],
        map: Some(lotus.clone()),
        minimap: minimap(),
        clips: clips(),
        ai,
        ai_provider: ai::provider("anthropic"),
        ai_app: None,
    };
    for (name, tab, ai) in [
        ("review-debrief", '1', None),
        ("review-fixes", '2', None),
        ("review-map", '3', None),
        ("review-clips", '4', None),
        ("review-screenshots", '5', None),
        ("review-ai", '6', Some(saved())),
    ] {
        let mut p = ReviewPanel::new(review(ai));
        p.key(key(KeyCode::Char(tab)));
        shell.open(Panel::Review(p));
        pictures.push((name, screen(&mut shell)));
    }

    // /plan, /habits and /rank.
    shell.open(Panel::Plan(PlanPanel::new(PlanData {
        plan: plan::plan("Lotus", &all),
        rank: Some("Gold 2".into()),
        fix: Some(("First into A Rubble alone".into(), "Use an ability first.".into())),
        map: Some(lotus.clone()),
        minimap: minimap(),
    })));
    pictures.push(("plan", screen(&mut shell)));
    let h = HabitsPanel::new(habits::progress(&all), "untraded", all.len());
    shell.open(Panel::Habits(Box::new(h)));
    pictures.push(("habits", screen(&mut shell)));
    let s = standing::Standing {
        better: vec![
            "You do more damage than most players in your games.".into(),
            "You trade your teammates more often than most.".into(),
        ],
        behind: vec![
            "You die first in a round more often than almost anyone in your games.".into(),
            "You die with your ult ready more than most.".into(),
        ],
        games: 6,
    };
    shell.open(Panel::Rank(Box::new(RankPanel::new(
        "Gold 2".into(),
        s,
        "Dying where nobody can trade",
    ))));
    pictures.push(("rank", screen(&mut shell)));

    let frames = launch_and_review(&mut shell, review(Some(saved())));

    std::fs::create_dir_all(&out_dir).unwrap();
    for (name, rows) in &pictures {
        std::fs::write(out_dir.join(format!("{name}.svg")), svg(rows, &subs)).unwrap();
    }
    gif(&frames, &subs, &out_dir.join("launch.gif"));
    let _ = std::fs::remove_dir_all(&base);
    eprintln!("redrew {} pictures in {}", pictures.len(), out_dir.display());
}
