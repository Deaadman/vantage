//! The `/plan`, `/habits` and `/rank` panels, drawn over the whole screen until Esc.

use crossterm::event::{KeyCode, KeyEvent};
use crossterm::style::Color;
use vantage_core::catalog::MapInfo;
use vantage_core::coach::plural;
use vantage_core::habits::{Progress, Trend, spark};
use vantage_core::minimap::Minimap;
use vantage_core::plan::{Plan, SidePlan, Tone};
use vantage_core::ranks::{self, Band};
use vantage_core::standing::Standing;

use crate::panel::{
    MapCache, body_height, bullet, field, frame, heading, is_close, map_rows_h, pad, para, rank_color, rule_line,
    scrolled, tab_key,
};
use crate::review_view::GREEN;
use crate::shell::{ACCENT, GREY, RED, Row, Style, YELLOW, wrap};

pub enum CoachAction {
    None,
    Close,
    /// Make this habit the focus.
    Focus(&'static str),
}

// ---------------------------------------------------------------- /plan

pub struct PlanData {
    pub plan: Plan,
    pub rank: Option<String>,
    /// The top fix from my newest match on the map: (title, what to try).
    pub fix: Option<(String, String)>,
    pub map: Option<MapInfo>,
    pub minimap: Result<Minimap, String>,
}

const PLAN_TABS: [&str; 3] = ["Attack", "Defense", "Map"];
const LEGEND_W: usize = 44;

pub struct PlanPanel {
    d: Box<PlanData>,
    tab: usize,
    scroll: usize,
    cache: MapCache,
}

impl PlanPanel {
    pub fn new(d: PlanData) -> Self {
        PlanPanel {
            d: Box::new(d),
            tab: 0,
            scroll: 0,
            cache: MapCache::default(),
        }
    }

    fn full(&self) -> bool {
        self.d.plan.matches >= vantage_core::plan::FULL_AT
    }

    pub fn key(&mut self, key: KeyEvent) -> CoachAction {
        if is_close(&key) {
            return CoachAction::Close;
        }
        if self.full() && tab_key(&mut self.tab, &key, PLAN_TABS.len()) {
            self.scroll = 0;
            return CoachAction::None;
        }
        match key.code {
            KeyCode::Down => self.scroll += 1,
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            _ => {}
        }
        CoachAction::None
    }

    fn title(&self) -> Row {
        let p = &self.d.plan;
        let agents = if p.agents.is_empty() {
            String::new()
        } else {
            format!(" as {}", p.agents.join(", "))
        };
        let mut row: Row = vec![
            (Style::fg(ACCENT).bold(), "  Plan  ".into()),
            (Style::default().bold(), format!("{} ", p.map)),
            (Style::fg(GREY), format!("· {}{agents}", plural(p.matches, "match"))),
        ];
        if let Some(r) = &self.d.rank {
            row.push((Style::fg(GREY), " · for ".into()));
            row.push((Style::fg(rank_color(Some(r))), r.clone()));
        }
        row
    }

    pub fn rows(&mut self, w: usize, h: usize) -> Vec<Row> {
        let title = self.title();
        if !self.full() {
            let body_h = body_height(h, false);
            let lines = self.not_enough(w);
            let body = scrolled(&mut self.scroll, lines, body_h);
            return frame(
                w,
                h,
                title,
                None,
                body,
                "↑↓ scroll · Esc closes · /plan --map picks another map",
            );
        }
        let body_h = body_height(h, true);
        let body = match self.tab {
            0 => scrolled(&mut self.scroll, side(&self.d.plan.attack, true, w), body_h),
            1 => scrolled(&mut self.scroll, side(&self.d.plan.defense, false, w), body_h),
            _ => self.map(w, body_h),
        };
        frame(
            w,
            h,
            title,
            Some((&PLAN_TABS, self.tab)),
            body,
            "←→ or 1-3 tabs · ↑↓ scroll · Esc closes · /plan --map picks another map",
        )
    }

