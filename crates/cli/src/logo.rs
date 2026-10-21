//! The launch logo: three stacked chevrons, like a rank badge. They appear
//! from the bottom up, a pulse runs up through them once, and the logo stays
//! on its last frame for the rest of the session.

use crossterm::style::Color;
use shellkit::{Logo, Row, half_blocks};

use crate::shell::{BRIGHT, RED};

/// Frames at the shell's tick (80 ms): 1.2 s.
pub const FRAMES: usize = 15;
const W: usize = 11;
/// Six pixel rows, two to a terminal row with half blocks.
const H: usize = 6;
/// Bottom chevron to top: deep red up to bright.
const SHADES: [Color; 3] = [
    Color::Rgb { r: 137, g: 40, b: 48 },
    Color::Rgb { r: 192, g: 48, b: 61 },
    RED,
];

type Pixels = Vec<Vec<Option<Color>>>;

/// Chevron `i`: a V of single pixels with its point `2i` rows up from the bottom.
fn chevron(px: &mut Pixels, i: usize, c: Color) {
    let lift = 2 * i;
    for d in 0..H - lift {
        let r = H - 1 - lift - d;
        px[r][W / 2 - d] = Some(c);
        px[r][W / 2 + d] = Some(c);
    }
}

fn pixels(frame: usize) -> Pixels {
    let f = frame.min(FRAMES - 1);
    let mut px = vec![vec![None; W]; H];
    for (i, shade) in SHADES.into_iter().enumerate() {
        // Each lands white, then the pulse passes up through them at 10, 11, 12.
        if f < 3 * i {
            continue;
        }
        let white = f == 3 * i || f == 10 + i;
        chevron(&mut px, i, if white { BRIGHT } else { shade });
    }
    px
}

/// The logo, for the shell's header.
#[derive(Debug, Clone, Copy, Default)]
pub struct Chevrons;

impl Logo for Chevrons {
    fn frames(&self) -> usize {
        FRAMES
    }

    fn rows(&self, frame: usize) -> Vec<Row> {
        half_blocks(&pixels(frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shellkit::{Style, row_text};

    #[test]
    fn ends_on_three_chevrons() {
        let rows = |f| Chevrons.rows(f);
        let last: Vec<String> = rows(FRAMES - 1).iter().map(row_text).collect();
        assert_eq!(last, ["▀▄▀▄▀▄▀▄▀▄▀", "  ▀▄▀▄▀▄▀  ", "    ▀▄▀    "]);
        // The top chevron is the brightest, at the tip of the badge.
        assert_eq!(rows(FRAMES - 1)[0][5].0, Style::fg(RED));
        // Later frames are the last one: the logo stays put.
        assert_eq!(rows(FRAMES + 40), rows(FRAMES - 1));
        // It starts with only the bottom chevron.
        assert_eq!(row_text(&rows(0)[0]), "▀▄       ▄▀");
    }
}
