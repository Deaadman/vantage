//! The `/review` panel, drawn over the whole screen until Esc: the debrief
//! of one match in plain English. Tabs: Debrief, Fixes, Moments, Rounds,
//! Map, then Clips and Screenshots (your clip app's clips matched to your
//! deaths) and the optional AI review. Rounds count from 1, as in the game.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Color;
use valreview_core::ai::{self, Saved};
use valreview_core::analysis::Side;
use valreview_core::coach::{list_and, plural};
use valreview_core::habits::{self, Check};

use crate::panel::{
    Item, MapCache, bullet, field, frame, heading, is_close, list_rows, map_rows_h, pad, para, rule_line,
    scrolled, step, step_of, steps, tab_key,
};
use crate::shell::{ACCENT, GREY, RED, Row, SPINNER, Style, YELLOW, wrap};
use crate::{ClipFolder, Clips, ReviewData};

pub const GREEN: Color = Color::Rgb { r: 120, g: 200, b: 120 };
const TABS: [&str; 8] = ["Debrief", "Fixes", "Moments", "Rounds", "Map", "Clips", "Screenshots", "AI"];
const T_CLIPS: usize = 5;
const T_SHOTS: usize = 6;
const T_AI: usize = 7;
const LEGEND_W: usize = 44;

pub enum ReviewAction {
    None,
    Close,
    /// Use this clip folder, then open the review again.
    SetClips(PathBuf),
    /// Run the AI review of this match.
    RunAi,
}

pub struct ReviewPanel {
    data: Box<ReviewData>,
    tab: usize,
    sel: usize,
    scroll: usize,
    round: usize,
    death: usize,
    clip_pick: usize,
    /// The AI tab shows the format and the raw answer.
    json: bool,
    flash: Option<String>,
    /// While the AI review runs: who is asked, and the spinner frame.
    asking: Option<(String, usize)>,
    map_cache: MapCache,
}

impl ReviewPanel {
    pub fn new(data: ReviewData) -> Self {
        ReviewPanel {
            data: Box::new(data),
            tab: 0,
            sel: 0,
            scroll: 0,
            round: 0,
            death: 0,
            clip_pick: 0,
            json: false,
            flash: None,
            asking: None,
            map_cache: MapCache::default(),
        }
    }

    pub fn match_id(&self) -> &str {
        &self.data.debrief.match_id
    }

    pub fn data(&self) -> &ReviewData {
        &self.data
    }

    /// The shell is running the AI review: show the spinner.
    pub fn asking(&mut self, who: Option<String>, tick: usize) {
        self.asking = who.map(|w| (w, tick));
    }

