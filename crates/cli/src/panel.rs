//! What the full-screen panels share: the frame (rule, title, tabs, body,
//! footer), wrapped paragraphs and labelled fields, selectable lists, keys,
//! and the minimap drawn in half blocks.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Color;
use vantage_core::catalog::MapInfo;
use vantage_core::minimap::Minimap;

use crate::review_view::GREEN;
use crate::shell::{ACCENT, BRIGHT, GREY, Row, Style, wrap};

/// Width of a field's label column.
pub const LABEL_W: usize = 16;

/// `text` padded with spaces to `n` columns.
pub fn pad(text: &str, n: usize) -> String {
    let k = text.chars().count();
    if k >= n {
        text.to_owned()
    } else {
        format!("{text}{}", " ".repeat(n - k))
    }
}

/// The panel: a rule, the title, the tabs, the body sized to fit, and a hint.
pub fn frame(w: usize, h: usize, title: Row, tabs: Option<(&[&str], usize)>, body: Vec<Row>, hint: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = vec![
        vec![(Style::fg(ACCENT), "─".repeat(w.saturating_sub(1)))],
        title,
        vec![],
    ];
    if let Some((names, tab)) = tabs {
        rows.push(tabs_row(names, tab));
        rows.push(vec![]);
    }
    let footer = [vec![], vec![(Style::fg(GREY), format!("  {hint}"))]];
    let body_h = h.saturating_sub(rows.len() + footer.len()).max(1);
    let mut body = body;
    body.truncate(body_h);
    body.resize(body_h, vec![]);
    rows.extend(body);
    rows.extend(footer);
    rows.truncate(h);
    rows
}

/// Height left for a body under `frame`'s title (and tabs).
pub fn body_height(h: usize, tabs: bool) -> usize {
    h.saturating_sub(if tabs { 7 } else { 5 }).max(1)
}

pub fn tabs_row(names: &[&str], tab: usize) -> Row {
    let mut row: Row = vec![(Style::default(), " ".into())];
    for (i, name) in names.iter().enumerate() {
        row.push((Style::default(), " ".into()));
        let style = if i == tab {
            Style {
                fg: Some(Color::Black),
                bg: Some(BRIGHT),
                bold: false,
            }
        } else {
            Style::default()
        };
        row.push((style, format!(" {name} ")));
    }
    row
}

/// `  Text` in bold, optionally coloured.
pub fn heading(text: &str, color: Option<Color>) -> Row {
    let style = color.map_or(Style::default(), Style::fg).bold();
    vec![(style, format!("  {text}"))]
}

/// A paragraph wrapped to the screen, indented `ind` columns.
pub fn para(text: &str, ind: usize, w: usize, style: Style) -> Vec<Row> {
    wrap(text, w.saturating_sub(ind + 2).max(10))
        .into_iter()
        .map(|p| vec![(style, format!("{}{p}", " ".repeat(ind)))])
        .collect()
}

/// A thin rule under a list, before the selected item's detail.
pub fn rule_line(w: usize) -> Row {
    vec![(
        Style::fg(GREY),
        format!("    {}", "─".repeat(w.saturating_sub(8).clamp(10, 60))),
    )]
}

/// `label` in grey, then `text` wrapped beside it.
pub fn field(label: &str, text: &str, w: usize, style: Style) -> Vec<Row> {
    wrap(text, w.saturating_sub(4 + LABEL_W + 3).max(10))
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            vec![
                (
                    Style::fg(GREY),
                    format!("    {}", pad(if i == 0 { label } else { "" }, LABEL_W)),
                ),
                (style, p),
            ]
        })
        .collect()
}

/// Numbered steps beside a label.
pub fn steps(label: &str, items: &[String], w: usize) -> Vec<Row> {
    let mut out = Vec::new();
    for (n, t) in items.iter().enumerate() {
        for (i, p) in wrap(t, w.saturating_sub(4 + LABEL_W + 6).max(10))
            .into_iter()
            .enumerate()
        {
            out.push(vec![
                (
                    Style::fg(GREY),
                    format!("    {}", pad(if n == 0 && i == 0 { label } else { "" }, LABEL_W)),
                ),
                (
                    Style::fg(GREEN),
                    if i == 0 { format!("{}. ", n + 1) } else { "   ".into() },
                ),
                (Style::default(), p),
            ]);
        }
    }
    out
}

/// A bullet: `    + text`, wrapped under itself.
pub fn bullet(mark: &str, color: Color, text: &str, w: usize) -> Vec<Row> {
    wrap(text, w.saturating_sub(10).max(10))
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            vec![
                (
                    Style::fg(color).bold(),
                    if i == 0 {
                        format!("    {mark} ")
                    } else {
                        "      ".into()
                    },
                ),
                (Style::default(), p),
            ]
        })
        .collect()
}

/// One selectable line: its text, and an optional tail in its own colour.
pub struct Item {
    pub text: String,
    pub tail: Option<(String, Color)>,
}

impl Item {
    pub fn new(text: impl Into<String>) -> Self {
        Item {
            text: text.into(),
            tail: None,
        }
    }

    pub fn tail(mut self, text: impl Into<String>, color: Color) -> Self {
        self.tail = Some((text.into(), color));
        self
    }
}

