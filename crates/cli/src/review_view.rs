//! The `/review` panel, drawn over the whole screen until Esc: the debrief
//! of one match in plain English. Tabs: Debrief, Fixes, Map, then Clips and
//! Screenshots (your clip app's clips matched to your deaths) and the
//! optional AI review. Rounds count from 1, as in the game.

use std::path::PathBuf;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Color;
use vantage_core::ai::{self, Saved};
use vantage_core::coach::{list_and, plural};
use vantage_core::habits::{self, Check};

use crate::coach_views::trend_color;
use crate::panel::{
    Item, MapCache, bullet, field, fit, frame, heading, is_close, list_h, list_rows, map_rows_h, pad, para, rule_line,
    scrolled, step, step_of, steps, tab_key,
};
use crate::shell::{ACCENT, GREY, RED, Row, Style, YELLOW, spike, wrap};
use crate::{ClipFolder, Clips, ReviewData};
use abalone::row_width;

pub const GREEN: Color = Color::Rgb { r: 120, g: 200, b: 120 };
const TABS: [&str; 6] = ["Debrief", "Fixes", "Map", "Clips", "Screenshots", "AI"];
const T_FIXES: usize = 1;
const T_MAP: usize = 2;
const T_CLIPS: usize = 3;
const T_SHOTS: usize = 4;
const T_AI: usize = 5;
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
    death: usize,
    clip_pick: usize,
    /// The AI tab shows the format and the raw answer.
    json: bool,
    flash: Option<String>,
    /// While the AI review runs: who is asked, and since when.
    asking: Option<(String, Instant)>,
    /// While an AI app reviews this match through MCP: the app, and since when.
    reviewing: Option<(String, Instant)>,
    map_cache: MapCache,
}

impl ReviewPanel {
    pub fn new(data: ReviewData) -> Self {
        ReviewPanel {
            data: Box::new(data),
            tab: 0,
            sel: 0,
            scroll: 0,
            death: 0,
            clip_pick: 0,
            json: false,
            flash: None,
            asking: None,
            reviewing: None,
            map_cache: MapCache::default(),
        }
    }

    pub fn match_id(&self) -> &str {
        &self.data.debrief.match_id
    }

    /// The same review, reloaded, on the tab this one was showing.
    pub fn reloaded(&self, data: ReviewData) -> Self {
        let mut p = ReviewPanel::new(data);
        p.tab = self.tab;
        p
    }

    /// A line of news at the bottom of the panel.
    pub fn note(&mut self, text: String) {
        self.flash = Some(text);
    }

    pub fn data(&self) -> &ReviewData {
        &self.data
    }

    /// The shell is running the AI review: show the spinner.
    pub fn asking(&mut self, who: Option<(String, Instant)>) {
        self.asking = who;
    }

    /// Waiting on an AI app's review, so the spinner keeps turning.
    pub fn waiting(&self) -> bool {
        self.reviewing.is_some() && self.data.ai.is_none()
    }

    /// An AI app is reviewing this match: show the spinner.
    pub fn reviewing(&mut self, who: Option<(String, Instant)>) {
        self.reviewing = who;
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
            self.open(self.tab);
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
                    Some(p) if !p.is_dir() => self.flash = Some(format!("There's no folder at {}", p.display())),
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
            if is_run(&key) {
                return self.run_ai();
            }
            if ch == Some('c')
                && self.data.ai.is_none()
                && let Some(app) = self.data.ai_app
            {
                let line = crate::mcp::ask_line(&self.data.debrief);
                self.flash = Some(match crate::mcp::copy(&line) {
                    Ok(()) => format!("Copied. Paste it into {}.", app_title(app)),
                    Err(e) => format!("{e:#}"),
                });
                return ReviewAction::None;
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
            T_FIXES => d.fixes.len(),
            T_MAP | T_CLIPS => d.deaths.len(),
            T_AI => self.data.ai.as_ref().map_or(0, |a| a.output.findings.len()),
            _ => 0,
        };
        let by = step_of(&key);
        if by != 0 {
            match self.tab {
                0 => self.scroll = (self.scroll as isize + by).max(0) as usize,
                T_MAP => step(&mut self.death, by, n),
                _ => step(&mut self.sel, by, n),
            }
        } else if key.code == KeyCode::Enter {
            match self.tab {
                0 => self.open(T_FIXES),
                T_FIXES => {
                    // The fix's first death, on the map.
                    if let Some(i) = d
                        .fixes
                        .get(self.sel)
                        .and_then(|f| d.deaths.iter().position(|x| f.rounds.contains(&x.round)))
                    {
                        self.open(T_MAP);
                        self.death = i;
                    }
                }
                T_CLIPS | T_SHOTS => self.open(self.tab + 1),
                _ => {}
            }
        }
        ReviewAction::None
    }