    /// The AI review finished, or failed.
    pub fn ai_done(&mut self, result: Result<Saved, String>) {
        self.asking = None;
        match result {
            Ok(s) => {
                self.data.ai = Some(s);
                self.sel = 0;
                self.flash = None;
            }
            Err(e) => self.flash = Some(e),
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> ReviewAction {
        if is_close(&key) {
            return ReviewAction::Close;
        }
        if tab_key(&mut self.tab, &key, TABS.len()) {
            self.sel = 0;
            self.scroll = 0;
            self.flash = None;
            return ReviewAction::None;
        }
        let ch = match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => Some(c),
            _ => None,
        };
        let d = &self.data.debrief;
        if self.tab == T_CLIPS
            && let Clips::NotSet(folders) = &self.data.clips
        {
            match key.code {
                KeyCode::Up => self.clip_pick = self.clip_pick.saturating_sub(1),
                KeyCode::Down => self.clip_pick = (self.clip_pick + 1).min(folders.len().saturating_sub(1)),
                KeyCode::Enter => match folders.get(self.clip_pick).and_then(|f| f.path.clone()) {
                    Some(p) => return ReviewAction::SetClips(p),
                    None => self.flash = Some("Type the path in /settings → Config → Clip folder.".into()),
                },
                _ => {}
            }
            return ReviewAction::None;
        }
        if self.tab == T_SHOTS && ch == Some('o') {
            if let Clips::Set { shots_dir, .. } = &self.data.clips {
                let _ = std::fs::create_dir_all(shots_dir);
                self.flash = Some(match std::process::Command::new("explorer").arg(shots_dir).spawn() {
                    Ok(_) => format!("Opened {} in File Explorer.", shots_dir.display()),
                    Err(e) => format!("Couldn't open File Explorer: {e}"),
                });
            }
            return ReviewAction::None;
        }
        if self.tab == T_AI {
            if ch == Some('a') {
                return self.run_ai();
            }
            if ch == Some('j') && self.data.ai.is_some() {
                self.json = !self.json;
                self.scroll = 0;
                return ReviewAction::None;
            }
            if self.json {
                self.scroll = (self.scroll as isize + step_of(&key)).max(0) as usize;
                return ReviewAction::None;
            }
        }
        let n = match self.tab {
            0 => d.fixes.len().min(3),
            1 => d.fixes.len(),
            2 => d.moments.len(),
            3 => d.rounds.len(),
            4 | T_CLIPS => d.deaths.len(),
            T_AI => self.data.ai.as_ref().map_or(0, |a| a.output.findings.len()),
            _ => 0,
        };
        let by = step_of(&key);
        if by != 0 {
            match self.tab {
                3 => step(&mut self.round, by, n),
                4 => step(&mut self.death, by, n),
                _ => step(&mut self.sel, by, n),
            }
        } else if key.code == KeyCode::Enter {
            match self.tab {
                0 => self.tab = 1,
                1 => {
                    if let Some(r) = d.fixes.get(self.sel).and_then(|f| f.rounds.first()) {
                        self.round = d.rounds.iter().position(|x| x.round == *r).unwrap_or(0);
                        self.tab = 3;
                    }
                }
                2 => {
                    if let Some(m) = d.moments.get(self.sel) {
                        self.round = d.rounds.iter().position(|x| x.round == m.round).unwrap_or(0);
                        self.tab = 3;
                    }
                }
                3 => {
                    if let Some(i) = d.rounds.get(self.round).and_then(|r| r.death) {
                        self.death = i;
                        self.tab = 4;
                    }
                }
                T_CLIPS | T_SHOTS => {
                    self.tab += 1;
                    self.sel = 0;
                    self.flash = None;
                }
                _ => {}
            }
        }
        ReviewAction::None
    }

    fn run_ai(&mut self) -> ReviewAction {
        if self.asking.is_some() {
            return ReviewAction::None;
        }
        if self.data.ai_provider.is_none() {
            self.flash = Some("Connect an AI first, in /settings → Config → AI review.".into());
        } else if self.data.ai.is_some() {
            self.flash = Some("This match already has an AI review. It's saved and can't be rerun.".into());
        } else if self.data.shots().is_empty() {
            self.flash = Some("There are no screenshots to send yet. Set up the Clips tab first.".into());
        } else {
            return ReviewAction::RunAi;
        }
        ReviewAction::None
    }

    fn title(&self) -> Row {
        let d = &self.data.debrief;
        let (res, color) = result_style(d.won, d.lost);
        vec![
            (Style::fg(ACCENT).bold(), "  Review  ".into()),
            (Style::default().bold(), format!("{} on {} ", d.agent, d.map)),
            (Style::fg(GREY), "· ".into()),
            (Style::fg(color).bold(), format!("{res} {}–{}", d.won, d.lost)),
            (
                Style::fg(GREY),
                format!(
                    " · {}, {}{}",
                    plural(d.kills, "kill"),
                    plural(d.deaths_n, "death"),
                    d.played_at.as_deref().map(|t| format!(" · {}", crate::short_when(t))).unwrap_or_default()
                ),
            ),
        ]
    }

    /// The panel, exactly `h` rows for a `w`-column screen.
    pub fn rows(&mut self, w: usize, h: usize) -> Vec<Row> {
        let body_h = h.saturating_sub(7).max(1);
        let body = match self.tab {
            0 => self.debrief(w, body_h),
            1 => self.fixes(w, body_h),
            2 => self.moments(w, body_h),
            3 => self.rounds(w, body_h),
            4 => self.map(w, body_h),
            T_CLIPS => self.clips(w, body_h),
            T_SHOTS => self.shots(w, body_h),
            _ => self.ai_tab(w, body_h),
        };
        let clips_set = matches!(self.data.clips, Clips::Set { .. });
        let what = match self.tab {
            0 => "↑↓ select · Enter: the full fix",
            1 => "↑↓ select · Enter: its rounds",
            2 => "↑↓ select · Enter: the round",
            3 => "↑↓ select · Enter: on the map",
            4 => "↑↓ step through deaths",
            T_CLIPS if clips_set => "↑↓ select · Enter: the screenshots",
            T_CLIPS => "↑↓ choose · Enter picks · change it later in /settings",
            T_SHOTS => "o opens the folder · Enter: the AI review",
            _ if self.data.ai.is_some() && self.json => "↑↓ scroll · j: back to the review",
            _ if self.data.ai.is_some() => "↑↓ select · j: the format and saved answer",
            _ if self.data.ai_provider.is_some() => "a: run the AI review",
            _ => "connect an AI in /settings → Config",
        };
        frame(
            w,
            h,
            self.title(),
            Some((&TABS, self.tab)),
            body,
            &format!("←→ or 1-8 tabs · {what} · Esc closes"),
        )
    }

    fn flash_rows(&self, b: &mut Vec<Row>) {
        if let Some(f) = &self.flash {
            b.push(vec![]);
            b.push(vec![(Style::fg(YELLOW), format!("  ◆ {f}"))]);
        }
    }

    fn debrief(&mut self, w: usize, h: usize) -> Vec<Row> {
        let d = &self.data.debrief;
        let mut b: Vec<Row> = vec![heading("The short version", None)];
        b.extend(para(&d.summary, 4, w, Style::default()));
        b.push(vec![]);
        let top: Vec<_> = d.fixes.iter().take(3).collect();
        if top.is_empty() {
            b.push(heading("Nothing to fix first", Some(GREEN)));
            b.extend(para("No mistake repeated enough in this match to call it a pattern.", 4, w, Style::fg(GREY)));
        } else {
            b.push(vec![
                (Style::fg(YELLOW).bold(), "  Fix first".into()),
                (Style::fg(GREY), "  the three that cost you the most rounds".into()),
            ]);
            self.sel = self.sel.min(top.len() - 1);
            b.extend(list_rows(
                top.iter()
                    .enumerate()
                    .map(|(i, f)| Item::new(format!("{}  {} ", i + 1, pad(&f.title, 50))).tail(&f.cost, GREY))
                    .collect(),
                self.sel,
                top.len(),
            ));
            let f = top[self.sel];
            b.push(vec![]);
            b.push(rule_line(w));
            b.push(vec![(Style::default().bold(), format!("    {}", f.title))]);
            b.extend(para(&f.short, 4, w, Style::default()));
            for (i, p) in wrap(&f.try_short, w.saturating_sub(12).max(10)).into_iter().enumerate() {
                b.push(vec![
                    (Style::fg(GREEN).bold(), if i == 0 { "    Try  ".into() } else { "         ".into() }),
                    (Style::default(), p),
                ]);
            }
        }
        if !d.keep.is_empty() {
            b.push(vec![]);
            b.push(heading("Keep doing", Some(GREEN)));
            for k in &d.keep {
                b.extend(bullet("+", GREEN, k, w));
            }
        }
        if let Some(c) = &self.data.focus {
            b.push(vec![]);
            b.push(vec![
                (Style::fg(YELLOW), "  ◆ ".into()),
                (Style::default(), format!("Your focus, {}: ", c.habit.name)),
                (Style::default().bold(), focus_result(c)),
            ]);
        }
        for n in &self.data.notes {
            b.push(vec![]);
            b.extend(para(&format!("Note: {n}"), 4, w, Style::fg(GREY)));
        }
        scrolled(&mut self.scroll, b, h)
    }

    fn fixes(&mut self, w: usize, h: usize) -> Vec<Row> {
        let d = &self.data.debrief;
        if d.fixes.is_empty() {
            return para("Nothing repeated enough in this match to make a fix of it. See the Moments tab for single rounds.", 2, w, Style::fg(GREY));
        }
        self.sel = self.sel.min(d.fixes.len() - 1);
        let mut b = list_rows(
            d.fixes
                .iter()
                .enumerate()
                .map(|(i, f)| Item::new(format!("{}  {} ", i + 1, pad(&f.title, 50))).tail(&f.cost, GREY))
                .collect(),
            self.sel,
            d.fixes.len().min(8),
        );
        let f = &d.fixes[self.sel];
        b.push(vec![]);
        b.push(rule_line(w));
        b.push(vec![(Style::default().bold(), format!("    {}", f.title))]);
        b.extend(field("What happened", &f.happened, w, Style::default()));
        b.extend(field("Why it costs", &f.why, w, Style::default()));
        b.extend(steps("Try this", &f.tries, w));
        b.extend(field("Next match", &f.check, w, Style::fg(ACCENT)));
        b.truncate(h);
        b
    }

    fn moments(&mut self, w: usize, h: usize) -> Vec<Row> {
        let d = &self.data.debrief;
        let mut b: Vec<Row> = vec![
            vec![(
                Style::fg(GREY),
                "  The deaths that decided a round, and what would have been better".into(),
            )],
            vec![],
        ];
        if d.moments.is_empty() {
            b.extend(para("None of your deaths in lost rounds fits a pattern worth calling out.", 4, w, Style::fg(GREY)));
            return b;
        }
        self.sel = self.sel.min(d.moments.len() - 1);
        b.extend(list_rows(
            d.moments
                .iter()
                .map(|m| {
                    Item::new(format!("{}{} ", pad(&format!("R{}", m.round), 5), pad(&m.situation, 38)))
                        .tail(format!("→ {}", m.better), GREY)
                })
                .collect(),
            self.sel,
            d.moments.len(),
        ));
        let m = &d.moments[self.sel];
        b.push(vec![]);
        b.push(rule_line(w));
        b.push(vec![(Style::default().bold(), format!("    Round {}: {}", m.round, m.situation))]);
        b.extend(para(&m.text, 4, w, Style::default()));
        b.truncate(h);
        b
    }

    fn rounds(&mut self, w: usize, h: usize) -> Vec<Row> {
        let d = &self.data.debrief;
        if d.rounds.is_empty() {
            return vec![vec![(Style::fg(GREY), "  No rounds were stored for this match.".into())]];
        }
        self.round = self.round.min(d.rounds.len() - 1);
        let items: Vec<Item> = d
            .rounds
            .iter()
            .map(|x| {
                let death = x.death.map(|i| &d.deaths[i]);
                let kills = if x.kills > 0 { plural(x.kills as usize, "kill") } else { String::new() };
                let what = match death {
                    Some(dd) => format!(
                        "{}{} got you at {}",
                        if kills.is_empty() { String::new() } else { format!("{kills}, then ") },
                        dd.killer,
                        dd.place.as_deref().unwrap_or("?")
                    ),
                    None if x.kills > 0 => format!("{kills}, survived"),
                    None => "survived".into(),
                };
                let tag = match death {
                    Some(dd) if dd.fix.is_some() => format!("fix {}", dd.fix.unwrap_or(0) + 1),
                    Some(dd) if dd.moment.is_some() => "moment".into(),
                    _ => String::new(),
                };
                Item::new(format!(
                    "{:>2}  {}{} {} {} ",
                    x.round,
                    pad(side_word(x.side), 8),
                    pad(won_word(x.won), 5),
                    pad(&x.buy, 9),
                    pad(&what, 44)
                ))
                .tail(tag, YELLOW)
            })
            .collect();
        let rd = &d.rounds[self.round];
        let mut detail: Vec<Row> = vec![
            rule_line(w),
            vec![(
                Style::default().bold(),
                format!("    Round {} · {} · {}", rd.round, side_word(rd.side), won_word(rd.won)),
            )],
        ];
        let buy = match rd.buy.as_str() {
            "save" => "a save".to_owned(),
            "pistol" => "pistols".to_owned(),
            "eco" => "an eco".to_owned(),
            b => format!("a {b}"),
        };
        let mut story = vec![format!("You were on {} with {buy}.", side_word(rd.side))];
        if rd.kills > 0 {
            story.push(format!("You got {}.", plural(rd.kills as usize, "kill")));
        }
        let death = rd.death.map(|i| &d.deaths[i]);
        story.push(death.map_or_else(|| "You survived the round.".into(), |dd| dd.sentence()));
        detail.extend(para(&story.join(" "), 4, w, Style::default()));
        if let Some(fi) = death.and_then(|dd| dd.fix) {
            detail.extend(para(&format!("Part of fix {}: {}.", fi + 1, d.fixes[fi].title), 4, w, Style::fg(YELLOW)));
        }
        if let Some(mi) = death.and_then(|dd| dd.moment) {
            detail.extend(para(&format!("Better: {}.", d.moments[mi].better), 4, w, Style::fg(GREEN)));
        }
        let list_h = h.saturating_sub(detail.len() + 2).min(d.rounds.len()).max(3);
        let mut b = vec![vec![(Style::fg(GREY), "    rnd  side    result buy       what happened".to_owned())]];
        b.extend(list_rows(items, self.round, list_h));
        b.push(vec![]);
        b.extend(detail);
        b.truncate(h);
        b
    }

    fn map(&mut self, w: usize, h: usize) -> Vec<Row> {
        let rows_h = map_rows_h(w, h, LEGEND_W);
        let data = &self.data;
        let d = &data.debrief;
        let mut cells = match self.map_cache.cells(&data.minimap, data.map.as_ref(), rows_h) {
            Ok(c) => c,
            Err(e) => return vec![vec![(Style::fg(GREY), format!("  No minimap: {e}"))]],
        };
        for uv in &d.wins {
            cells.mark(*uv, GREEN, "▲");
        }
        for (i, x) in d.deaths.iter().enumerate() {
            if i != self.death
                && let Some(uv) = x.uv
            {
                cells.mark(uv, RED, "●");
            }
        }
        let sel = d.deaths.get(self.death);
        if let Some(uv) = sel.and_then(|x| x.uv) {
            cells.mark(uv, YELLOW, "◆");
        }
        let mut legend: Vec<Row> = vec![
            vec![(Style::fg(RED).bold(), "● ".into()), (Style::default(), "where you died".into())],
            vec![(Style::fg(GREEN).bold(), "▲ ".into()), (Style::default(), "where you won a fight".into())],
            vec![(Style::fg(YELLOW).bold(), "◆ ".into()), (Style::default(), "this death (↑↓)".into())],
            vec![],
            vec![
                (Style::fg(GREEN).bold(), "Your spot: ".into()),
                (Style::default(), d.best_spot.clone().unwrap_or_else(|| "none yet".into())),
            ],
            vec![
                (Style::fg(RED).bold(), "Your trap: ".into()),
                (
                    Style::default(),
                    d.worst_spot
                        .as_ref()
                        .map_or_else(|| "none".into(), |(s, n)| format!("{s} ({})", plural(*n, "death"))),
                ),
            ],
            vec![],
        ];
        if let Some(x) = sel {
            legend.push(vec![(
                Style::default().bold(),
                format!("Death {} of {} · round {}", self.death + 1, d.deaths.len(), x.round),
            )]);
            for p in wrap(&x.sentence(), 40) {
                legend.push(vec![(Style::fg(GREY), p)]);
            }
        }
        cells.beside(legend)
    }

    fn clips(&mut self, w: usize, h: usize) -> Vec<Row> {
        let d = &self.data.debrief;
        let mut b: Vec<Row> = Vec::new();
        match &self.data.clips {
            Clips::NotSet(folders) => {
                b.push(heading("Where do your clips save?", None));
                b.extend(para(
                    "Val Review finds the clip of each death by when the clip was saved, and takes screenshots from the seconds before you died. The AI tab uses them to judge where you stood.",
                    4,
                    w,
                    Style::fg(GREY),
                ));
                b.push(vec![]);
                let name_w = folders.iter().map(|f| f.name.chars().count() + 2).max().unwrap_or(0).max(24);
                for (i, f) in folders.iter().enumerate() {
                    let on = i == self.clip_pick;
                    b.push(vec![
                        (
                            if on { Style::fg(ACCENT) } else { Style::default() },
                            format!("{}{}", if on { "  ❯ " } else { "    " }, pad(&f.name, name_w)),
                        ),
                        (f.about_style(), f.about.clone()),
                    ]);
                }
                b.push(vec![]);
                b.push(heading("To get a clip for every death", None));
                for t in [
                    "Turn on your clip app's instant replay, so it keeps the last 30 seconds.",
                    "Press its clip key right after you die. Clips of 15 to 30 seconds work best.",
                    "Download the match replay in VALORANT too: it tells Val Review which clip is which death.",
                ] {
                    b.extend(bullet("•", ACCENT, t, w));
                }
                self.flash_rows(&mut b);
            }
            Clips::Set { dir, per_death, .. } => {
                let have = per_death.iter().filter(|c| c.is_some()).count();
                b.push(vec![
                    (Style::default(), "  ".into()),
                    (Style::default().bold(), format!("{have} of {} deaths have a clip", d.deaths.len())),
                    (Style::fg(GREY), format!(" · from {}", dir.display())),
                ]);
                b.push(vec![]);
                if d.deaths.is_empty() {
                    b.push(vec![(Style::fg(GREY), "    You didn't die this match.".into())]);
                    return b;
                }
                self.sel = self.sel.min(d.deaths.len() - 1);
                let items = d
                    .deaths
                    .iter()
                    .zip(per_death)
                    .map(|(x, c)| {
                        let what = format!("{} at {}", x.killer, x.place.as_deref().unwrap_or("?"));
                        let it = Item::new(format!("{}{}", pad(&format!("R{}", x.round), 5), pad(&what, 26)));
                        match c {
                            Some(m) => it.tail(
                                format!("✓ {}", m.path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()),
                                GREEN,
                            ),
                            None => it.tail("no clip", GREY),
                        }
                    })
                    .collect();
                b.extend(list_rows(items, self.sel, h.saturating_sub(6).max(3)));
                let x = &d.deaths[self.sel];
                let text = match &per_death[self.sel] {
                    Some(m) => format!(
                        "Round {}: {} The clip was saved {:.0} seconds after this death.",
                        x.round,
                        x.sentence(),
                        m.before_end_s
                    ),
                    None => format!(
                        "Round {}: no clip was saved within {:.0} seconds of this death, so it's left out.",
                        x.round,
                        valreview_core::clips::MATCH_WINDOW_S
                    ),
                };
                b.push(vec![]);
                b.extend(para(&text, 4, w, Style::fg(GREY)));
            }
        }
        b.truncate(h);
        b
    }

    fn shots(&mut self, w: usize, h: usize) -> Vec<Row> {
        let d = &self.data.debrief;
        let Clips::Set {
            shots_dir,
            shots,
            shot_error,
            style,
            ..
        } = &self.data.clips
        else {
            return para(
                "Pick your clip folder in the Clips tab first; the screenshots are taken from your clips.",
                2,
                w,
                Style::fg(YELLOW),
            );
        };
        let mut b: Vec<Row> = Vec::new();
        if shots.is_empty() {
            b.extend(para(
                shot_error.as_deref().unwrap_or("None of your deaths has a clip, so there's nothing to take screenshots from."),
                2,
                w,
                Style::fg(YELLOW),
            ));
        } else {
            b.push(vec![
                (Style::default(), "  ".into()),
                (Style::fg(GREEN).bold(), "✓ ".into()),
                (Style::default().bold(), format!("{} ready", plural(shots.len(), "screenshot"))),
                (Style::fg(GREY), ", one for each death with a clip".into()),
            ]);
            b.push(vec![]);
            let how = match style {
                valreview_core::clips::ShotStyle::Strip => "Each screenshot puts three moments side by side: 3 seconds before you died, 1 second before, and the moment you died, so the whole fight is in one image.",
                valreview_core::clips::ShotStyle::One => "Each screenshot is the moment you died.",
                valreview_core::clips::ShotStyle::Three => "Each death has three screenshots: 3 seconds before you died, 1 second before, and the moment you died.",
            };
            b.extend(para(how, 2, w, Style::fg(GREY)));
            b.push(vec![]);
            b.push(vec![
                (Style::fg(GREY), "  Saved to  ".into()),
                (Style::default(), shots_dir.display().to_string()),
            ]);
            b.push(vec![]);
            let room = h.saturating_sub(12).max(3);
            for (round, files) in shots.iter().take(room) {
                let x = d.deaths.iter().find(|x| x.round == *round);
                let names = files
                    .iter()
                    .filter_map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .collect::<Vec<_>>()
                    .join(", ");
                b.push(vec![
                    (Style::default(), format!("    {} ", pad(&names, 10))),
                    (
                        Style::fg(GREY),
                        x.map(|x| format!("{} at {}", x.killer, x.place.as_deref().unwrap_or("?"))).unwrap_or_default(),
                    ),
                ]);
            }
            if shots.len() > room {
                b.push(vec![(Style::fg(GREY), format!("    …and {} more", shots.len() - room))]);
            }
            if let Some(e) = shot_error {
                b.push(vec![]);
                b.extend(para(e, 2, w, Style::fg(YELLOW)));
            }
            b.push(vec![]);
            b.extend(para(
                if self.data.ai_provider.is_some() {
                    "The AI tab sends these with the match when you run the AI review."
                } else {
                    "Connect an AI in /settings and the AI tab can review these for you."
                },
                2,
                w,
                Style::fg(ACCENT),
            ));
        }
        self.flash_rows(&mut b);
        b.truncate(h);
        b
    }

    fn ai_tab(&mut self, w: usize, h: usize) -> Vec<Row> {
        let shots = self.data.shots().len();
        let provider = self.data.ai_provider;
        if let Some((who, tick)) = &self.asking {
            let mut b = vec![
                vec![
                    (Style::fg(RED), format!("  {} ", SPINNER[tick / 2 % SPINNER.len()])),
                    (Style::fg(ACCENT), format!("Asking {who}…")),
                ],
                vec![],
            ];
            b.extend(para(
                &format!(
                    "Sent {who} this match's facts and {}. The answer is checked against the AI review format before it's saved.",
                    plural(shots, "screenshot")
                ),
                4,
                w,
                Style::fg(GREY),
            ));
            return b;
        }
        if let Some(saved) = self.data.ai.clone() {
            return if self.json { self.ai_json(&saved, w, h) } else { self.ai_result(&saved, w, h) };
        }
        let mut b = vec![heading("AI review", None)];
        b.extend(para(
            "An AI looks at the screenshots of your deaths alongside what Val Review already knows about the match, and judges what the replay alone can't show: where you stood, which angles could see you, and where your crosshair was before the fight.",
            4,
            w,
            Style::fg(GREY),
        ));
        b.push(vec![]);
        match provider {
            None => {
                b.extend(bullet(
                    "•",
                    ACCENT,
                    "Connect Claude, ChatGPT, Gemini, or Ollama on this PC in /settings → Config → AI review.",
                    w,
                ));
                b.extend(bullet(
                    "•",
                    ACCENT,
                    "Claude, ChatGPT and Gemini use your own API key, and the provider bills you for each review. Ollama runs a model on this PC instead: free, and nothing leaves the computer, but it needs a good graphics card.",
                    w,
                ));
                b.extend(bullet(
                    "•",
                    ACCENT,
                    "Everything else in this review works without it. This tab only adds to it.",
                    w,
                ));
            }
            Some(p) => {
                let what = if shots > 0 {
                    format!("this match's facts and {} from the Screenshots tab", plural(shots, "screenshot"))
                } else {
                    "this match's facts and no screenshots (pick your clip folder in the Clips tab first)".to_owned()
                };
                if p.local {
                    b.extend(field("Gives", &format!("Ollama, on this PC: {what}. Nothing leaves this computer."), w, Style::default()));
                } else {
                    b.extend(field("Sends", &format!("{}: {what}", p.name), w, Style::default()));
                }
                b.extend(field(
                    "Answers in",
                    &format!("the same format every time ({}), so Val Review can read it", ai::SCHEMA),
                    w,
                    Style::default(),
                ));
                b.extend(field(
                    "Saved",
                    "once for this match. It can't be rerun, so every match is judged the same way, once.",
                    w,
                    Style::default(),
                ));
                b.push(vec![]);
                b.push(vec![(Style::fg(GREEN).bold(), "    Press a to run it".into())]);
            }
        }
        self.flash_rows(&mut b);
        b.truncate(h);
        b
    }

    fn ai_result(&mut self, s: &Saved, w: usize, h: usize) -> Vec<Row> {
        let o = &s.output;
        let saved = crate::short_when(&s.meta.saved_at);
        let mut b: Vec<Row> = vec![vec![
            (Style::fg(ACCENT).bold(), "  AI review".into()),
            (Style::fg(GREY), format!(" · {} · saved {saved} · can't be rerun", s.meta.provider)),
        ]];
        b.extend(para(&o.summary, 4, w, Style::default()));
        b.push(vec![]);
        if let Some(f) = &self.flash {
            b.push(vec![(Style::fg(YELLOW), format!("  ◆ {f}"))]);
            b.push(vec![]);
        }
        b.push(vec![
            (Style::fg(YELLOW).bold(), "  What it saw".into()),
            (Style::fg(GREY), "  in the screenshots, most costly first".into()),
        ]);
        self.sel = self.sel.min(o.findings.len().saturating_sub(1));
        b.extend(list_rows(
            o.findings
                .iter()
                .map(|f| {
                    Item::new(format!("{} ", pad(&f.title, 50)))
                        .tail(format!("{} · {}", ai::category_label(&f.category), f.confidence), GREY)
                })
                .collect(),
            self.sel,
            o.findings.len(),
        ));
        if let Some(f) = o.findings.get(self.sel) {
            b.push(vec![]);
            b.push(rule_line(w));
            b.push(vec![(Style::default().bold(), format!("    {}", f.title))]);
            let rounds: Vec<String> = f.rounds.iter().map(u32::to_string).collect();
            b.extend(field("Rounds", &format!("{} · {}", list_and(&rounds), f.screenshots.join(", ")), w, Style::default()));
            b.extend(field("What it saw", &f.saw, w, Style::default()));
            b.extend(field("Why it costs", &f.why, w, Style::default()));
            b.extend(field("Try", &f.try_this, w, Style::fg(GREEN)));
        }
        if !o.keep.is_empty() {
            b.push(vec![]);
            b.push(heading("Keep doing", Some(GREEN)));
            for k in &o.keep {
                b.extend(bullet("+", GREEN, &format!("{}. {}", k.title, k.saw), w));
            }
        }
        if !o.unclear.is_empty() {
            b.push(vec![]);
            b.push(heading("Couldn't tell", Some(GREY)));
            for u in &o.unclear {
                b.extend(bullet("?", GREY, &format!("{}: {}", u.screenshot, u.reason), w));
            }
        }
        b.truncate(h);
        b
    }

    fn ai_json(&mut self, s: &Saved, w: usize, h: usize) -> Vec<Row> {
        let mut lines = vec![heading("The format", None)];
        lines.extend(para(
            &format!(
                "Every answer has to fit {}. The provider is asked for exactly this shape, and an answer that doesn't fit is rejected and asked for again.",
                ai::SCHEMA
            ),
            4,
            w,
            Style::fg(GREY),
        ));
        lines.push(vec![]);
        for (name, desc) in ai::FORMAT {
            for (i, p) in wrap(desc, w.saturating_sub(26).max(10)).into_iter().enumerate() {
                lines.push(vec![
                    (Style::fg(ACCENT), format!("    {}", pad(if i == 0 { name } else { "" }, 18))),
                    (Style::default(), p),
                ]);
            }
        }
        let cats = ai::CATEGORIES.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ");
        for (i, p) in wrap(&cats, w.saturating_sub(26).max(10)).into_iter().enumerate() {
            lines.push(vec![
                (Style::fg(ACCENT), format!("    {}", pad(if i == 0 { "categories" } else { "" }, 18))),
                (Style::default(), p),
            ]);
        }
        lines.push(vec![]);
        lines.push(heading("This match's saved answer", None));
        lines.push(vec![]);
        let json = serde_json::to_string_pretty(&s.output).unwrap_or_default();
        for l in json.lines() {
            for p in wrap(l, w.saturating_sub(10).max(10)) {
                lines.push(vec![(Style::fg(GREY), "  │ ".into()), (Style::default(), p)]);
            }
        }
        scrolled(&mut self.scroll, lines, h)
    }
}

impl ClipFolder {
    fn about_style(&self) -> Style {
        match self.found {
            Some(0) => Style::fg(GREY),
            Some(_) => Style::fg(GREEN),
            None if self.path.is_none() => Style::fg(GREY),
            None => Style::default(),
        }
    }
}

/// `2 times this match. Better than before.`
pub fn focus_result(c: &Check) -> String {
    let Some(n) = c.this_match else {
        return "this match couldn't tell.".into();
    };
    let this = match c.habit.slug {
        "moving" => format!("{} on the move this match", habits::describe("moving", n)),
        "crosshair" => format!("{} this match", habits::describe("crosshair", n)),
        _ if n == 0.0 => "not once this match".to_owned(),
        _ => format!("{} this match", plural(n as usize, "time")),
    };
    format!("{this}. {}", c.verdict())
}

pub fn result_style(won: usize, lost: usize) -> (&'static str, Color) {
    match valreview_core::coach::result_word(won, lost) {
        "won" => ("won", GREEN),
        "lost" => ("lost", RED),
        _ => ("drew", YELLOW),
    }
}

fn side_word(s: Option<Side>) -> &'static str {
    match s {
        Some(Side::Attack) => "attack",
        Some(Side::Defense) => "defense",
        None => "?",
    }
}

fn won_word(w: Option<bool>) -> &'static str {
    match w {
        Some(true) => "won",
        Some(false) => "lost",
        None => "?",
    }
}
