//! The `/trends` panel, drawn over the whole screen until Esc: one chart a
//! tab of your stats across matches, oldest on the left, and the death spots
//! that repeat. Enter opens the selected match's review.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Color;
use valreview_core::MatchReview;
use valreview_core::review::{PlayerLine, percent, tally};
use valreview_core::trends::{SpotAcrossMatches, spots_across_matches};

use crate::review_view::{GREEN, ReviewAction};
use crate::shell::{ACCENT, BRIGHT, GREY, RED, Row, Style, YELLOW};

const RULE: Color = Color::Rgb { r: 110, g: 110, b: 110 };
const ORANGE: Color = Color::Rgb { r: 240, g: 150, b: 90 };
const CYAN: Color = Color::Rgb { r: 110, g: 200, b: 230 };
/// Bar colours, by how often you played the agent.
const AGENT_COLOURS: [Color; 5] = [ACCENT, GREEN, YELLOW, ORANGE, CYAN];
const RANGES: [(&str, usize); 3] = [("All matches", 0), ("Last 10", 10), ("Last 5", 5)];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// One match as the charts need it.
pub struct TrendMatch {
    pub id: String,
    /// `10-19 21:04`.
    pub label: String,
    /// `19 Oct`.
    pub day: String,
    pub map: String,
    pub agent: String,
    pub won: usize,
    pub lost: usize,
    pub line: PlayerLine,
}

impl TrendMatch {
    pub fn new(r: &MatchReview) -> Self {
        let played = r.context.played_at.as_deref().unwrap_or("");
        let day = match (
            played.get(5..7).and_then(|m| m.parse::<usize>().ok()),
            played.get(8..10),
        ) {
            (Some(m @ 1..=12), Some(d)) => format!("{} {}", d.trim_start_matches('0'), MONTHS[m - 1]),
            _ => "?".into(),
        };
        TrendMatch {
            id: r.context.match_id.clone(),
            label: valreview_core::summary::match_label(r),
            day,
            map: r.map_name().trim_matches('`').to_owned(),
            agent: r.agent.clone(),
            won: r.round_facts.iter().filter(|f| f.won == Some(true)).count(),
            lost: r.round_facts.iter().filter(|f| f.won == Some(false)).count(),
            line: PlayerLine::from_review(r, &r.subject, &[]),
        }
    }
}

/// Everything `/trends` shows: my reviews, oldest first.
pub struct TrendsData {
    pub matches: Vec<TrendMatch>,
    pub reviews: Vec<MatchReview>,
}

struct Metric {
    tab: &'static str,
    title: &'static str,
    get: fn(&TrendMatch) -> Option<f64>,
    fmt: fn(f64) -> String,
    /// A share, compared in points.
    pct: bool,
    /// Counts: axis steps stay whole.
    whole: bool,
    higher_is_better: bool,
}

const METRICS: [Metric; 5] = [
    Metric {
        tab: "Damage",
        title: "Damage per round",
        get: |m| Some(m.line.adr),
        fmt: |v| format!("{v:.0}"),
        pct: false,
        whole: false,
        higher_is_better: true,
    },
    Metric {
        tab: "K/D",
        title: "Kills per death",
        get: |m| Some(m.line.kills as f64 / m.line.deaths.max(1) as f64),
        fmt: |v| format!("{v:.2}"),
        pct: false,
        whole: false,
        higher_is_better: true,
    },
    Metric {
        tab: "Headshots",
        title: "Headshot rate",
        get: |m| m.line.hs,
        fmt: percent,
        pct: true,
        whole: false,
        higher_is_better: true,
    },
    Metric {
        tab: "KST",
        title: "Rounds with a kill, survived or traded",
        get: |m| m.line.kst,
        fmt: percent,
        pct: true,
        whole: false,
        higher_is_better: true,
    },
    Metric {
        tab: "First deaths",
        title: "First deaths a match",
        get: |m| Some(m.line.first_deaths as f64),
        fmt: |v| {
            if v.fract() == 0.0 {
                format!("{v:.0}")
            } else {
                format!("{v:.1}")
            }
        },
        pct: false,
        whole: true,
        higher_is_better: false,
    },
];