    fn map(&mut self, w: usize, h: usize) -> Vec<Row> {
        let rows_h = map_rows_h(w, h, LEGEND_W);
        let mut cells = match self.cache.cells(&self.d.minimap, self.d.map.as_ref(), rows_h) {
            Ok(c) => c,
            Err(e) => return vec![vec![(Style::fg(GREY), format!("  No minimap: {e}"))]],
        };
        let p = &self.d.plan;
        for uv in &p.deaths {
            cells.mark(*uv, RED, "●");
        }
        for uv in &p.wins {
            cells.mark(*uv, GREEN, "▲");
        }
        let mut legend: Vec<Row> = vec![
            vec![
                (Style::fg(RED).bold(), "● ".into()),
                (Style::default(), "where you died".into()),
            ],
            vec![
                (Style::fg(GREEN).bold(), "▲ ".into()),
                (Style::default(), "where you won fights".into()),
            ],
            vec![],
        ];
        if !p.go_here.is_empty() {
            legend.push(vec![(Style::fg(GREEN).bold(), "Go here".into())]);
            legend.extend(p.go_here.iter().map(|s| vec![(Style::default(), s.clone())]));
            legend.push(vec![]);
        }
        if !p.stay_out.is_empty() {
            legend.push(vec![(Style::fg(RED).bold(), "Stay out of here".into())]);
            legend.extend(p.stay_out.iter().map(|s| vec![(Style::default(), s.clone())]));
            legend.push(vec![]);
        }
        legend.push(vec![(Style::default().bold(), "In one line".into())]);
        legend.extend(wrap(&p.one_line, 40).into_iter().map(|l| vec![(Style::default(), l)]));
        cells.beside(legend)
    }