/// `items` as a list `h` rows tall, the selection marked and kept in view.
pub fn list_rows(items: Vec<Item>, sel: usize, h: usize) -> Vec<Row> {
    let n = items.len();
    let first = sel.saturating_sub(h.saturating_sub(1)).min(n.saturating_sub(h));
    items
        .into_iter()
        .enumerate()
        .skip(first)
        .take(h)
        .map(|(i, it)| {
            let on = i == sel;
            let mut row: Row = vec![(
                if on { Style::fg(ACCENT) } else { Style::default() },
                format!("{}{}", if on { "  ❯ " } else { "    " }, it.text),
            )];
            if let Some((t, c)) = it.tail {
                row.push((Style::fg(if on { ACCENT } else { c }), t));
            }
            row
        })
        .collect()
}

/// `lines` from `scroll`, `h` tall, the last row saying how many more.
pub fn scrolled(scroll: &mut usize, lines: Vec<Row>, h: usize) -> Vec<Row> {
    let max = lines.len().saturating_sub(h);
    *scroll = (*scroll).min(max);
    let mut rows: Vec<Row> = lines.into_iter().skip(*scroll).take(h).collect();
    if *scroll < max
        && let Some(last) = rows.last_mut()
    {
        *last = vec![(Style::fg(GREY), format!("    ↓ {} more", max - *scroll))];
    }
    rows
}

pub fn is_close(key: &KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    matches!(key.code, KeyCode::Esc)
        || (key.code == KeyCode::Char('q') && !ctrl)
        || (ctrl && key.code == KeyCode::Char('c'))
}

/// ←→, Tab and the number keys move between `n` tabs; true when they did.
pub fn tab_key(tab: &mut usize, key: &KeyEvent, n: usize) -> bool {
    match key.code {
        KeyCode::Left | KeyCode::BackTab => *tab = (*tab + n - 1) % n,
        KeyCode::Right | KeyCode::Tab => *tab = (*tab + 1) % n,
        KeyCode::Char(c) if c.is_ascii_digit() && !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let i = c as usize - '1' as usize;
            if c == '0' || i >= n {
                return false;
            }
            *tab = i;
        }
        _ => return false,
    }
    true
}

/// ↑↓, PgUp/PgDn, Home/End as a step.
pub fn step_of(key: &KeyEvent) -> isize {
    match key.code {
        KeyCode::Up => -1,
        KeyCode::Down => 1,
        KeyCode::PageUp => -10,
        KeyCode::PageDown => 10,
        KeyCode::Home => -10_000,
        KeyCode::End => 10_000,
        _ => 0,
    }
}

pub fn step(v: &mut usize, by: isize, n: usize) {
    *v = (*v as isize + by).clamp(0, n.saturating_sub(1) as isize) as usize;
}

pub fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb { r, g, b }
}

/// A rank in its in-game colour; grey when unset.
pub fn rank_color(rank: Option<&str>) -> Color {
    rank.and_then(vantage_core::ranks::colour).map_or(GREY, rgb)
}

/// The minimap in half blocks, `rows_h` rows tall and twice that many
/// columns, with marks on it. Each cell is one pixel wide and two tall.
pub struct MapCells {
    cells: Vec<Vec<(Style, String)>>,
    n: usize,
}

impl MapCells {
    pub fn new(px: &[[u8; 4]], n: usize, map: Option<&MapInfo>) -> Self {
        let rows_h = n / 2;
        let opaque = |p: [u8; 4]| p[3] > 60;
        let dim = |p: [u8; 4]| Color::Rgb {
            r: (p[0] as f64 * 0.55) as u8,
            g: (p[1] as f64 * 0.55) as u8,
            b: (p[2] as f64 * 0.55) as u8,
        };
        let cells = (0..rows_h)
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
        let mut m = MapCells { cells, n };
        // Site letters for orientation.
        if let Some(map) = map
            && let Some(p) = map.projection
        {
            for c in map.callouts.iter().filter(|c| c.region == "Site") {
                let letter = c.super_region.chars().next().map(String::from).unwrap_or_default();
                m.mark(p.uv(c.pos_cm[0], c.pos_cm[1]), BRIGHT, &letter);
            }
        }
        m
    }

    pub fn mark(&mut self, uv: [f64; 2], fg: Color, ch: &str) {
        let (c, r) = ((uv[0] * self.n as f64) as isize, (uv[1] * self.n as f64 / 2.0) as isize);
        if c < 0 || r < 0 || c as usize >= self.n || r as usize >= self.cells.len() {
            return;
        }
        let cell = &mut self.cells[r as usize][c as usize];
        let bg = cell.0.bg.or(cell.0.fg.filter(|_| cell.1 != " "));
        *cell = (
            Style {
                fg: Some(fg),
                bg,
                bold: true,
            },
            ch.to_owned(),
        );
    }

    /// The map with `legend` beside it, one legend row per map row.
    pub fn beside(self, legend: Vec<Row>) -> Vec<Row> {
        self.cells
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

/// The minimap shrunk to a size, kept between frames.
#[derive(Default)]
pub struct MapCache(Option<(usize, Vec<[u8; 4]>)>);

impl MapCache {
    /// Map cells `rows_h` tall, or why there's no map.
    pub fn cells(
        &mut self,
        minimap: &Result<Minimap, String>,
        map: Option<&MapInfo>,
        rows_h: usize,
    ) -> Result<MapCells, String> {
        let n = rows_h * 2;
        if self.0.as_ref().is_none_or(|(size, _)| *size != n) {
            let m = minimap.as_ref().map_err(Clone::clone)?;
            self.0 = Some((n, m.downsample(n, n)));
        }
        Ok(MapCells::new(&self.0.as_ref().expect("just filled").1, n, map))
    }
}

/// Map rows for a body `h` tall beside a legend `legend_w` wide.
pub fn map_rows_h(w: usize, h: usize, legend_w: usize) -> usize {
    h.min(w.saturating_sub(legend_w + 8) / 2).max(4)
}
