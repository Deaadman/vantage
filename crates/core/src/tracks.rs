//! Player movement downsampled to 4 Hz, kept after the raw export is deleted
//! (for a later death-map view). Never put into an AI-facing output.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{Float32Array, RecordBatch, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

use crate::deaths::MAX_SAMPLE_AGE_MS;
use crate::game::Game;

pub const STEP_MS: u32 = 250;

/// Writes `time_ms, pawn, pos_x, pos_y, pos_z, yaw` (cm, degrees) for every
/// player at each 250 ms step where their position is known. Returns the row count.
pub fn write_tracks(game: &Game, path: &Path) -> Result<usize> {
    let (mut t, mut pawn, mut x, mut y, mut z, mut yaw) = (vec![], vec![], vec![], vec![], vec![], vec![]);
    let end = game.export.manifest.duration_ms.min(u32::MAX as u64) as u32;
    for p in game.players() {
        let Some(track) = game.export.tracks.get(&p.pawn) else {
            continue;
        };
        for step in (0..=end).step_by(STEP_MS as usize) {
            if let Some(s) = track.at(step, MAX_SAMPLE_AGE_MS) {
                t.push(step);
                pawn.push(p.pawn);
                x.push(s.pos[0]);
                y.push(s.pos[1]);
                z.push(s.pos[2]);
                yaw.push(s.yaw);
            }
        }
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("time_ms", DataType::UInt32, false),
        Field::new("pawn", DataType::UInt32, false),
        Field::new("pos_x", DataType::Float32, false),
        Field::new("pos_y", DataType::Float32, false),
        Field::new("pos_z", DataType::Float32, false),
        Field::new("yaw", DataType::Float32, false),
    ]));
    let rows = t.len();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(t)),
            Arc::new(UInt32Array::from(pawn)),
            Arc::new(Float32Array::from(x)),
            Arc::new(Float32Array::from(y)),
            Arc::new(Float32Array::from(z)),
            Arc::new(Float32Array::from(yaw)),
        ],
    )?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(3)?))
        .build();
    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(&batch)?;
    writer.close()?;
    Ok(rows)
}
