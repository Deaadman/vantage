//! What the full-screen panels share: Abalone's widgets (the frame,
//! paragraphs, fields, lists, keys), rank colours, and the minimap drawn in
//! half blocks (`abalone::image`).

use abalone::image::{self, Image};
pub use abalone::widgets::{
    Item, body_height, bullet, field, frame, heading, is_close, list_rows, para, rule_line, scrolled, step, step_of,
    steps, tab_key,
};
pub use abalone::{pad, rgb};
use crossterm::style::Color;
use vantage_core::catalog::MapInfo;
use vantage_core::minimap::Minimap;

use crate::shell::{BRIGHT, GREY, Row, Style};

/// A rank in its in-game colour; grey when unset.
pub fn rank_color(rank: Option<&str>) -> Color {
    rank.and_then(vantage_core::ranks::colour).map_or(GREY, rgb)
}

/// The minimap in half blocks, `rows_h` rows tall and twice that many
/// columns, with marks on it. Each cell is one pixel wide and two tall.
/// It's turned so the attackers' spawn is at the bottom.
pub struct MapCells {
    cells: Vec<Row>,
    n: usize,
    /// Quarter turns clockwise from the image, for placing marks.
    turns: u8,
}

impl MapCells {
    pub fn new(px: &[[u8; 4]], n: usize, map: Option<&MapInfo>) -> Self {
        let turns = map.map_or(0, MapInfo::attacker_turns);
        // Dimmed so marks stand out; faint pixels are left out.
        let cells = Image::new(n, n, px.to_vec()).turn(turns).cells(|p| {
            (p[3] > 60).then(|| Color::Rgb {
                r: (p[0] as f64 * 0.55) as u8,
                g: (p[1] as f64 * 0.55) as u8,
                b: (p[2] as f64 * 0.55) as u8,
            })
        });
        let mut m = MapCells { cells, n, turns };
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

    /// `ch` at a minimap position (`uv` on the image as it comes, before turning).
    pub fn mark(&mut self, uv: [f64; 2], fg: Color, ch: &str) {
        let uv = vantage_core::catalog::turn_uv(uv, self.turns);
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

    /// The map with `legend` beside it, one legend row per map row. Legend
    /// rows past the bottom of the map carry on under it, in the same column.
    pub fn beside(self, legend: Vec<Row>) -> Vec<Row> {
        let indent = " ".repeat(2 + self.n + 4);
        let map_h = self.cells.len();
        let mut rows: Vec<Row> = self
            .cells
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
            .collect();
        for l in legend.into_iter().skip(map_h) {
            let mut out: Row = vec![(Style::default(), indent.clone())];
            out.extend(l);
            rows.push(out);
        }
        rows
    }
}

/// The minimap shrunk to a size, kept between frames.
#[derive(Default)]
pub struct MapCache(image::Cache);

impl MapCache {
    /// Map cells `rows_h` tall, or why there's no map.
    pub fn cells(
        &mut self,
        minimap: &Result<Minimap, String>,
        map: Option<&MapInfo>,
        rows_h: usize,
    ) -> Result<MapCells, String> {
        let n = rows_h * 2;
        let img = self
            .0
            .get_with(n, n, || minimap.as_ref().map(Minimap::image).map_err(Clone::clone))?;
        Ok(MapCells::new(&img.px, n, map))
    }
}

/// `rows` cut to `h`; when any are cut, the last row says there's more.
pub fn fit(mut rows: Vec<Row>, h: usize) -> Vec<Row> {
    if rows.len() > h && h > 0 {
        rows.truncate(h - 1);
        rows.push(vec![(Style::fg(GREY), "    ↓ more · make the window taller".into())]);
    }
    rows
}

/// How many rows of a list of `n` to show, at most `max`, so `around` other
/// rows still fit in `h`; never fewer than 3 (or `n`).
pub fn list_h(n: usize, max: usize, around: usize, h: usize) -> usize {
    h.saturating_sub(around).clamp(3.min(n), max.min(n).max(1))
}

/// Map rows for a body `h` tall beside a legend `legend_w` wide.
pub fn map_rows_h(w: usize, h: usize, legend_w: usize) -> usize {
    h.min(w.saturating_sub(legend_w + 8) / 2).max(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_rows_say_so() {
        let rows: Vec<Row> = (0..10).map(|i| vec![(Style::default(), i.to_string())]).collect();
        assert_eq!(fit(rows.clone(), 10).len(), 10);
        let cut = fit(rows, 4);
        assert_eq!(cut.len(), 4);
        assert!(cut[3][0].1.contains("↓ more"));
        assert_eq!(list_h(8, 8, 20, 24), 4);
        assert_eq!(list_h(8, 8, 30, 24), 3);
        assert_eq!(list_h(2, 8, 30, 24), 2);
        assert_eq!(list_h(8, 8, 0, 24), 8);
    }

    #[test]
    fn marks_turn_with_the_map() {
        // One lit pixel on a 4x4 map, at column 3, row 0 (top right).
        let n = 4;
        let mut px = vec![[0, 0, 0, 0]; n * n];
        px[3] = [255, 255, 255, 255];
        for turns in 0..4 {
            let turned = Image::new(n, n, px.clone()).turn(turns).px;
            let lit = turned.iter().position(|p| p[3] == 255).unwrap();
            let [u, v] = vantage_core::catalog::turn_uv([3.5 / 4.0, 0.5 / 4.0], turns);
            assert_eq!(
                (lit % n, lit / n),
                ((u * n as f64) as usize, (v * n as f64) as usize),
                "{turns} turns"
            );
        }
    }
}
