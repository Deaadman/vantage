//! Map minimaps from valorant-api.com, cached as PNGs in the data dir and
//! decoded to RGBA for drawing in the terminal. Minimap coordinates (`uv` in
//! [0, 1]², from the catalog's projection) index this image directly.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::catalog::MapInfo;

#[derive(Debug, Clone)]
pub struct Minimap {
    pub width: usize,
    pub height: usize,
    /// Row-major RGBA, 4 bytes a pixel.
    pub rgba: Vec<u8>,
}

impl Minimap {
    /// The cached image for `map`, downloading it the first time.
    pub fn load(data_dir: &Path, map: &MapInfo) -> Result<Self> {
        let path = cache_path(data_dir, map);
        if !path.is_file() {
            let url = map
                .minimap_image
                .as_deref()
                .with_context(|| format!("valorant-api.com has no minimap for {}", map.name))?;
            std::fs::create_dir_all(path.parent().expect("cache path has a parent"))?;
            crate::install::download(url, &path)?;
        }
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        Self::decode(&bytes).with_context(|| format!("decoding {}", path.display()))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut decoder = png::Decoder::new(Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info()?;
        let mut buf = vec![0; reader.output_buffer_size().context("image too large")?];
        let info = reader.next_frame(&mut buf)?;
        let (w, h) = (info.width as usize, info.height as usize);
        let px = &buf[..info.buffer_size()];
        let rgba: Vec<u8> = match info.color_type {
            png::ColorType::Rgba => px.to_vec(),
            png::ColorType::Rgb => px
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect(),
            png::ColorType::GrayscaleAlpha => px
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[0], p[0], p[1]])
                .collect(),
            png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            other => bail!("unsupported PNG colour type {other:?}"),
        };
        if rgba.len() != w * h * 4 {
            bail!("PNG size does not match its pixels");
        }
        Ok(Minimap {
            width: w,
            height: h,
            rgba,
        })
    }

    /// The image shrunk to `w`×`h` by averaging, colour weighted by alpha:
    /// `[r, g, b, a]` per cell, row-major.
    pub fn downsample(&self, w: usize, h: usize) -> Vec<[u8; 4]> {
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            let (y0, y1) = span(y, h, self.height);
            for x in 0..w {
                let (x0, x1) = span(x, w, self.width);
                let (mut r, mut g, mut b, mut a, mut n) = (0f64, 0f64, 0f64, 0f64, 0f64);
                for py in y0..y1 {
                    for px in x0..x1 {
                        let i = (py * self.width + px) * 4;
                        let alpha = self.rgba[i + 3] as f64;
                        r += self.rgba[i] as f64 * alpha;
                        g += self.rgba[i + 1] as f64 * alpha;
                        b += self.rgba[i + 2] as f64 * alpha;
                        a += alpha;
                        n += 1.0;
                    }
                }
                out.push(if a > 0.0 {
                    [(r / a) as u8, (g / a) as u8, (b / a) as u8, (a / n) as u8]
                } else {
                    [0, 0, 0, 0]
                });
            }
        }
        out
    }
}

/// Source pixels `[start, end)` for cell `i` of `cells` over `size` pixels.
fn span(i: usize, cells: usize, size: usize) -> (usize, usize) {
    let start = i * size / cells;
    let end = ((i + 1) * size / cells).max(start + 1).min(size);
    (start.min(size.saturating_sub(1)), end)
}

pub fn cache_path(data_dir: &Path, map: &MapInfo) -> PathBuf {
    data_dir.join("minimaps").join(format!("{}.png", map.uuid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_and_downsamples() {
        // A 2x2 RGBA PNG: opaque white, transparent, opaque black, half-alpha red.
        let mut bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut bytes, 2, 2);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 255, 255, 0, 0, 128])
                .unwrap();
        }
        let m = Minimap::decode(&bytes).unwrap();
        assert_eq!((m.width, m.height), (2, 2));
        let full = m.downsample(2, 2);
        assert_eq!(full[0], [255, 255, 255, 255]);
        assert_eq!(full[1][3], 0);
        let one = m.downsample(1, 1);
        // Transparent pixels add no colour: white, black and red weighted by alpha.
        assert!(one[0][3] > 0 && one[0][0] > one[0][1]);
    }
}