    /// Switches to `tab`, from its top.
    fn open(&mut self, tab: usize) {
        self.tab = tab;
        self.sel = 0;
        self.scroll = 0;
        self.flash = None;
    }

    fn run_ai(&mut self) -> ReviewAction {
        if self.asking.is_some() {
            return ReviewAction::None;
        }
        if self.data.ai.is_some() {
            self.flash = Some("This match already has an AI review. It's saved and can't be rerun.".into());
        } else if let Some(app) = self.data.ai_app {
            self.flash = Some(format!("Ask {} instead: press c to copy what to ask.", app.name));
        } else if self.data.ai_provider.is_none() {
            self.flash = Some("Connect an AI first, in /settings → Config → AI review.".into());
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
                    d.played_at
                        .as_deref()
                        .map(|t| format!(" · {}", crate::short_when(t)))
                        .unwrap_or_default()
                ),
            ),
        ]
    }

    /// The panel, exactly `h` rows for a `w`-column screen.
    pub fn rows(&mut self, w: usize, h: usize) -> Vec<Row> {
        let body_h = h.saturating_sub(7).max(1);
        let body = match self.tab {
            0 => self.debrief(w, body_h),
            T_FIXES => self.fixes(w, body_h),
            T_MAP => self.map(w, body_h),
            T_CLIPS => self.clips(w, body_h),
            T_SHOTS => self.shots(w, body_h),
            _ => self.ai_tab(w, body_h),
        };
        let clips_set = matches!(self.data.clips, Clips::Set { .. });
        let what = match self.tab {
            0 => "↑↓ scroll · Enter: the fixes · ",
            T_FIXES => "↑↓ select · Enter: on the map · ",
            T_MAP => "↑↓ step through deaths · ",
            T_CLIPS if clips_set => "↑↓ select · Enter: the screenshots · ",
            T_CLIPS => "↑↓ choose · Enter picks · change it later in /settings · ",
            T_SHOTS => "o opens the folder · Enter: the AI review · ",
            _ if self.data.ai.is_some() && self.json => "↑↓ scroll · j: back to the review · ",
            _ if self.data.ai.is_some() => "↑↓ select · j: the format and saved answer · ",
            _ if self.data.ai_provider.is_some() => "Ctrl+Enter: run the AI review · ",
            _ if self.data.ai_app.is_some() && self.reviewing.is_none() => "c: copy what to ask · ",
            _ if self.data.ai_app.is_some() => "",
            _ => "connect an AI in /settings → Config · ",
        };
        frame(
            w,
            h,
            self.title(),
            Some((&TABS, self.tab)),
            body,
            &format!("←→ or 1-{} tabs · {what}Esc closes", TABS.len()),
        )
    }

    fn flash_rows(&self, b: &mut Vec<Row>, w: usize) {
        if let Some(f) = &self.flash {
            b.push(vec![]);
            b.extend(notice(f, w));
        }
    }

    fn debrief(&mut self, w: usize, h: usize) -> Vec<Row> {
        let d = &self.data.debrief;
        let mut b: Vec<Row> = vec![heading("The short version", None)];
        b.extend(para(&d.summary, 4, w, Style::default()));
        b.push(vec![]);
        let top: Vec<_> = d.fixes.iter().take(3).collect();
        if top.is_empty() {
            b.push(heading("No issues found", Some(GREEN)));
            b.extend(para(
                "No mistake repeated enough in this match to call it a pattern.",
                4,
                w,
                Style::fg(GREY),
            ));
        } else {
            let how_many = ["the one", "the two", "the three"][top.len() - 1];
            b.push(vec![
                (Style::fg(YELLOW).bold(), "  Issues found".into()),
                (Style::fg(GREY), format!(" · {how_many} that cost you the most rounds")),
            ]);
            for (i, f) in top.iter().enumerate() {
                b.push(vec![
                    (Style::default(), format!("    {}  {} ", i + 1, pad(&f.title, 50))),
                    (Style::fg(GREY), f.cost.clone()),
                ]);
            }
            b.push(vec![(
                Style::fg(GREY),
                "    To see more details, go to the Fixes tab.".into(),
            )]);
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
            let mut row: Row = vec![
                (Style::fg(YELLOW), "  ◆ ".into()),
                (Style::default(), format!("Your focus, {}: ", c.habit.name)),
                (Style::default().bold(), focus_result(c)),
            ];
            let mut trend: Row = Vec::new();
            if let Some(p) = self.data.focus_trend.as_ref().filter(|p| !p.values.is_empty()) {
                let c = trend_color(p.trend);
                trend.push((Style::fg(c), format!("  {}", habits::spark(&p.values))));
                trend.push((Style::fg(c).bold(), format!("  {}", p.trend.lately())));
            }
            // The trend goes on its own line when both don't fit on one.
            if row_width(&row) + row_width(&trend) < w {
                row.extend(trend);
                b.push(row);
            } else {
                b.push(row);
                if !trend.is_empty() {
                    let mut next: Row = vec![(Style::default(), "  ".into())];
                    next.extend(trend);
                    b.push(next);
                }
            }
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
            return para(
                "Nothing repeated enough in this match to make a fix of it.",
                2,
                w,
                Style::fg(GREY),
            );
        }
        self.sel = self.sel.min(d.fixes.len() - 1);
        let f = &d.fixes[self.sel];
        let mut detail: Vec<Row> = vec![vec![], rule_line(w), vec![]];
        detail.extend(field("What happened", &f.happened, w, Style::default()));
        detail.push(vec![]);
        detail.extend(field("Why it costs", &f.why, w, Style::default()));
        detail.push(vec![]);
        detail.extend(steps("Try this", &f.tries, w));
        detail.push(vec![]);
        detail.extend(field("Next match", &f.check, w, Style::fg(ACCENT)));
        // The list shrinks, scrolling with the selection, so the fix's detail fits.
        let mut b = list_rows(
            d.fixes
                .iter()
                .enumerate()
                .map(|(i, f)| Item::new(format!("{}  {} ", i + 1, pad(&f.title, 50))).tail(&f.cost, GREY))
                .collect(),
            self.sel,
            list_h(d.fixes.len(), 8, detail.len(), h),
        );
        b.extend(detail);
        fit(b, h)
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
            vec![
                (Style::fg(RED).bold(), "● ".into()),
                (Style::default(), "where you died".into()),
            ],
            vec![
                (Style::fg(GREEN).bold(), "▲ ".into()),
                (Style::default(), "where you won a fight".into()),
            ],
            vec![
                (Style::fg(YELLOW).bold(), "◆ ".into()),
                (Style::default(), "this death (↑↓)".into()),
            ],
            vec![],
            vec![
                (Style::fg(GREEN).bold(), "Your spot: ".into()),
                (
                    Style::default(),
                    d.best_spot.clone().unwrap_or_else(|| "none yet".into()),
                ),
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
        fit(cells.beside(legend), h)
    }

    fn clips(&mut self, w: usize, h: usize) -> Vec<Row> {
        let d = &self.data.debrief;
        let mut b: Vec<Row> = Vec::new();
        match &self.data.clips {
            Clips::NotSet(folders) => {
                b.push(heading("Where do your clips save?", None));
                b.extend(para(
                    "Vantage finds the clip of each death by when the clip was saved, and takes screenshots from the seconds before you died. The AI tab uses them to judge where you stood.",
                    4,
                    w,
                    Style::fg(GREY),
                ));
                b.push(vec![]);
                let name_w = folders
                    .iter()
                    .map(|f| f.name.chars().count() + 2)
                    .max()
                    .unwrap_or(0)
                    .max(24);
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
                    "Download the match replay in VALORANT too: it tells Vantage which clip is which death.",
                ] {
                    b.extend(bullet("•", ACCENT, t, w));
                }
                self.flash_rows(&mut b, w);
            }
            Clips::Set { dir, per_death, .. } => {
                let have = per_death.iter().filter(|c| c.is_some()).count();
                b.push(vec![
                    (Style::default(), "  ".into()),
                    (
                        Style::default().bold(),
                        format!("{have} of {} deaths have a clip", d.deaths.len()),
                    ),
                    (Style::fg(GREY), format!(" · from {}", dir.display())),
                ]);
                b.push(vec![]);
                if d.deaths.is_empty() {
                    b.push(vec![(Style::fg(GREY), "    You didn't die this match.".into())]);
                    return b;
                }
                self.sel = self.sel.min(d.deaths.len() - 1);
                let deaths: Vec<String> = d
                    .deaths
                    .iter()
                    .map(|x| format!("{} at {}", x.killer, x.place.as_deref().unwrap_or("?")))
                    .collect();
                let names: Vec<String> = per_death
                    .iter()
                    .map(|c| {
                        c.as_ref()
                            .and_then(|m| m.path.file_name())
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    })
                    .collect();
                let width =
                    |v: &[String], min: usize| v.iter().map(|s| s.chars().count() + 2).max().unwrap_or(0).max(min);
                let death_w = width(&deaths, 20);
                // The clip column takes what's left, for long file names.
                let clip_w = width(&names, 38).min(w.saturating_sub(death_w + 30).max(12));
                b.push(vec![(
                    Style::fg(GREY),
                    format!(
                        "    {:>3}  {}{}{:>6}  {:>8}",
                        "rnd",
                        pad("death", death_w),
                        pad("clip", clip_w),
                        "length",
                        "size"
                    ),
                )]);
                let items = deaths
                    .iter()
                    .zip(&names)
                    .zip(d.deaths.iter().zip(per_death))
                    .map(|((what, name), (x, c))| {
                        let it = Item::new(format!("{:>3}  {}", x.round, pad(what, death_w)));
                        match c {
                            Some(m) => Item::new(format!(
                                "{}{}{:>6}  {:>8}",
                                it.text,
                                pad(&clip_name(name, clip_w - 2), clip_w),
                                m.length_s.map_or_else(|| "?".into(), clip_length),
                                m.bytes.map_or_else(|| "?".into(), clip_size)
                            )),
                            None => it.tail("no clip", GREY),
                        }
                    })
                    .collect();
                b.extend(list_rows(items, self.sel, h.saturating_sub(3).max(3)));
            }
        }
        fit(b, h)
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
                shot_error
                    .as_deref()
                    .unwrap_or("None of your deaths has a clip, so there's nothing to take screenshots from."),
                2,
                w,
                Style::fg(YELLOW),
            ));
        } else {
            b.push(vec![
                (Style::default(), "  ".into()),
                (Style::fg(GREEN).bold(), "✓ ".into()),
                (
                    Style::default().bold(),
                    format!("{} ready", plural(shots.len(), "screenshot")),
                ),
                (Style::fg(GREY), ", one for each death with a clip".into()),
            ]);
            b.push(vec![]);
            let how = match style {
                vantage_core::clips::ShotStyle::Strip => {
                    "Each screenshot puts three moments side by side: 3 seconds before you died, 1 second before, and the moment you died, so the whole fight is in one image."
                }
                vantage_core::clips::ShotStyle::One => "Each screenshot is the moment you died.",
                vantage_core::clips::ShotStyle::Three => {
                    "Each death has three screenshots: 3 seconds before you died, 1 second before, and the moment you died."
                }
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
                        x.map(|x| format!("{} at {}", x.killer, x.place.as_deref().unwrap_or("?")))
                            .unwrap_or_default(),
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
        }
        self.flash_rows(&mut b, w);
        fit(b, h)
    }

    fn ai_tab(&mut self, w: usize, h: usize) -> Vec<Row> {
        let shots = self.data.shots().len();
        let provider = self.data.ai_provider;
        if let Some((who, started)) = &self.asking {
            let (style, glyph) = spike(started.elapsed());
            let mut b = vec![
                vec![
                    (style, format!("  {glyph} ")),
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
        if self.data.ai.is_none()
            && let Some((who, started)) = &self.reviewing
        {
            let (style, glyph) = spike(started.elapsed());
            let mut b = vec![
                vec![
                    (style, format!("  {glyph} ")),
                    (Style::fg(ACCENT), format!("{who} is reviewing this match…")),
                ],
                vec![],
            ];
            b.extend(para(
                &format!(
                    "It has this match's facts and {}. Its answer is checked against the AI review format before it's saved, and it's told what to fix if it doesn't fit.",
                    plural(shots, "screenshot")
                ),
                4,
                w,
                Style::fg(GREY),
            ));
            return b;
        }
        if let Some(saved) = self.data.ai.clone() {
            return if self.json {
                self.ai_json(&saved, w, h)
            } else {
                self.ai_result(&saved, w, h)
            };
        }
        let mut b = vec![heading("AI review", None)];
        b.extend(para(
            "An AI looks at the screenshots of your deaths alongside what Vantage already knows about the match, and judges what the replay alone can't show: where you stood, which angles could see you, and where your crosshair was before the fight.",
            4,
            w,
            Style::fg(GREY),
        ));
        b.push(vec![]);
        match (provider, self.data.ai_app) {
            (None, Some(app)) => {
                let what = if shots > 0 {
                    format!("this match's facts and {}", plural(shots, "screenshot"))
                } else {
                    "this match's facts, but no screenshots yet (pick your clip folder in the Clips tab first)"
                        .to_owned()
                };
                b.extend(field(
                    "Sends",
                    &format!("{}, when you ask it: {what}", app_title(app)),
                    w,
                    Style::default(),
                ));
                b.extend(field(
                    "Answers in",
                    &format!("the same format every time ({}), checked when it's saved", ai::SCHEMA),
                    w,
                    Style::default(),
                ));
                b.extend(field(
                    "Saved",
                    "once for this match. It can't be rerun, so every match is judged the same way, once.",
                    w,
                    Style::default(),
                ));
                b.extend(field(
                    "Costs",
                    "nothing extra: it uses the plan you already pay for, not an API key",
                    w,
                    Style::default(),
                ));
                if app.id == "other" {
                    b.extend(field(
                        "Set up",
                        "add a local MCP server to your AI app with the command: vantage mcp",
                        w,
                        Style::default(),
                    ));
                }
                b.push(vec![]);
                b.push(vec![(Style::fg(GREEN).bold(), format!("    Ask {}:", app.name))]);
                b.extend(para(
                    &crate::mcp::ask_line(&self.data.debrief),
                    4,
                    w,
                    Style::default().bold(),
                ));
                b.extend(para(
                    &format!(
                        "Press c to copy it. This tab fills in once {} saves its answer.",
                        app.name
                    ),
                    4,
                    w,
                    Style::fg(GREY),
                ));
            }
            (None, None) => {
                b.extend(bullet(
                    "•",
                    ACCENT,
                    "Connect your AI app, Claude, ChatGPT, Gemini, or Ollama on this PC in /settings → Config → AI review.",
                    w,
                ));
                b.extend(bullet(
                    "•",
                    ACCENT,
                    "No API key? Your AI app (Claude Desktop, Claude Code and others) reviews through MCP on the plan you already pay for. You start it from that app's chat.",
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
            (Some(p), _) => {
                let what = if shots > 0 {
                    format!(
                        "this match's facts and {} from the Screenshots tab",
                        plural(shots, "screenshot")
                    )
                } else {
                    "this match's facts and no screenshots (pick your clip folder in the Clips tab first)".to_owned()
                };
                if p.local {
                    b.extend(field(
                        "Gives",
                        &format!("Ollama, on this PC: {what}. Nothing leaves this computer."),
                        w,
                        Style::default(),
                    ));
                } else {
                    b.extend(field("Sends", &format!("{}: {what}", p.name), w, Style::default()));
                }
                b.extend(field(
                    "Answers in",
                    &format!("the same format every time ({}), so Vantage can read it", ai::SCHEMA),
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
                b.push(vec![(Style::fg(GREEN).bold(), "    Press Ctrl+Enter to run it".into())]);
            }
        }
        self.flash_rows(&mut b, w);
        fit(b, h)
    }

    fn ai_result(&mut self, s: &Saved, w: usize, h: usize) -> Vec<Row> {
        let o = &s.output;
        let saved = crate::short_when(&s.meta.saved_at);
        let mut b: Vec<Row> = vec![vec![
            (Style::fg(ACCENT).bold(), "  AI review".into()),
            (Style::fg(GREY), format!(" · {} · saved {saved}", s.meta.provider)),
        ]];
        b.extend(para(&o.summary, 4, w, Style::default()));
        b.push(vec![]);
        if let Some(f) = &self.flash {
            b.extend(notice(f, w));
            b.push(vec![]);
        }
        b.push(vec![
            (Style::fg(YELLOW).bold(), "  What it found".into()),
            (Style::fg(GREY), " · in the screenshots, most costly first".into()),
        ]);
        self.sel = self.sel.min(o.findings.len().saturating_sub(1));
        let mut detail: Vec<Row> = Vec::new();
        if let Some(f) = o.findings.get(self.sel) {
            detail.extend([vec![], rule_line(w), vec![]]);
            let rounds: Vec<String> = f.rounds.iter().map(u32::to_string).collect();
            detail.extend(field(
                "Rounds",
                &format!("{} · {}", list_and(&rounds), f.screenshots.join(", ")),
                w,
                Style::default(),
            ));
            detail.extend(field("What it saw", &f.saw, w, Style::default()));
            detail.extend(field("Why it costs", &f.why, w, Style::default()));
            detail.extend(field("Try", &f.try_this, w, Style::fg(GREEN)));
        }
        // The list shrinks, scrolling with the selection, so the finding's detail fits.
        b.extend(list_rows(
            o.findings
                .iter()
                .map(|f| {
                    Item::new(format!("{} ", pad(&f.title, 50)))
                        .tail(format!("{} · {}", ai::category_label(&f.category), f.confidence), GREY)
                })
                .collect(),
            self.sel,
            list_h(o.findings.len(), o.findings.len(), b.len() + detail.len(), h),
        ));
        b.extend(detail);
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
        fit(b, h)
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
                    (
                        Style::fg(ACCENT),
                        format!("    {}", pad(if i == 0 { name } else { "" }, 18)),
                    ),
                    (Style::default(), p),
                ]);
            }
        }
        let cats = ai::CATEGORIES.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ");
        for (i, p) in wrap(&cats, w.saturating_sub(26).max(10)).into_iter().enumerate() {
            lines.push(vec![
                (
                    Style::fg(ACCENT),
                    format!("    {}", pad(if i == 0 { "categories" } else { "" }, 18)),
                ),
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

/// `2 times this match.`
pub fn focus_result(c: &Check) -> String {
    let Some(n) = c.this_match else {
        return "this match couldn't tell.".into();
    };
    let slug = c.habit.slug;
    match slug {
        "moving" => format!("{} on the move this match.", habits::describe(slug, n)),
        "crosshair" | "bodies" | "rotation" => format!("{} this match.", habits::describe(slug, n)),
        "buy-sync" if n == 0.0 => "in sync every round this match.".to_owned(),
        "buy-sync" => format!("{} out of sync this match.", plural(n as usize, "round")),
        _ if n == 0.0 => "not once this match.".to_owned(),
        _ => format!("{} this match.", plural(n as usize, "time")),
    }
}

pub fn result_style(won: usize, lost: usize) -> (&'static str, Color) {
    match vantage_core::coach::result_word(won, lost) {
        "won" => ("won", GREEN),
        "lost" => ("lost", RED),
        _ => ("drew", YELLOW),
    }
}

/// The app's name to start a sentence with: `Your AI app`.
fn app_title(app: &ai::App) -> String {
    let mut c = app.name.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Ctrl+Enter, which runs the AI review.
fn is_run(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Enter | KeyCode::Char('\n'))
}

/// `  ◆ text`, wrapped under itself.
fn notice(text: &str, w: usize) -> Vec<Row> {
    wrap(text, w.saturating_sub(8).max(10))
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            vec![(
                Style::fg(YELLOW),
                format!("{}{p}", if i == 0 { "  ◆ " } else { "    " }),
            )]
        })
        .collect()
}

/// A file name cut to `n` characters, with `…` where it was cut.
fn clip_name(name: &str, n: usize) -> String {
    if name.chars().count() <= n {
        return name.to_owned();
    }
    let mut s: String = name.chars().take(n.saturating_sub(1)).collect();
    s.push('…');
    s
}

/// `30s`, or `1m 05s`.
fn clip_length(s: f64) -> String {
    let s = s.round() as u64;
    if s < 60 {
        format!("{s}s")
    } else {
        format!("{}m {:02}s", s / 60, s % 60)
    }
}

/// `48.2 MB`.
fn clip_size(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.1} MB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_tests::{key, review_data, saved};
    use vantage_core::clips::ShotStyle;

    fn with_shots() -> Clips {
        Clips::Set {
            dir: "clips".into(),
            per_death: vec![None; 7],
            shots_dir: "shots".into(),
            shots: vec![(1, vec!["R01.jpg".into()])],
            shot_error: None,
            style: ShotStyle::Strip,
        }
    }

    fn ctrl_enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)
    }

    #[test]
    fn enter_follows_debrief_to_fixes_to_the_map() {
        let mut p = ReviewPanel::new(review_data(with_shots(), None));
        p.key(key(KeyCode::Down));
        p.key(key(KeyCode::Down));
        assert_eq!(p.scroll, 2, "the Debrief scrolls");
        p.key(key(KeyCode::Enter));
        assert_eq!((p.tab, p.scroll), (T_FIXES, 0));
        p.key(key(KeyCode::Down));
        let fix = &p.data.debrief.fixes[p.sel];
        let first = p
            .data
            .debrief
            .deaths
            .iter()
            .position(|d| fix.rounds.contains(&d.round))
            .unwrap();
        p.key(key(KeyCode::Enter));
        assert_eq!((p.tab, p.death, p.sel), (T_MAP, first, 0));
        // Clips → Screenshots → AI with Enter.
        p.key(key(KeyCode::Char('4')));
        p.key(key(KeyCode::Enter));
        assert_eq!(p.tab, T_SHOTS);
        p.key(key(KeyCode::Enter));
        assert_eq!(p.tab, T_AI);
    }

    #[test]
    fn ctrl_enter_runs_the_ai_review_once() {
        let mut p = ReviewPanel::new(review_data(with_shots(), None));
        p.key(key(KeyCode::Char('6')));
        assert!(matches!(p.key(key(KeyCode::Enter)), ReviewAction::None));
        assert!(matches!(p.key(key(KeyCode::Char('a'))), ReviewAction::None));
        assert!(matches!(p.key(ctrl_enter()), ReviewAction::RunAi));
        assert!(is_run(&KeyEvent::new(KeyCode::Char('\n'), KeyModifiers::CONTROL)));

        let mut done = ReviewPanel::new(review_data(with_shots(), Some(saved())));
        done.key(key(KeyCode::Char('6')));
        assert!(matches!(done.key(ctrl_enter()), ReviewAction::None));
        assert!(done.flash.as_deref().is_some_and(|f| f.contains("already")));
        // Switching tabs clears the message.
        done.key(key(KeyCode::Left));
        assert!(done.flash.is_none());

        let mut no_shots = ReviewPanel::new(review_data(Clips::NotSet(vec![]), None));
        no_shots.key(key(KeyCode::Char('6')));
        assert!(matches!(no_shots.key(ctrl_enter()), ReviewAction::None));
        assert!(no_shots.flash.as_deref().is_some_and(|f| f.contains("no screenshots")));
    }

    #[test]
    fn clip_table_cells() {
        assert_eq!(clip_length(30.0), "30s");
        assert_eq!(clip_length(59.4), "59s");
        assert_eq!(clip_length(59.6), "1m 00s");
        assert_eq!(clip_length(65.0), "1m 05s");
        assert_eq!(clip_size(50_544_640), "48.2 MB");
        assert_eq!(clip_size(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
        assert_eq!(clip_name("short.mp4", 20), "short.mp4");
        assert_eq!(clip_name("VALORANT 2026.10.19.mp4", 10), "VALORANT …");
        assert_eq!(clip_name("abc", 0), "…");
    }
}