/// The axis top: four even steps of 1, 2, 2.5 or 5 (no 2.5 for counts).
fn nice_max(v: f64, whole: bool) -> f64 {
    if v <= 0.0 {
        return 4.0;
    }
    let raw = v / 4.0;
    let e = 10f64.powf(raw.log10().floor());
    let steps: &[f64] = if whole {
        &[1.0, 2.0, 5.0, 10.0]
    } else {
        &[1.0, 2.0, 2.5, 5.0, 10.0]
    };
    for m in steps {
        let step = m * e;
        if step >= raw && (!whole || step >= 1.0) {
            return 4.0 * step;
        }
    }
    40.0 * e
}

/// `rows` merged into as few pieces as their styles allow.
fn merge(cells: impl IntoIterator<Item = (Style, char)>) -> Row {
    let mut row: Row = Vec::new();
    for (style, c) in cells {
        match row.last_mut() {
            Some((s, text)) if *s == style => text.push(c),
            _ => row.push((style, c.to_string())),
        }
    }
    row
}

pub struct TrendsPanel {
    data: TrendsData,
    tab: usize,
    range: usize,
    /// Selected match in the range; None is the newest.
    sel: Option<usize>,
    /// Agent -> bar colour.
    colours: Vec<(String, Color)>,
    spots: Option<(usize, Vec<SpotAcrossMatches>)>,
}

