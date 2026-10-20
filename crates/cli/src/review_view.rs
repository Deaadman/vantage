//! The `/review` and `/summary` panel, drawn over the whole screen until Esc.
//! For one match: Overview, Work on, Rounds, Deaths, Map. For all matches on
//! a map: Overview, Work on, Matches, Deaths, Map. Rounds count from 1 here,
//! as in the game.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Color;
use valreview_core::analysis::Side;
use valreview_core::catalog::MapInfo;
use valreview_core::deaths::DeathFact;
use valreview_core::facts::RoundFact;
use valreview_core::minimap::Minimap;
use valreview_core::packet::MatchReview;
use valreview_core::review::{self, Finding, ordinal, percent};
use valreview_core::summary;

use crate::shell::{ACCENT, BRIGHT, GREY, RED, Row, Style, YELLOW, wrap};
use crate::{ReviewData, SummaryData};

pub const GREEN: Color = Color::Rgb { r: 120, g: 200, b: 120 };
const MATCH_TABS: [&str; 5] = ["Overview", "Work on", "Rounds", "Deaths", "Map"];
const MAP_TABS: [&str; 5] = ["Overview", "Work on", "Matches", "Deaths", "Map"];
const LEGEND_W: usize = 44;

pub enum ReviewAction {
    None,
    Close,
    /// Open this match's review (from a summary's Matches tab).
    Open(String),
}

/// What the panel shows: one match, or every match on one map.
pub enum Content {
    Match(Box<ReviewData>),
    Map(Box<SummaryData>),
}

pub struct ReviewPanel {
    data: Content,
    tab: usize,
    /// First row shown on the Overview tab.
    scroll: usize,
    finding: usize,
    /// Selected round (one match) or match (a map).
    round: usize,
    death: usize,
    /// The minimap shrunk to a size, kept between frames: (pixels a side, cells).
    map_cache: Option<(usize, Vec<[u8; 4]>)>,
}

impl ReviewPanel {
    pub fn new(data: ReviewData) -> Self {
        Self::with(Content::Match(Box::new(data)))
    }

    pub fn summary(data: SummaryData) -> Self {
        Self::with(Content::Map(Box::new(data)))
    }

    fn with(data: Content) -> Self {
        ReviewPanel {
            data,
            tab: 0,
            scroll: 0,
            finding: 0,
            round: 0,
            death: 0,
            map_cache: None,
        }
    }

    fn one(&self) -> Option<&ReviewData> {
        match &self.data {
            Content::Match(d) => Some(d),
            Content::Map(_) => None,
        }
    }

    fn tabs(&self) -> [&'static str; 5] {
        match self.data {
            Content::Match(_) => MATCH_TABS,
            Content::Map(_) => MAP_TABS,
        }
    }

    fn findings(&self) -> &[Finding] {
        match &self.data {
            Content::Match(d) => &d.overview.work_on,
            Content::Map(d) => &d.summary.work_on,
        }
    }

    /// One match's deaths (empty for a map).
    fn deaths(&self) -> &[DeathFact] {
        self.one().map_or(&[], |d| &d.review.death_facts)
    }

    /// One match's rounds (empty for a map).
    fn rounds(&self) -> &[RoundFact] {
        self.one().map_or(&[], |d| &d.review.round_facts)
    }

    /// Every death shown, with its match: newest match first for a map.
    fn all_deaths(&self) -> Vec<(&MatchReview, &DeathFact)> {
        match &self.data {
            Content::Match(d) => d.review.death_facts.iter().map(|f| (&d.review, f)).collect(),
            Content::Map(d) => d
                .reviews
                .iter()
                .rev()
                .flat_map(|r| r.death_facts.iter().map(move |f| (r, f)))
                .collect(),
        }
    }

    fn minimap(&self) -> &Result<Minimap, String> {
        match &self.data {
            Content::Match(d) => &d.minimap,
            Content::Map(d) => &d.minimap,
        }
    }

    fn map_info(&self) -> Option<&MapInfo> {
        match &self.data {
            Content::Match(d) => d.map.as_ref(),
            Content::Map(d) => d.map.as_ref(),
        }
    }

