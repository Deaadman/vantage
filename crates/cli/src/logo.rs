//! The launch logo: three stacked chevrons, like a rank badge. They appear
//! from the bottom up, a pulse runs up through them once, and the logo stays
//! on its last frame for the rest of the session.

use crossterm::style::Color;

use crate::shell::{BRIGHT, RED, Row, Style};

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

type Pixels = [[Option<Color>; W]; H];

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
    let mut px = [[None; W]; H];
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

/// The logo at `frame`, as three rows of half blocks.
pub fn rows(frame: usize) -> [Row; 3] {
    let px = pixels(frame);
    std::array::from_fn(|k| {
        (0..W)
            .map(|c| match (px[2 * k][c], px[2 * k + 1][c]) {
                (None, None) => (Style::default(), " ".to_owned()),
                (Some(t), None) => (Style::fg(t), "▀".to_owned()),
                (None, Some(b)) => (Style::fg(b), "▄".to_owned()),
                (Some(t), Some(b)) if t == b => (Style::fg(t), "█".to_owned()),
                (Some(t), Some(b)) => (
                    Style {
                        bg: Some(b),
                        ..Style::fg(t)
                    },
                    "▀".to_owned(),
                ),
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(r: &Row) -> String {
        r.iter().map(|(_, s)| s.as_str()).collect()
    }

    #[test]
    fn ends_on_three_chevrons() {
        let last = rows(FRAMES - 1).map(|r| text(&r));
        assert_eq!(last, ["▀▄▀▄▀▄▀▄▀▄▀", "  ▀▄▀▄▀▄▀  ", "    ▀▄▀    "]);
        // The top chevron is the brightest, at the tip of the badge.
        assert_eq!(rows(FRAMES - 1)[0][5].0, Style::fg(RED));
        // Later frames are the last one: the logo stays put.
        assert_eq!(rows(FRAMES + 40), rows(FRAMES - 1));
        // It starts with only the bottom chevron.
        assert_eq!(text(&rows(0)[0]), "▀▄       ▄▀");
    }
}