    fn not_enough(&self, w: usize) -> Vec<Row> {
        let p = &self.d.plan;
        let rank = self.d.rank.as_deref();
        let at = rank.map(|r| format!(", and at {r}")).unwrap_or_default();
        let mut b = para(
            &format!(
                "You've played {} on {}. After {}, this becomes your own plan: where you win and lose on each side. Until then, here's what matters on {}{at}.",
                plural(p.matches, "match"),
                p.map,
                vantage_core::plan::FULL_AT,
                p.map
            ),
            2,
            w,
            Style::fg(YELLOW),
        );
        b.push(vec![]);
        b.push(heading(&format!("On {}", p.map), None));
        for t in vantage_core::plan::tips(&p.map) {
            b.extend(bullet("•", ACCENT, t, w));
        }
        if let Some(r) = rank {
            let adv = ranks::advice(ranks::band(r));
            b.push(vec![]);
            let mut at: Row = vec![(Style::default().bold(), "  At ".into())];
            at.extend(ranks_in_colour(adv.label, Style::default().bold()));
            b.push(at);
            for (t, why) in adv.wins.iter().take(3) {
                b.extend(bullet("•", ACCENT, &format!("{t}. {why}"), w));
            }
        }
        if let Some((title, try_this)) = &self.d.fix {
            b.push(vec![]);
            b.push(heading("From your match here", None));
            b.extend(bullet("!", YELLOW, &format!("{title}. {try_this}"), w));
        }
        b
    }
}

fn site_bar(won: usize, of: usize) -> Row {
    let p = if of == 0 { 0.0 } else { won as f64 / of as f64 };
    let n = (p * 10.0).round() as usize;
    let c = if p >= 0.5 {
        GREEN
    } else if p >= 0.4 {
        YELLOW
    } else {
        RED
    };
    vec![
        (Style::fg(c), "█".repeat(n)),
        (Style::fg(Color::Rgb { r: 58, g: 58, b: 58 }), "░".repeat(10 - n)),
    ]
}

fn side(p: &SidePlan, attack: bool, w: usize) -> Vec<Row> {
    let mut b = vec![heading(
        if attack {
            "On attack: how your rounds go at each site"
        } else {
            "On defense: how your rounds go at each site"
        },
        None,
    )];
    if p.sites.is_empty() {
        b.extend(para(
            "Not enough rounds on this side yet to tell one site from another.",
            4,
            w,
            Style::fg(GREY),
        ));
    }
    for s in &p.sites {
        let mut row: Row = vec![(Style::default().bold(), format!("    {}", pad(&s.site, 4)))];
        row.extend(site_bar(s.won, s.of));
        row.push((
            Style::fg(GREY),
            format!("  won {:>2} of {} ", s.won, pad(&s.of.to_string(), 3)),
        ));
        let c = match s.tone {
            Tone::Good => GREEN,
            Tone::Bad => YELLOW,
            Tone::Plain => GREY,
        };
        row.push((Style::fg(c), s.note.clone()));
        b.push(row);
    }
    if !p.do_this.is_empty() {
        b.push(vec![]);
        b.push(heading("Do", Some(GREEN)));
        for t in &p.do_this {
            b.extend(bullet("✓", GREEN, t, w));
        }
    }
    if !p.avoid.is_empty() {
        b.push(vec![]);
        b.push(heading("Avoid", Some(RED)));
        for t in &p.avoid {
            b.extend(bullet("✗", RED, t, w));
        }
    }
    b.push(vec![]);
    b.push(heading(
        if attack { "After the plant" } else { "Copy this round" },
        None,
    ));
    b.extend(para(&p.after, 4, w, Style::default()));
    b
}

// -------------------------------------------------------------- /habits

/// The habit names' column.
const NAME_W: usize = 40;

pub struct HabitsPanel {
    progress: Vec<Progress>,
    focus: &'static str,
    matches: usize,
    sel: usize,
    flash: Option<String>,
}

pub fn trend_color(t: Trend) -> Color {
    match t {
        Trend::Better => GREEN,
        Trend::Worse => RED,
        Trend::Steady => GREY,
    }
}

impl HabitsPanel {
    pub fn new(progress: Vec<Progress>, focus: &'static str, matches: usize) -> Self {
        let sel = progress.iter().position(|p| p.habit.slug == focus).unwrap_or(0);
        HabitsPanel {
            progress,
            focus,
            matches,
            sel,
            flash: None,
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> CoachAction {
        if is_close(&key) {
            return CoachAction::Close;
        }
        match key.code {
            KeyCode::Up => {
                self.sel = self.sel.saturating_sub(1);
                self.flash = None;
            }
            KeyCode::Down => {
                self.sel = (self.sel + 1).min(self.progress.len().saturating_sub(1));
                self.flash = None;
            }
            KeyCode::Enter => {
                if let Some(p) = self.progress.get(self.sel) {
                    self.focus = p.habit.slug;
                    self.flash = Some(format!("{} is now your focus. Every debrief checks it.", p.habit.name));
                    return CoachAction::Focus(p.habit.slug);
                }
            }
            _ => {}
        }
        CoachAction::None
    }

    pub fn rows(&mut self, w: usize, h: usize) -> Vec<Row> {
        let n = self.matches.min(vantage_core::habits::WINDOW);
        let focus_name = vantage_core::habits::focus(Some(self.focus)).name;
        let title: Row = vec![
            (Style::fg(ACCENT).bold(), "  Habits  ".into()),
            (Style::fg(GREY), format!("your last {} · focus: ", plural(n, "match"))),
            (Style::fg(YELLOW).bold(), focus_name.into()),
        ];
        let mut b: Vec<Row> = vec![vec![(
            Style::fg(GREY),
            format!(
                "    {}{}{}lately",
                pad("habit", NAME_W),
                pad(&format!("last {n}"), 10),
                pad("now", 20)
            ),
        )]];
        let mut list: Vec<Row> = Vec::new();
        let mut sel_row = 0;
        for (i, p) in self.progress.iter().enumerate() {
            if i == 0 || self.progress[i - 1].habit.group != p.habit.group {
                list.push(heading(p.habit.group, Some(GREY)));
            }
            let on = i == self.sel;
            if on {
                sel_row = list.len();
            }
            let c = trend_color(p.trend);
            let base = if on { Style::fg(ACCENT) } else { Style::default() };
            let mut row: Row = vec![
                (
                    base,
                    format!("{}{}", if on { "  ❯ " } else { "    " }, pad(p.habit.name, NAME_W)),
                ),
                (Style::fg(c), pad(&spark(&p.values), 10)),
                (base, pad(&p.now, 20)),
                (Style::fg(c).bold(), pad(p.trend.word(), 8)),
            ];
            if p.habit.slug == self.focus {
                row.push((Style::fg(YELLOW).bold(), "◆ focus".into()));
            }
            list.push(row);
        }
        let mut detail: Vec<Row> = Vec::new();
        if let Some(p) = self.progress.get(self.sel) {
            detail.push(vec![]);
            detail.push(rule_line(w));
            detail.push(vec![]);
            detail.extend(field("What it is", p.habit.what, w, Style::default()));
            detail.extend(field("Why it matters", p.habit.why, w, Style::default()));
            detail.extend(field("Practise", p.habit.drill, w, Style::fg(GREEN)));
        }
        if let Some(f) = &self.flash {
            detail.push(vec![]);
            detail.push(vec![(Style::fg(YELLOW), format!("    ◆ {f}"))]);
        }
        // On a short screen the list scrolls with the selection, so the detail still fits.
        let list_h = body_height(h, false).saturating_sub(b.len() + detail.len()).max(5);
        if list.len() > list_h {
            let first = sel_row.saturating_sub(list_h / 2).min(list.len() - list_h);
            list = list.drain(first..first + list_h).collect();
        }
        b.extend(list);
        b.extend(detail);
        frame(
            w,
            h,
            title,
            None,
            b,
            "↑↓ select · Enter: make it your focus · Esc closes · better = your last 3 matches beat the ones before",
        )
    }
}

/// `Gold and Platinum` with each rank in its own colour.
fn ranks_in_colour(text: &str, base: Style) -> Row {
    text.split_inclusive(' ')
        .map(|word| {
            let tier = word.trim_end();
            match ranks::colour(tier) {
                Some(c) => (
                    Style {
                        fg: Some(crate::panel::rgb(c)),
                        ..base
                    },
                    word.to_owned(),
                ),
                None => (base, word.to_owned()),
            }
        })
        .collect()
}

// ---------------------------------------------------------------- /rank

const RANK_TABS: [&str; 2] = ["What wins at your rank", "You in your games"];

pub struct RankPanel {
    rank: String,
    standing: Standing,
    focus_name: &'static str,
    tab: usize,
    scroll: usize,
}

impl RankPanel {
    pub fn new(rank: String, standing: Standing, focus_name: &'static str) -> Self {
        RankPanel {
            rank,
            standing,
            focus_name,
            tab: 0,
            scroll: 0,
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> CoachAction {
        if is_close(&key) {
            return CoachAction::Close;
        }
        if tab_key(&mut self.tab, &key, RANK_TABS.len()) {
            self.scroll = 0;
        }
        match key.code {
            KeyCode::Down => self.scroll += 1,
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            _ => {}
        }
        CoachAction::None
    }

    pub fn rows(&mut self, w: usize, h: usize) -> Vec<Row> {
        let next = ranks::next(&self.rank);
        let band = ranks::band(&self.rank);
        let adv = ranks::advice(band);
        let title: Row = vec![
            (Style::fg(ACCENT).bold(), "  Rank  ".into()),
            (Style::fg(rank_color(Some(&self.rank))).bold(), self.rank.clone()),
            (Style::fg(GREY), " · set by you in /settings · aiming for ".into()),
            (Style::fg(rank_color(Some(&next))), next.clone()),
        ];
        let mut b: Vec<Row> = Vec::new();
        if self.tab == 0 {
            b.extend(para(
                &format!("What decides games at {}, most important first.", adv.label),
                2,
                w,
                Style::fg(GREY),
            ));
            b.push(vec![]);
            for (i, (t, why)) in adv.wins.iter().enumerate() {
                b.push(vec![
                    (Style::fg(ACCENT).bold(), format!("  {}  ", i + 1)),
                    (Style::default().bold(), (*t).into()),
                ]);
                b.extend(para(why, 5, w, Style::default()));
                b.push(vec![]);
            }
            b.extend(para(adv.verdict, 2, w, Style::fg(YELLOW)));
        } else {
            let s = &self.standing;
            if s.better.is_empty() && s.behind.is_empty() {
                b.extend(para(
                    "Not enough matches with the whole lobby stored yet to compare you with the players in your games. Play a few more and it fills in.",
                    2,
                    w,
                    Style::fg(GREY),
                ));
            } else {
                b.extend(para(
                    &format!(
                        "Compared with the other players in your last {}, who are mostly around your rank.",
                        plural(s.games, "match")
                    ),
                    2,
                    w,
                    Style::fg(GREY),
                ));
                b.push(vec![]);
                if !s.better.is_empty() {
                    b.push(heading("Better than most", Some(GREEN)));
                    for t in &s.better {
                        b.extend(bullet("+", GREEN, t, w));
                    }
                    b.push(vec![]);
                }
                if !s.behind.is_empty() {
                    b.push(heading("Behind most", Some(RED)));
                    for t in &s.behind {
                        b.extend(bullet("-", RED, t, w));
                    }
                    b.push(vec![]);
                }
                b.push(vec![
                    (Style::default().bold(), "  To reach ".into()),
                    (Style::fg(rank_color(Some(&next))).bold(), next.clone()),
                ]);
                let text = match band {
                    Band::Low => "Work on stopping before you shoot and on keeping your crosshair at head height, and the first deaths will drop.".to_owned(),
                    Band::Mid => format!(
                        "What holds players back here is the second list: when you fight, more than how you aim. Your focus, {}, is a good place to start.",
                        self.focus_name
                    ),
                    Band::High => "Dying first tells enemies where you'll be: change which fights you take, and when.".to_owned(),
                    Band::Top => "At this rank, the second list is what better players use against you. Every repeat is a pattern they can read.".to_owned(),
                };
                b.extend(para(&text, 4, w, Style::default()));
            }
        }
        let body_h = body_height(h, true);
        let body = scrolled(&mut self.scroll, b, body_h);
        frame(
            w,
            h,
            title,
            Some((&RANK_TABS, self.tab)),
            body,
            "←→ tabs · ↑↓ scroll · Esc closes · replays don't include rank, so change it in /settings when you rank up",
        )
    }
}