    /// The selectable rows of the third tab: rounds, or matches.
    fn third_len(&self) -> usize {
        match &self.data {
            Content::Match(d) => d.review.round_facts.len(),
            Content::Map(d) => d.summary.matches.len(),
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> ReviewAction {
        let n_find = self.findings().len();
        let n_rounds = self.third_len();
        let n_deaths = self.all_deaths().len();
        let n_tabs = self.tabs().len();
        let step = |v: &mut usize, by: isize, n: usize| {
            *v = (*v as isize + by).clamp(0, n.saturating_sub(1) as isize) as usize;
        };
        let by = match key.code {
            KeyCode::Up => -1,
            KeyCode::Down => 1,
            KeyCode::PageUp => -10,
            KeyCode::PageDown => 10,
            KeyCode::Home => -10_000,
            KeyCode::End => 10_000,
            _ => 0,
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return ReviewAction::Close,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return ReviewAction::Close,
            KeyCode::Left | KeyCode::BackTab => self.tab = (self.tab + n_tabs - 1) % n_tabs,
            KeyCode::Right | KeyCode::Tab => self.tab = (self.tab + 1) % n_tabs,
            KeyCode::Char(c @ '1'..='5') => self.tab = c as usize - '1' as usize,
            KeyCode::Enter => return self.enter(),
            _ if by != 0 => match self.tab {
                0 => self.scroll = (self.scroll as isize + by).max(0) as usize,
                1 => step(&mut self.finding, by, n_find),
                2 => step(&mut self.round, by, n_rounds),
                _ => step(&mut self.death, by, n_deaths),
            },
            _ => {}
        }
        ReviewAction::None
    }

    /// Enter follows a link to the next level of detail.
    fn enter(&mut self) -> ReviewAction {
        if let Content::Map(d) = &self.data {
            match self.tab {
                2 => {
                    if let Some(m) = d.summary.matches.get(self.round) {
                        return ReviewAction::Open(m.id.clone());
                    }
                }
                3 => self.tab = 4,
                _ => {}
            }
            return ReviewAction::None;
        }
        match self.tab {
            // A finding: its first round in the Rounds tab.
            1 => {
                if let Some(r) = self.findings().get(self.finding).and_then(|f| f.rounds.first())
                    && let Some(i) = self.rounds().iter().position(|f| f.round + 1 == *r)
                {
                    self.round = i;
                    self.tab = 2;
                }
            }
            // A round: my death in it, if any.
            2 => {
                let round = self.rounds().get(self.round).map(|f| f.round);
                if let Some(i) = self.deaths().iter().position(|d| d.round == round) {
                    self.death = i;
                    self.tab = 3;
                }
            }
            // A death: where it was.
            3 => self.tab = 4,
            _ => {}
        }
        ReviewAction::None
    }

    /// The panel, exactly `h` rows for a `w`-column screen.
    pub fn rows(&mut self, w: usize, h: usize) -> Vec<Row> {
        let title = match &self.data {
            Content::Match(d) => match_title(d),
            Content::Map(d) => map_title(d),
        };
        let mut rows: Vec<Row> = vec![
            vec![(Style::fg(ACCENT), "─".repeat(w.saturating_sub(1)))],
            title,
            vec![],
        ];
        let mut tabs: Row = vec![(Style::default(), " ".into())];
        for (i, name) in self.tabs().iter().enumerate() {
            tabs.push((Style::default(), " ".into()));
            let style = if i == self.tab {
                Style {
                    fg: Some(Color::Black),
                    bg: Some(BRIGHT),
                    bold: false,
                }
            } else {
                Style::default()
            };
            tabs.push((style, format!(" {name} ")));
        }
        rows.push(tabs);
        rows.push(vec![]);

        let map = matches!(self.data, Content::Map(_));
        let enter = match (map, self.tab) {
            (false, 1) => " · Enter: its rounds",
            (false, 2) => " · Enter: your death",
            (true, 2) => " · Enter: open the match",
            (_, 3) => " · Enter: on the map",
            _ => "",
        };
        let tail = if map {
            "Esc in a match comes back here"
        } else {
            "/export exports this match for an AI"
        };
        let footer = vec![
            vec![],
            vec![(
                Style::fg(GREY),
                format!("  ←→ or 1-5 tabs · ↑↓ select{enter} · Esc closes · {tail}"),
            )],
        ];
        let body_h = h.saturating_sub(rows.len() + footer.len()).max(1);
        let mut body = match (map, self.tab) {
            (false, 0) => self.overview(w, body_h),
            (true, 0) => self.summary_overview(w, body_h),
            (_, 1) => self.work_on(w, body_h),
            (false, 2) => self.rounds_tab(w, body_h),
            (true, 2) => self.matches_tab(w, body_h),
            (_, 3) => self.deaths_tab(w, body_h),
            _ => self.map(w, body_h),
        };
        body.resize(body_h, vec![]);
        rows.extend(body);
        rows.extend(footer);
        rows.truncate(h);
        rows
    }

    // ------------------------------------------------------------ overview

    fn overview(&mut self, w: usize, h: usize) -> Vec<Row> {
        let Content::Match(d) = &self.data else {
            return vec![];
        };
        let o = &d.overview;
        let mut lines: Vec<Row> = vec![title("You against the lobby")];
        for s in &o.stats {
            let (rank, color) = match s.rank {
                Some(r) if r <= 3 => (format!("{} of {}", ordinal(r), s.of), GREEN),
                Some(r) if r + 2 >= s.of => (format!("{} of {}", ordinal(r), s.of), YELLOW),
                Some(r) => (format!("{} of {}", ordinal(r), s.of), GREY),
                None => (String::new(), GREY),
            };
            lines.push(vec![
                (Style::default(), format!("    {:<26}", s.label)),
                (Style::default().bold(), format!("{:<18}", s.value)),
                (Style::fg(color), rank),
            ]);
        }
        lines.push(vec![]);

        lines.push(title("Scoreboard"));
        lines.push(vec![(
            Style::fg(GREY),
            format!(
                "    {:<12}{:>4}{:>4}{:>9}{:>6}{:>5}{:>5}{:>6}",
                "", "K", "D", "dmg/rnd", "HS", "FK", "FD", "KST"
            ),
        )]);
        let mut enemies = false;
        for p in &o.players {
            if !p.ally && !enemies {
                enemies = true;
                lines.push(vec![(Style::fg(GREY), "    enemies".into())]);
            }
            let style = if p.me {
                Style::fg(ACCENT).bold()
            } else if p.ally {
                Style::default()
            } else {
                Style::fg(GREY)
            };
            lines.push(vec![(
                style,
                format!(
                    "  {} {:<12}{:>4}{:>4}{:>9.0}{:>6}{:>5}{:>5}{:>6}",
                    if p.me { "❯" } else { " " },
                    p.agent,
                    p.kills,
                    p.deaths,
                    p.adr,
                    p.hs.map_or("-".into(), percent),
                    p.first_kills,
                    p.first_deaths,
                    p.kst.map_or("-".into(), percent)
                ),
            )]);
        }
        if !o.has_lobby {
            lines.push(vec![(
                Style::fg(GREY),
                "    Only your review is stored for this match, so there is no lobby to compare with.".into(),
            )]);
        }
        lines.push(vec![(
            Style::fg(GREY),
            "    KST: rounds with a kill, survived or traded (the replay has no assists).".into(),
        )]);
        lines.push(vec![]);

        lines.push(vec![(Style::fg(GREEN).bold(), "  Went well".into())]);
        if o.went_well.is_empty() {
            lines.push(vec![(Style::fg(GREY), "    Nothing stood out.".into())]);
        }
        for f in &o.went_well {
            lines.push(vec![
                (Style::fg(GREEN), "    + ".into()),
                (Style::default(), f.title.clone()),
            ]);
        }
        lines.push(vec![]);
        lines.push(vec![
            (Style::fg(YELLOW).bold(), "  Work on".into()),
            (Style::fg(GREY), "  (the Work on tab has the evidence)".into()),
        ]);
        if o.work_on.is_empty() {
            lines.push(vec![(Style::fg(GREY), "    Nothing stood out.".into())]);
        }
        for (i, f) in o.work_on.iter().take(3).enumerate() {
            lines.push(vec![
                (Style::fg(YELLOW), format!("    {} ", i + 1)),
                (Style::default(), f.title.clone()),
            ]);
        }
        let notes: Vec<&String> = d.review.warnings.iter().chain(&d.review.context.build_note).collect();
        if !notes.is_empty() {
            lines.push(vec![]);
            for n in notes {
                for piece in wrap(&format!("Note: {n}"), w.saturating_sub(6).max(10)) {
                    lines.push(vec![(Style::fg(GREY), format!("    {piece}"))]);
                }
            }
        }
        self.scrolled(lines, h)
    }

    /// `lines` from the Overview's scroll position, with a "more" line.
    fn scrolled(&mut self, lines: Vec<Row>, h: usize) -> Vec<Row> {
        let max = lines.len().saturating_sub(h);
        self.scroll = self.scroll.min(max);
        let mut rows: Vec<Row> = lines.into_iter().skip(self.scroll).take(h).collect();
        if self.scroll < max
            && let Some(last) = rows.last_mut()
        {
            *last = vec![(Style::fg(GREY), format!("    ↓ {} more", max - self.scroll))];
        }
        rows
    }

    /// A map's Overview: totals against the lobbies, matches, what went well.
    fn summary_overview(&mut self, w: usize, h: usize) -> Vec<Row> {
        let Content::Map(d) = &self.data else {
            return vec![];
        };
        let s = &d.summary;
        let mut lines: Vec<Row> = Vec::new();
        if let Some(note) = &s.note {
            for piece in wrap(note, w.saturating_sub(6).max(10)) {
                lines.push(vec![(Style::fg(YELLOW), format!("  {piece}"))]);
            }
            lines.push(vec![]);
        }
        lines.push(title("Across these matches"));
        for st in &s.stats {
            lines.push(vec![
                (Style::default(), format!("    {:<26}", st.label)),
                (Style::default().bold(), st.value.clone()),
            ]);
        }
        lines.push(vec![]);
        lines.push(title("Matches"));
        for m in &s.matches {
            let (res, color) = if m.won > m.lost {
                ("W", GREEN)
            } else if m.won < m.lost {
                ("L", RED)
            } else {
                ("D", YELLOW)
            };
            lines.push(vec![
                (Style::default(), format!("    {}  {:<10} ", m.label, m.agent)),
                (Style::fg(color).bold(), format!("{res} {:>2}–{:<2}", m.won, m.lost)),
                (
                    Style::fg(GREY),
                    format!(
                        "  {:>2}/{:<2}  {:>4.0} dmg/round{}",
                        m.kills,
                        m.deaths,
                        m.adr,
                        m.adr_rank
                            .map(|r| format!(" ({} of {})", ordinal(r), m.of))
                            .unwrap_or_default()
                    ),
                ),
            ]);
        }
        lines.push(vec![]);
        lines.push(vec![(Style::fg(GREEN).bold(), "  Went well".into())]);
        if s.went_well.is_empty() {
            lines.push(vec![(Style::fg(GREY), "    Nothing stood out.".into())]);
        }
        for f in &s.went_well {
            lines.push(vec![
                (Style::fg(GREEN), "    + ".into()),
                (Style::default(), f.title.clone()),
            ]);
        }
        lines.push(vec![]);
        lines.push(vec![
            (Style::fg(YELLOW).bold(), "  Work on".into()),
            (Style::fg(GREY), "  (the Work on tab has the evidence)".into()),
        ]);
        if s.work_on.is_empty() {
            lines.push(vec![(Style::fg(GREY), "    Nothing repeats yet.".into())]);
        }
        for (i, f) in s.work_on.iter().take(3).enumerate() {
            lines.push(vec![
                (Style::fg(YELLOW), format!("    {} ", i + 1)),
                (Style::default(), f.title.clone()),
            ]);
        }
        self.scrolled(lines, h)
    }

    /// A map's matches; Enter opens one.
    fn matches_tab(&mut self, w: usize, h: usize) -> Vec<Row> {
        let Content::Map(d) = &self.data else {
            return vec![];
        };
        let ms = &d.summary.matches;
        if ms.is_empty() {
            return vec![];
        }
        self.round = self.round.min(ms.len() - 1);
        let header = format!(
            "{:<11}  {:<10} {:<7} {:>5}  {:>7}  {:<10} {}",
            "played", "agent", "score", "K/D", "dmg/rnd", "place", "HS"
        );
        let lines = ms.iter().map(|m| {
            format!(
                "{:<11}  {:<10} {:<7} {:>5}  {:>7.0}  {:<10} {}",
                m.label,
                m.agent,
                format!("{}–{}", m.won, m.lost),
                format!("{}/{}", m.kills, m.deaths),
                m.adr,
                m.adr_rank
                    .map(|r| format!("{} of {}", ordinal(r), m.of))
                    .unwrap_or_default(),
                m.hs.map_or("-".into(), percent)
            )
        });
        let mut rows = vec![vec![(Style::fg(GREY), format!("    {header}"))]];
        rows.extend(list_rows(lines, self.round, h.saturating_sub(3).min(ms.len())));
        rows.push(vec![]);
        let detail = vec![
            format!("Enter opens the full review of the {} match.", ms[self.round].label),
            "Esc in that review comes back to this summary.".to_owned(),
        ];
        rows.extend(detail_rows(&detail, w));
        rows.truncate(h);
        rows
    }

    // ------------------------------------------------------------- work on

    fn work_on(&mut self, w: usize, h: usize) -> Vec<Row> {
        if self.findings().is_empty() {
            let text = match self.data {
                Content::Match(_) => "  Nothing stood out in this match.",
                Content::Map(_) => "  Nothing repeats across these matches yet.",
            };
            return vec![(vec![(Style::fg(GREY), text.into())])];
        }
        self.finding = self.finding.min(self.findings().len() - 1);
        let findings = self.findings();
        let list_h = findings.len().min((h / 2).max(3));
        let mut rows = list_rows(
            findings
                .iter()
                .enumerate()
                .map(|(i, f)| format!("{:>2}  {}", i + 1, f.title)),
            self.finding,
            list_h,
        );
        rows.push(vec![]);
        rows.extend(evidence(&findings[self.finding], w));
        rows.truncate(h);
        rows
    }

    // -------------------------------------------------------------- rounds

    fn rounds_tab(&mut self, w: usize, h: usize) -> Vec<Row> {
        if self.rounds().is_empty() {
            return vec![(vec![(Style::fg(GREY), "  No rounds in this review.".into())])];
        }
        self.round = self.round.min(self.rounds().len() - 1);
        let rf = self.rounds();
        let detail = self.round_detail(&rf[self.round], w);
        let list_h = h.saturating_sub(detail.len() + 2).max(3).min(rf.len());
        let header = format!(
            "{:>3}  {:<4} {:<15} {:<7} {:<10} {:>2} {:>4}  {}",
            "rnd", "side", "result", "buy", "gun", "K", "dmg", "you"
        );
        let lines = rf.iter().map(|f| {
            let death = self.deaths().iter().find(|d| d.round == Some(f.round));
            let you = match death {
                Some(d) => format!(
                    "died to {}{}",
                    d.killer_agent,
                    d.nearest_callout
                        .as_ref()
                        .map(|c| format!(" at {c}"))
                        .unwrap_or_default()
                ),
                None => "survived".into(),
            };
            let first = match (f.first_kill, f.first_death) {
                (true, _) => " · first kill",
                (_, true) => " · first death",
                _ => "",
            };
            format!(
                "{:>3}  {:<4} {:<15} {:<7} {:<10} {:>2} {:>4.0}  {you}{first}",
                f.round + 1,
                side(f.side),
                result(f),
                f.economy.buy_type,
                f.economy.primary.as_deref().unwrap_or("-"),
                f.kills,
                f.damage_dealt
            )
        });
        let mut rows = vec![vec![(Style::fg(GREY), format!("    {header}"))]];
        rows.extend(list_rows(lines, self.round, list_h));
        rows.push(vec![]);
        rows.extend(detail);
        rows.truncate(h);
        rows
    }

    fn round_detail(&self, f: &RoundFact, w: usize) -> Vec<Row> {
        let mut lines = vec![format!(
            "Round {} · {} · {}",
            f.round + 1,
            match f.side {
                Some(Side::Attack) => "attack",
                Some(Side::Defense) => "defense",
                None => "side unknown",
            },
            match f.won {
                Some(true) => format!("won ({})", f.result.as_deref().unwrap_or("?")),
                Some(false) => format!("lost ({})", f.result.as_deref().unwrap_or("?")),
                None => "result unknown".into(),
            }
        )];
        let e = &f.economy;
        lines.push(format!(
            "Buy: {}{}, {} credits of guns and armour{}",
            e.buy_type,
            e.primary.as_deref().map(|g| format!(" with {g}")).unwrap_or_default(),
            e.loadout_value,
            e.credits_start
                .map(|c| format!(", {c} credits at the start"))
                .unwrap_or_default()
        ));
        let hits = f.hits_head + f.hits_body + f.hits_legs;
        lines.push(format!(
            "You: {} kill{}, {:.0} damage dealt, {:.0} taken, {} hits ({} head) from {} shots",
            f.kills,
            if f.kills == 1 { "" } else { "s" },
            f.damage_dealt,
            f.damage_taken,
            hits,
            f.hits_head,
            f.shots
        ));
        let mut extra = Vec::new();
        if f.first_kill {
            extra.push("first kill of the round".to_owned());
        }
        if f.first_death {
            extra.push("first death of the round".to_owned());
        }
        if let Some(n) = f.clutch_vs {
            extra.push(format!("clutch 1v{n}"));
        }
        if f.carried_spike {
            extra.push("carried the spike".into());
        }
        if f.planted {
            extra.push("planted".into());
        }
        if f.defused {
            extra.push("defused".into());
        }
        if f.ult_used {
            extra.push("used your ult".into());
        }
        if !f.casts.is_empty() {
            extra.push(format!(
                "casts {}",
                f.casts
                    .iter()
                    .map(|(k, n)| format!("{k}×{n}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        if !extra.is_empty() {
            lines.push(capitalise(&extra.join(", ")));
        }
        for d in self.deaths().iter().filter(|d| d.round == Some(f.round)) {
            let line = review::death_line(d);
            let what = line.split_once(": ").map_or(line.as_str(), |(_, rest)| rest);
            lines.push(format!("Died: {what}"));
        }
        detail_rows(&lines, w)
    }

    // -------------------------------------------------------------- deaths

    fn deaths_tab(&mut self, w: usize, h: usize) -> Vec<Row> {
        let n = self.all_deaths().len();
        if n == 0 {
            return vec![(vec![(Style::fg(GREY), "  No deaths to show.".into())])];
        }
        self.death = self.death.min(n - 1);
        let map = matches!(self.data, Content::Map(_));
        let d = self.all_deaths();
        let (r, f) = d[self.death];
        let mut detail_lines = death_detail(f);
        if map {
            detail_lines[0] = summary::death_line(r, f);
        }
        let detail = detail_rows(&detail_lines, w);
        let list_h = h.saturating_sub(detail.len() + 2).max(3).min(n);
        let lead = |r: &MatchReview| {
            if map {
                format!("{:<11}  ", summary::match_label(r))
            } else {
                String::new()
            }
        };
        let header = format!(
            "{}{:>3}  {:<10} {:<20} {:<16} {:>5}  {:<8} {}",
            if map {
                format!("{:<11}  ", "match")
            } else {
                String::new()
            },
            "rnd",
            "killer",
            "weapon",
            "where",
            "dist",
            "hit them",
            "traded"
        );
        let lines = d.iter().map(|(r, f)| {
            format!(
                "{}{:>3}  {:<10} {:<20} {:<16} {:>5}  {:<8} {}",
                lead(r),
                f.round.map_or("?".into(), |r| (r + 1).to_string()),
                f.killer_agent,
                f.context.killer_weapon.as_deref().unwrap_or("?"),
                f.nearest_callout.as_deref().unwrap_or("-"),
                f.dist_to_killer_m.map_or("-".into(), |m| format!("{m:.0} m")),
                if f.context.duel_hits_dealt > 0 {
                    plural(f.context.duel_hits_dealt, "hit")
                } else {
                    "no".into()
                },
                if f.traded_within_3s { "yes" } else { "" }
            )
        });
        let mut rows = vec![vec![(Style::fg(GREY), format!("    {header}"))]];
        rows.extend(list_rows(lines, self.death, list_h));
        rows.push(vec![]);
        rows.extend(detail);
        rows.truncate(h);
        rows
    }

    // ----------------------------------------------------------------- map

    fn map(&mut self, w: usize, h: usize) -> Vec<Row> {
        // Each cell is one pixel wide and two tall, so pixels come out square.
        let rows_h = h.min(w.saturating_sub(LEGEND_W + 4) / 2).max(4);
        let n = rows_h * 2;
        if self.map_cache.as_ref().is_none_or(|(size, _)| *size != n) {
            match self.minimap() {
                Ok(m) => self.map_cache = Some((n, m.downsample(n, n))),
                Err(e) => return vec![(vec![(Style::fg(GREY), format!("  No minimap: {e}"))])],
            }
        }
        let px = &self.map_cache.as_ref().expect("just filled").1;
        let opaque = |p: [u8; 4]| p[3] > 60;
        let dim = |p: [u8; 4]| Color::Rgb {
            r: (p[0] as f64 * 0.55) as u8,
            g: (p[1] as f64 * 0.55) as u8,
            b: (p[2] as f64 * 0.55) as u8,
        };
        let mut cells: Vec<Vec<(Style, String)>> = (0..rows_h)
            .map(|r| {
                (0..n)
                    .map(|c| {
                        let (top, bottom) = (px[2 * r * n + c], px[(2 * r + 1) * n + c]);
                        match (opaque(top), opaque(bottom)) {
                            (true, true) => (
                                Style {
                                    fg: Some(dim(top)),
                                    bg: Some(dim(bottom)),
                                    bold: false,
                                },
                                "▀".to_owned(),
                            ),
                            (true, false) => (Style::fg(dim(top)), "▀".to_owned()),
                            (false, true) => (Style::fg(dim(bottom)), "▄".to_owned()),
                            (false, false) => (Style::default(), " ".to_owned()),
                        }
                    })
                    .collect()
            })
            .collect();
        let at = |uv: [f64; 2]| -> Option<(usize, usize)> {
            let (c, r) = ((uv[0] * n as f64) as isize, (uv[1] * n as f64 / 2.0) as isize);
            (c >= 0 && r >= 0 && (c as usize) < n && (r as usize) < rows_h).then_some((r as usize, c as usize))
        };
        let mark = |cells: &mut Vec<Vec<(Style, String)>>, uv: [f64; 2], fg: Color, ch: &str| {
            if let Some((r, c)) = at(uv) {
                let bg = cells[r][c].0.bg.or(cells[r][c].0.fg.filter(|_| cells[r][c].1 != " "));
                cells[r][c] = (
                    Style {
                        fg: Some(fg),
                        bg,
                        bold: true,
                    },
                    ch.to_owned(),
                );
            }
        };
        // Site letters for orientation.
        if let Some(map) = self.map_info()
            && let Some(p) = map.projection
        {
            for c in map.callouts.iter().filter(|c| c.region == "Site") {
                let letter = c.super_region.chars().next().map(String::from).unwrap_or_default();
                mark(&mut cells, p.uv(c.pos_cm[0], c.pos_cm[1]), BRIGHT, &letter);
            }
        }
        let deaths = self.all_deaths();
        for (i, (_, d)) in deaths.iter().enumerate() {
            if i != self.death
                && let Some(uv) = d.minimap_uv
            {
                mark(&mut cells, uv, RED, "●");
            }
        }
        if let Some(uv) = deaths.get(self.death).and_then(|(_, d)| d.minimap_uv) {
            mark(&mut cells, uv, YELLOW, "◆");
        }

        // The legend beside the map.
        let mut legend: Vec<Row> = vec![
            vec![
                (Style::fg(RED).bold(), "● ".into()),
                (Style::default(), "your deaths".into()),
            ],
            vec![
                (Style::fg(YELLOW).bold(), "◆ ".into()),
                (Style::default(), "selected (↑↓)".into()),
            ],
            vec![
                (Style::fg(BRIGHT).bold(), "A ".into()),
                (Style::default(), "sites".into()),
            ],
            vec![],
        ];
        let spots: Vec<String> = match &self.data {
            Content::Match(d) => d
                .review
                .clusters
                .iter()
                .map(|c| {
                    format!(
                        "{} ×{}  (rounds {})",
                        c.nearest_callout.as_deref().unwrap_or("?"),
                        c.deaths,
                        c.rounds
                            .iter()
                            .map(|r| (r + 1).to_string())
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                })
                .collect(),
            Content::Map(d) => d
                .summary
                .spots
                .iter()
                .map(|s| {
                    format!(
                        "{} ×{} in {} matches",
                        s.callout.as_deref().unwrap_or("?"),
                        s.deaths,
                        s.matches
                    )
                })
                .collect(),
        };
        if !spots.is_empty() {
            legend.push(vec![(Style::default().bold(), "Repeated spots".into())]);
            for spot in spots {
                legend.push(vec![(Style::default(), spot)]);
            }
            legend.push(vec![]);
        }
        if let Some((r, d)) = deaths.get(self.death) {
            legend.push(vec![(
                Style::default().bold(),
                format!("Death {} of {}", self.death + 1, deaths.len()),
            )]);
            let line = match self.data {
                Content::Match(_) => review::death_line(d),
                Content::Map(_) => summary::death_line(r, d),
            };
            for piece in wrap(&line, LEGEND_W) {
                legend.push(vec![(Style::fg(GREY), piece)]);
            }
        }
        let missing = deaths.iter().filter(|(_, d)| d.minimap_uv.is_none()).count();
        if missing > 0 {
            legend.push(vec![]);
            legend.push(vec![(Style::fg(GREY), format!("{missing} deaths have no position"))]);
        }

        cells
            .into_iter()
            .enumerate()
            .map(|(r, row)| {
                let mut out: Row = vec![(Style::default(), "  ".into())];
                out.extend(row);
                out.push((Style::default(), "    ".into()));
                if let Some(l) = legend.get(r) {
                    out.extend(l.iter().cloned());
                }
                out
            })
            .collect()
    }
}

// --------------------------------------------------------------- helpers

/// `Review  Sova on Lotus · lost 12–14 · 18/21 · 148 dmg/round · 2026-10-18 00:22`
fn match_title(d: &ReviewData) -> Row {
    let o = &d.overview;
    let adr = o.players.iter().find(|p| p.me).map(|p| p.adr).unwrap_or(0.0);
    let result = if o.won > o.lost {
        ("won", GREEN)
    } else if o.won < o.lost {
        ("lost", RED)
    } else {
        ("drew", YELLOW)
    };
    let played = d
        .review
        .context
        .played_at
        .as_deref()
        .map(|t| t.get(..16).unwrap_or(t).replacen('T', " ", 1))
        .unwrap_or_default();
    vec![
        (Style::fg(ACCENT).bold(), "  Review  ".into()),
        (Style::default().bold(), format!("{} on {} ", o.agent, o.map)),
        (Style::fg(GREY), "· ".into()),
        (Style::fg(result.1).bold(), format!("{} {}–{}", result.0, o.won, o.lost)),
        (
            Style::fg(GREY),
            format!(" · {}/{} · {adr:.0} dmg/round · {played}", o.kills, o.deaths),
        ),
    ]
}

/// `Summary  Lotus · 5 matches (won 3, lost 2) · Sova ×3, Fade ×2`
fn map_title(d: &SummaryData) -> Row {
    let s = &d.summary;
    let n = s.matches.len();
    let won = s.matches.iter().filter(|m| m.won > m.lost).count();
    vec![
        (Style::fg(ACCENT).bold(), "  Summary  ".into()),
        (Style::default().bold(), format!("{} ", s.map)),
        (
            Style::fg(GREY),
            format!(
                "· {n} match{} (won {won}, lost {}) · {}",
                if n == 1 { "" } else { "es" },
                n - won,
                s.agents
            ),
        ),
    ]
}

fn title(text: &str) -> Row {
    vec![(Style::default().bold(), format!("  {text}"))]
}

/// `lines` as a selectable list `h` rows tall, the selection kept in view.
fn list_rows(lines: impl Iterator<Item = String>, sel: usize, h: usize) -> Vec<Row> {
    let lines: Vec<String> = lines.collect();
    let n = lines.len();
    let first = sel.saturating_sub(h.saturating_sub(1)).min(n.saturating_sub(h));
    lines
        .into_iter()
        .enumerate()
        .skip(first)
        .take(h)
        .map(|(i, l)| {
            if i == sel {
                vec![(Style::fg(ACCENT), format!("  ❯ {l}"))]
            } else {
                vec![(Style::default(), format!("    {l}"))]
            }
        })
        .collect()
}

/// A finding's evidence, wrapped, under a rule.
fn evidence(f: &Finding, w: usize) -> Vec<Row> {
    let mut lines: Vec<String> = f.evidence.clone();
    if !f.rounds.is_empty() {
        lines.push(format!(
            "Rounds: {}",
            f.rounds.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
        ));
    }
    detail_rows(&lines, w)
}

/// Detail lines under a thin rule, wrapped to the screen.
fn detail_rows(lines: &[String], w: usize) -> Vec<Row> {
    let text_w = w.saturating_sub(8).max(10);
    let mut rows = vec![vec![(Style::fg(GREY), format!("    {}", "─".repeat(text_w.min(60))))]];
    for (i, l) in lines.iter().enumerate() {
        let style = if i == 0 {
            Style::default().bold()
        } else {
            Style::default()
        };
        for piece in wrap(l, text_w) {
            rows.push(vec![(style, format!("    {piece}"))]);
        }
    }
    rows
}

fn death_detail(f: &DeathFact) -> Vec<String> {
    let c = &f.context;
    let mut lines = vec![review::death_line(f)];
    lines.push(format!(
        "{} s after the barrier{} · {} teammate{} and {} enem{} alive{}",
        c.since_barrier_s.map_or("?".into(), |s| format!("{s:.1}")),
        c.clock_left_s
            .map(|s| format!(", {s:.0} s left on the clock"))
            .unwrap_or_default(),
        f.teammates_alive,
        if f.teammates_alive == 1 { "" } else { "s" },
        f.enemies_alive,
        if f.enemies_alive == 1 { "y" } else { "ies" },
        if c.outnumbered { " (outnumbered)" } else { "" }
    ));
    lines.push(format!(
        "Holding {}{}{} · moving {}",
        c.weapon_in_hand.as_deref().unwrap_or("?"),
        c.magazine_ammo
            .zip(c.magazine_size)
            .map(|(a, s)| format!(" ({a}/{s} in the magazine)"))
            .unwrap_or_default(),
        c.inventory_value
            .map(|v| format!(", {v} credits of gear"))
            .unwrap_or_default(),
        f.speed_m_s.map_or("?".into(), |s| format!("{s:.1} m/s"))
    ));
    lines.push(format!(
        "The fight: you dealt {:.0} damage in {}, took {:.0} in {}{}{}",
        c.duel_damage_dealt,
        plural(c.duel_hits_dealt, "hit"),
        c.duel_damage_taken,
        plural(c.duel_hits_taken, "hit"),
        match c.hit_killer_first {
            Some(true) => " · you hit first",
            Some(false) => " · they hit first",
            None => "",
        },
        c.duel_ms
            .map(|m| format!(" · {m} ms from first hit to death"))
            .unwrap_or_default()
    ));
    let mut aim = Vec::new();
    if let Some(a) = f.crosshair_offset_to_killer_deg {
        aim.push(format!("crosshair {a:.0}° from the killer"));
    }
    if let Some(v) = c.vertical_aim_offset_deg {
        aim.push(format!("{v:+.1}° vertically"));
    }
    if let Some(r) = &c.killing_blow_region {
        aim.push(format!(
            "killed by a {r} hit{}",
            if c.killing_blow_wallbang { " through a wall" } else { "" }
        ));
    }
    if !aim.is_empty() {
        lines.push(capitalise(&aim.join(", ")));
    }
    let near = if f.nearest_teammates.is_empty() {
        "no teammate position known".to_owned()
    } else {
        f.nearest_teammates
            .iter()
            .map(|t| format!("{} {:.0} m", t.agent, t.dist_m))
            .collect::<Vec<_>>()
            .join(", ")
    };
    lines.push(format!(
        "Nearest teammates: {near}{}",
        if f.traded_within_3s {
            " · traded within 3 s"
        } else {
            " · not traded"
        }
    ));
    let mut flags = Vec::new();
    if c.ult_ready == Some(true) {
        flags.push("ult ready");
    }
    if c.carrying_spike {
        flags.push("carrying the spike");
    }
    if c.post_plant {
        flags.push("after the plant");
    }
    if f.first_death_of_round {
        flags.push("first death of the round");
    }
    if c.same_spot_as_own_kill {
        flags.push("within 5 m of your earlier kill");
    }
    if !flags.is_empty() {
        lines.push(capitalise(&flags.join(", ")));
    }
    lines
}

fn side(s: Option<Side>) -> &'static str {
    match s {
        Some(Side::Attack) => "atk",
        Some(Side::Defense) => "def",
        None => "?",
    }
}

fn result(f: &RoundFact) -> String {
    match f.won {
        Some(true) => format!("W {}", f.result.as_deref().unwrap_or("")),
        Some(false) => format!("L {}", f.result.as_deref().unwrap_or("")),
        None => "?".into(),
    }
}

fn plural(n: u32, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}