impl TrendsPanel {
    pub fn new(data: TrendsData) -> Self {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for m in &data.matches {
            match counts.iter_mut().find(|(a, _)| *a == m.agent) {
                Some((_, n)) => *n += 1,
                None => counts.push((m.agent.clone(), 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let colours = counts
            .into_iter()
            .enumerate()
            .map(|(i, (a, _))| (a, AGENT_COLOURS.get(i).copied().unwrap_or(GREY)))
            .collect();
        TrendsPanel {
            data,
            tab: 0,
            range: 0,
            sel: None,
            colours,
            spots: None,
        }
    }

    fn colour(&self, agent: &str) -> Color {
        self.colours.iter().find(|(a, _)| a == agent).map_or(GREY, |c| c.1)
    }

    /// Where the range starts in the matches.
    fn start(&self) -> usize {
        match RANGES[self.range].1 {
            0 => 0,
            n => self.data.matches.len().saturating_sub(n),
        }
    }

    fn shown(&self) -> &[TrendMatch] {
        &self.data.matches[self.start()..]
    }

    fn selected(&self) -> usize {
        let n = self.shown().len();
        self.sel.map_or(n.saturating_sub(1), |s| s.min(n.saturating_sub(1)))
    }

    pub fn key(&mut self, key: KeyEvent) -> ReviewAction {
        let n_tabs = METRICS.len() + 1;
        let n = self.shown().len();
        let sel = self.selected();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return ReviewAction::Close,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return ReviewAction::Close,
            KeyCode::Left | KeyCode::BackTab => self.tab = (self.tab + n_tabs - 1) % n_tabs,
            KeyCode::Right | KeyCode::Tab => self.tab = (self.tab + 1) % n_tabs,
            KeyCode::Char(c @ '1'..='6') => self.tab = (c as usize - '1' as usize).min(n_tabs - 1),
            KeyCode::Char('r') => {
                self.range = (self.range + 1) % RANGES.len();
                self.sel = None;
            }
            KeyCode::Up => self.sel = Some(sel.saturating_sub(1)),
            KeyCode::Down => self.sel = Some((sel + 1).min(n.saturating_sub(1))),
            KeyCode::Home => self.sel = Some(0),
            KeyCode::End => self.sel = None,
            KeyCode::Enter if self.tab < METRICS.len() => {
                if let Some(m) = self.shown().get(sel) {
                    return ReviewAction::Open(m.id.clone());
                }
            }
            _ => {}
        }
        ReviewAction::None
    }

    /// The panel, exactly `h` rows for a `w`-column screen.
    pub fn rows(&mut self, w: usize, h: usize) -> Vec<Row> {
        let shown = self.shown();
        let n = shown.len();
        let (first, last) = (
            shown.first().map_or("?", |m| &m.day),
            shown.last().map_or("?", |m| &m.day),
        );
        let mut rows: Vec<Row> = vec![
            vec![(Style::fg(ACCENT), "─".repeat(w.saturating_sub(1)))],
            vec![
                (Style::fg(ACCENT).bold(), "  Trends  ".into()),
                (
                    Style::default().bold(),
                    format!("{n} match{} ", if n == 1 { "" } else { "es" }),
                ),
                (
                    Style::fg(GREY),
                    format!("· {first} – {last} · {}", tally(shown.iter().map(|m| m.agent.clone()))),
                ),
            ],
            vec![],
        ];
        let mut tabs: Row = vec![(Style::default(), " ".into())];
        for (i, name) in METRICS.iter().map(|m| m.tab).chain(["Death spots"]).enumerate() {
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
        let chart = self.tab < METRICS.len();
        let footer = vec![
            vec![],
            vec![(
                Style::fg(GREY),
                format!(
                    "  ←→ or 1-{} tabs{} · r changes the range · Esc closes",
                    METRICS.len() + 1,
                    if chart {
                        " · ↑↓ pick a match · Enter opens it"
                    } else {
                        ""
                    }
                ),
            )],
        ];
        let body_h = h.saturating_sub(rows.len() + footer.len()).max(1);
        let mut body = if chart {
            self.chart(&METRICS[self.tab], w, body_h)
        } else {
            self.spots_tab(w, body_h)
        };
        body.resize(body_h, vec![]);
        rows.extend(body);
        rows.extend(footer);
        rows.truncate(h);
        rows
    }

    fn range_row(&self) -> Row {
        let mut row: Row = vec![(Style::default(), "  ".into())];
        for (i, (label, _)) in RANGES.iter().enumerate() {
            if i > 0 {
                row.push((Style::fg(GREY), " · ".into()));
            }
            let style = if i == self.range {
                Style::fg(ACCENT).bold()
            } else {
                Style::fg(GREY)
            };
            row.push((style, (*label).into()));
        }
        row
    }

    /// One outlined bar a match, the value above it and the date below.
    fn chart(&self, m: &Metric, w: usize, h: usize) -> Vec<Row> {
        let shown = self.shown();
        let n = shown.len().max(1);
        let sel = self.selected();
        let vals: Vec<Option<f64>> = shown.iter().map(m.get).collect();
        let mut out: Vec<Row> = vec![
            self.range_row(),
            vec![],
            vec![(Style::default().bold(), format!("  {}", m.title))],
        ];
        // Plot rows: a multiple of four so the four axis steps land on rows.
        let ch = ((h.saturating_sub(11)).min(16) / 4 * 4).max(4);
        let max = nice_max(vals.iter().flatten().copied().fold(0.0, f64::max), m.whole);
        let ticks: Vec<String> = (0..=4).map(|k| (m.fmt)(max * k as f64 / 4.0)).collect();
        let yw = ticks.iter().map(|t| t.chars().count()).max().unwrap_or(1);
        let plot_w = w.saturating_sub(yw + 6).max(10);
        let slot = (plot_w / n).clamp(4, 9);
        let bw = slot - 1;
        let used = slot * n + 1;
        // Row 0 holds the value above the tallest bars; rows 1..=ch are the plot.
        let blank = (Style::default(), ' ');
        let mut grid = vec![vec![blank; used]; ch + 1];
        let put = |grid: &mut Vec<Vec<(Style, char)>>, r: usize, c: usize, s: Style, ch2: char| {
            if let Some(cell) = grid.get_mut(r).and_then(|row| row.get_mut(c)) {
                *cell = (s, ch2);
            }
        };
        for (i, (mtch, v)) in shown.iter().zip(&vals).enumerate() {
            let x0 = i * slot + 1;
            let style = Style::fg(self.colour(&mtch.agent));
            let label = v.map_or("-".to_owned(), m.fmt);
            let top = match v {
                Some(v) if *v > 0.0 => {
                    let hgt = ((v / max * ch as f64).round() as usize).clamp(1, ch);
                    let top = ch - hgt + 1;
                    put(&mut grid, top, x0, style, '╭');
                    put(&mut grid, top, x0 + bw - 1, style, '╮');
                    for c in x0 + 1..x0 + bw - 1 {
                        put(&mut grid, top, c, style, '─');
                    }
                    for r in top + 1..=ch {
                        put(&mut grid, r, x0, style, '│');
                        put(&mut grid, r, x0 + bw - 1, style, '│');
                    }
                    top
                }
                _ => {
                    for c in x0..x0 + bw {
                        put(&mut grid, ch, c, style, '─');
                    }
                    ch
                }
            };
            let lw = label.chars().count();
            if lw <= bw || i == sel {
                let ls = if i == sel {
                    Style::fg(BRIGHT).bold()
                } else {
                    Style::fg(GREY)
                };
                let start = x0 + bw.saturating_sub(lw) / 2;
                for (j, c) in label.chars().enumerate() {
                    put(&mut grid, top.saturating_sub(1), start + j, ls, c);
                }
            }
        }
        let step = ch / 4;
        for (r, cells) in grid.into_iter().enumerate() {
            // The value at the top of row r is (ch - r + 1) / ch of the axis.
            let k = (ch + 1 - r) / step;
            let tick = r > 0 && (ch + 1 - r).is_multiple_of(step) && k <= 4;
            let mut row: Row = vec![
                (
                    Style::fg(GREY),
                    format!("  {:>yw$} ", if tick { ticks[k].as_str() } else { "" }),
                ),
                (
                    Style::fg(RULE),
                    if r == 0 {
                        " "
                    } else if tick {
                        "┤"
                    } else {
                        "│"
                    }
                    .into(),
                ),
            ];
            row.extend(merge(cells));
            out.push(row);
        }
        out.push(vec![
            (Style::fg(GREY), format!("  {:>yw$} ", ticks[0])),
            (Style::fg(RULE), format!("└{}", "─".repeat(used))),
        ]);
        let mut dates = vec![blank; used];
        for (i, mtch) in shown.iter().enumerate() {
            if slot < 6 && i != sel && (shown.len() - 1 - i) % 2 == 1 {
                continue;
            }
            let lab: String = mtch.label.chars().take(5).collect();
            let start = i * slot + 1 + bw.saturating_sub(5) / 2;
            let style = if i == sel {
                Style::fg(BRIGHT).bold()
            } else {
                Style::fg(GREY)
            };
            for (j, c) in lab.chars().enumerate() {
                if let Some(cell) = dates.get_mut(start + j) {
                    *cell = (style, c);
                }
            }
        }
        let mut row: Row = vec![(Style::default(), " ".repeat(yw + 4))];
        row.extend(merge(dates));
        out.push(row);
        let mut legend: Row = vec![(Style::default(), " ".repeat(yw + 4))];
        let mut agents: Vec<&str> = Vec::new();
        for mtch in shown {
            if !agents.contains(&mtch.agent.as_str()) {
                agents.push(&mtch.agent);
            }
        }
        for (i, a) in agents.iter().enumerate() {
            if i > 0 {
                legend.push((Style::fg(GREY), " · ".into()));
            }
            legend.push((Style::fg(self.colour(a)), "● ".into()));
            legend.push((Style::default(), (*a).into()));
        }
        out.push(legend);
        out.push(vec![]);
        out.push(self.compare(m, &vals));
        if let Some(mtch) = shown.get(sel) {
            let (res, colour) = if mtch.won > mtch.lost {
                ("W", GREEN)
            } else if mtch.won < mtch.lost {
                ("L", RED)
            } else {
                ("D", YELLOW)
            };
            out.push(vec![
                (Style::fg(ACCENT), "  ❯ ".into()),
                (
                    Style::default(),
                    format!("{} · {} · {} · ", mtch.label, mtch.map, mtch.agent),
                ),
                (Style::fg(colour).bold(), format!("{res} {}–{}", mtch.won, mtch.lost)),
                (
                    Style::fg(GREY),
                    format!(
                        " · {} {}",
                        m.title.to_lowercase(),
                        vals[sel].map_or("-".to_owned(), m.fmt)
                    ),
                ),
            ]);
        }
        out
    }

    /// The last half of the range against the half before it.
    fn compare(&self, m: &Metric, vals: &[Option<f64>]) -> Row {
        let k = vals.len() / 2;
        let avg = |v: &[Option<f64>]| {
            let xs: Vec<f64> = v.iter().flatten().copied().collect();
            (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / xs.len() as f64)
        };
        let (Some(recent), Some(before)) = (
            avg(&vals[vals.len() - k..]),
            avg(&vals[vals.len().saturating_sub(2 * k)..vals.len() - k]),
        ) else {
            return vec![(
                Style::fg(GREY),
                "  Play a few more matches to compare recent ones with earlier ones.".into(),
            )];
        };
        if k < 2 {
            return vec![(
                Style::fg(GREY),
                "  Play a few more matches to compare recent ones with earlier ones.".into(),
            )];
        }
        let delta = recent - before;
        let same = delta.abs() < if m.pct { 0.005 } else { 0.05 };
        let better = if m.higher_is_better { delta > 0.0 } else { delta < 0.0 };
        let amount = if m.pct {
            format!("{:.0} pts", delta.abs() * 100.0)
        } else {
            (m.fmt)(delta.abs())
        };
        vec![
            (Style::default(), format!("  Last {k} matches ")),
            (Style::default().bold(), (m.fmt)(recent)),
            (
                Style::fg(GREY),
                format!(" against {} for the {k} before  ", (m.fmt)(before)),
            ),
            if same {
                (Style::fg(GREY), "no change".into())
            } else {
                (
                    Style::fg(if better { GREEN } else { RED }).bold(),
                    format!("{} {amount}", if delta > 0.0 { "▲" } else { "▼" }),
                )
            },
        ]
    }

    /// Spots where you died in two or more matches, as red bars.
    fn spots_tab(&mut self, w: usize, h: usize) -> Vec<Row> {
        if self.spots.as_ref().is_none_or(|(r, _)| *r != self.range) {
            let mut spots: Vec<SpotAcrossMatches> = spots_across_matches(&self.data.reviews[self.start()..])
                .into_iter()
                .filter(|s| s.matches >= 2)
                .collect();
            spots.sort_by(|a, b| b.deaths.cmp(&a.deaths).then(b.matches.cmp(&a.matches)));
            self.spots = Some((self.range, spots));
        }
        let spots = &self.spots.as_ref().expect("just filled").1;
        let mut out: Vec<Row> = vec![
            self.range_row(),
            vec![],
            vec![
                (Style::default().bold(), "  Where you keep dying".into()),
                (Style::fg(GREY), "  (spots with deaths in 2 or more matches)".into()),
            ],
            vec![],
        ];
        if spots.is_empty() {
            out.push(vec![(Style::fg(GREY), "    Nothing repeats yet.".into())]);
            return out;
        }
        let top = spots[0].deaths.max(1);
        let bar_w = w.saturating_sub(60).clamp(10, 36);
        for s in spots.iter().take(h.saturating_sub(out.len()).max(1)) {
            let name = format!("{} ({})", s.callout.as_deref().unwrap_or("a spot"), s.map);
            let len = ((s.deaths as f64 / top as f64 * bar_w as f64).round() as usize).max(1);
            out.push(vec![
                (Style::default(), format!("    {name:<26}")),
                (Style::fg(RED), "█".repeat(len)),
                (Style::default(), " ".repeat(bar_w - len + 2)),
                (Style::default().bold(), format!("{:>2}", s.deaths)),
                (
                    Style::fg(GREY),
                    format!(
                        " deaths in {} match{}",
                        s.matches,
                        if s.matches == 1 { "" } else { "es" }
                    ),
                ),
            ]);
        }
        out
    }
}

/// `valreview trends` outside the shell: each chart's comparison as text.
pub fn print_trends(data: &TrendsData) {
    let n = data.matches.len();
    outln!(
        "{n} match{} · {}",
        if n == 1 { "" } else { "es" },
        tally(data.matches.iter().map(|m| m.agent.clone()))
    );
    let k = n / 2;
    for m in &METRICS {
        let vals: Vec<f64> = data.matches.iter().filter_map(m.get).collect();
        let avg = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
        if k >= 2 && vals.len() >= 2 * k {
            let (before, recent) = (&vals[vals.len() - 2 * k..vals.len() - k], &vals[vals.len() - k..]);
            outln!(
                "  {:<40} last {k}: {}  the {k} before: {}",
                m.title,
                (m.fmt)(avg(recent)),
                (m.fmt)(avg(before))
            );
        } else {
            outln!("  {:<40} {}", m.title, (m.fmt)(avg(&vals)));
        }
    }
    let mut spots: Vec<SpotAcrossMatches> = spots_across_matches(&data.reviews)
        .into_iter()
        .filter(|s| s.matches >= 2)
        .collect();
    spots.sort_by(|a, b| b.deaths.cmp(&a.deaths).then(b.matches.cmp(&a.matches)));
    if !spots.is_empty() {
        outln!();
        outln!("Where you keep dying");
        for s in spots.iter().take(5) {
            outln!(
                "  {} ({}): {} deaths in {} matches",
                s.callout.as_deref().unwrap_or("a spot"),
                s.map,
                s.deaths,
                s.matches
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_steps_are_even() {
        assert_eq!(nice_max(171.0, false), 200.0);
        assert_eq!(nice_max(0.26, false), 0.4);
        assert_eq!(nice_max(1.5, false), 2.0);
        // Counts never step by 2.5 or less than 1.
        assert_eq!(nice_max(5.0, true), 8.0);
        assert_eq!(nice_max(2.0, true), 4.0);
        assert_eq!(nice_max(0.0, true), 4.0);
    }
}
