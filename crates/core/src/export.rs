//! Reads a `vrfkit export` directory: `manifest.json`, `events.parquet`, and
//! the player-pawn rows of `movement.parquet`.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use arrow_array::{Array, ArrayRef, RecordBatch, cast::AsArray, types};
use arrow_schema::DataType;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::Deserialize;

use crate::fields::{Fields, load_fields};

/// Positions at or below this on both x and z are vrfkit's "hidden actor" parking spot.
const HIDDEN_BELOW_CM: f32 = -40_000.0;

#[derive(Debug, Clone)]
pub struct Player {
    /// Account UUID.
    pub subject: String,
    /// BombPlayerState actor.
    pub actor_net_guid: u32,
    /// Last non-zero `SpawnedCharacter`: the join key to events and movement.
    pub pawn: u32,
    /// Every pawn this player had (a reconnect adds one).
    pub pawns: Vec<u32>,
    /// Agent UUID from `playerLoadouts`, lower-case.
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub match_id: String,
    pub replay_build: String,
    pub duration_ms: u64,
    pub map_path: Option<String>,
    pub timestamp_ticks: Option<i64>,
    pub players: Vec<Player>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    RoundStarted { round: u32 },
    Death { killer: u32, victim: u32 },
    UltimateUsed,
    SpikePlanted,
    SpikeDefused,
    SpikeExploded,
    SwitchTeams,
    Other,
}

#[derive(Debug, Clone, Copy)]
pub struct Event {
    pub time_ms: u32,
    pub kind: EventKind,
}

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub time_ms: u32,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub vel: [f32; 3],
}

impl Sample {
    pub fn is_hidden(&self) -> bool {
        self.pos[0] <= HIDDEN_BELOW_CM && self.pos[2] <= HIDDEN_BELOW_CM
    }
}

/// Movement samples for one pawn, sorted by `time_ms` (stable, so rows that
/// share a millisecond keep their file order).
#[derive(Debug, Clone, Default)]
pub struct Track {
    pub samples: Vec<Sample>,
}

impl Track {
    /// The last sample at or before `t_ms`, if it is no older than `max_age_ms`
    /// and not a hidden-actor position.
    pub fn at(&self, t_ms: u32, max_age_ms: u32) -> Option<&Sample> {
        let idx = self.samples.partition_point(|s| s.time_ms <= t_ms);
        let s = self.samples.get(idx.checked_sub(1)?)?;
        if t_ms - s.time_ms > max_age_ms || s.is_hidden() {
            return None;
        }
        Some(s)
    }
}

#[derive(Debug, Clone)]
pub struct Export {
    pub manifest: Manifest,
    /// Sorted by time (stable).
    pub events: Vec<Event>,
    /// Keyed by pawn.
    pub tracks: HashMap<u32, Track>,
    /// Typed rows from `fields.parquet` (damage, economy, abilities, rounds, ...).
    pub fields: Fields,
    pub actors: Actors,
}

/// From `actors.parquet`: what class each actor is, and where some spawned.
#[derive(Debug, Clone, Default)]
pub struct Actors {
    /// Actor GUID -> class path (first channel open that names one).
    pub class: HashMap<u32, String>,
    /// Channel opens with a spawn position: (time_ms, actor, class, pos cm).
    pub spawns: Vec<(u32, u32, String, [f64; 3])>,
    /// Last channel close per actor: the actor is gone after it (dormancy is not a close).
    pub closed_at: HashMap<u32, u32>,
}

impl Actors {
    /// Last path segment of the actor's class, e.g. `AssaultRifle_AK.AssaultRifle_AK_C`.
    pub fn class_leaf(&self, guid: u32) -> Option<&str> {
        self.class.get(&guid).map(|c| c.rsplit('/').next().unwrap_or(c))
    }
}

impl Export {
    pub fn load(dir: &Path) -> Result<Self> {
        if !dir.is_dir() {
            bail!(
                "{} is not a directory; expected a `vrfkit export` output dir",
                dir.display()
            );
        }
        let manifest = load_manifest(&dir.join("manifest.json"))?;
        let events = load_events(&dir.join("events.parquet"))?;
        let pawns: HashSet<u32> = manifest.players.iter().flat_map(|p| p.pawns.iter().copied()).collect();
        let tracks = load_movement(&dir.join("movement.parquet"), &pawns)?;
        let fields = load_fields(&dir.join("fields.parquet"))?;
        let actors = load_actors(&dir.join("actors.parquet"))?;
        Ok(Export {
            manifest,
            events,
            tracks,
            fields,
            actors,
        })
    }

    pub fn player_by_pawn(&self, pawn: u32) -> Option<&Player> {
        self.manifest.players.iter().find(|p| p.pawns.contains(&pawn))
    }
}

// ---------------------------------------------------------------- manifest

#[derive(Deserialize)]
struct RawManifest {
    #[serde(default)]
    friendly_name: Option<String>,
    #[serde(default)]
    source_file: Option<String>,
    #[serde(default)]
    replay_build: String,
    #[serde(default)]
    duration_ms: u64,
    #[serde(default)]
    timestamp_ticks: Option<i64>,
    #[serde(default)]
    level_names_and_times: Vec<RawLevel>,
    #[serde(default)]
    game_specific_data: Vec<String>,
    #[serde(default)]
    players: Vec<RawPlayer>,
}

#[derive(Deserialize)]
struct RawLevel {
    name: String,
}

#[derive(Deserialize)]
struct RawPlayer {
    actor_net_guid: u32,
    subject: String,
    character_net_guid: u32,
    #[serde(default)]
    character_net_guids: Vec<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawGameData {
    #[serde(default)]
    player_loadouts: Vec<RawLoadout>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLoadout {
    subject: String,
    character_id: String,
}

fn load_manifest(path: &Path) -> Result<Manifest> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw: RawManifest = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;

    // playerLoadouts lives in one of the game_specific_data JSON strings (index 1 so far).
    let mut agents: HashMap<String, String> = HashMap::new();
    for blob in &raw.game_specific_data {
        if let Ok(gd) = serde_json::from_str::<RawGameData>(blob) {
            for l in gd.player_loadouts {
                agents.insert(l.subject.to_lowercase(), l.character_id.to_lowercase());
            }
        }
    }

    let players = raw
        .players
        .into_iter()
        .filter(|p| p.character_net_guid != 0)
        .map(|p| {
            let mut pawns: Vec<u32> = p.character_net_guids.into_iter().filter(|&g| g != 0).collect();
            if !pawns.contains(&p.character_net_guid) {
                pawns.push(p.character_net_guid);
            }
            Player {
                agent_id: agents.get(&p.subject.to_lowercase()).cloned(),
                subject: p.subject.to_lowercase(),
                actor_net_guid: p.actor_net_guid,
                pawn: p.character_net_guid,
                pawns,
            }
        })
        .collect::<Vec<_>>();
    if players.is_empty() {
        bail!("manifest lists no players with a pawn; is this an unsupported replay?");
    }

    let match_id = raw
        .friendly_name
        .or(raw.source_file.map(|s| s.trim_end_matches(".vrf").to_owned()))
        .unwrap_or_else(|| "unknown".into());
    Ok(Manifest {
        match_id,
        replay_build: raw.replay_build,
        duration_ms: raw.duration_ms,
        map_path: raw.level_names_and_times.into_iter().next().map(|l| l.name),
        timestamp_ticks: raw.timestamp_ticks,
        players,
    })
}

// ---------------------------------------------------------------- parquet helpers

pub(crate) fn open_reader(
    path: &Path,
    columns: &[&str],
) -> Result<parquet::arrow::arrow_reader::ParquetRecordBatchReader> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)
        .with_context(|| format!("reading parquet {}", path.display()))?;
    let schema = builder.parquet_schema();
    let mut indices = Vec::with_capacity(columns.len());
    for &name in columns {
        let idx = (0..schema.num_columns())
            .find(|&i| schema.column(i).name() == name)
            .ok_or_else(|| {
                anyhow!(
                    "{} has no column `{name}`; vrfkit output format may have changed",
                    path.display()
                )
            })?;
        indices.push(idx);
    }
    let mask = ProjectionMask::leaves(schema, indices);
    Ok(builder.with_projection(mask).with_batch_size(65_536).build()?)
}

pub(crate) fn column(batch: &RecordBatch, name: &str, ty: &DataType) -> Result<ArrayRef> {
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| anyhow!("missing column `{name}`"))?;
    if col.data_type() == ty {
        return Ok(col.clone());
    }
    arrow_cast::cast(col, ty)
        .with_context(|| format!("column `{name}` ({}) is not convertible to {ty}", col.data_type()))
}

// ---------------------------------------------------------------- events

fn load_events(path: &Path) -> Result<Vec<Event>> {
    let mut out = Vec::new();
    for batch in open_reader(path, &["group", "time1", "word0", "word1"])? {
        let batch = batch?;
        let group = column(&batch, "group", &DataType::Utf8)?;
        let group = group.as_string::<i32>();
        let time = column(&batch, "time1", &DataType::UInt32)?;
        let time = time.as_primitive::<types::UInt32Type>();
        let w0 = column(&batch, "word0", &DataType::UInt32)?;
        let w0 = w0.as_primitive::<types::UInt32Type>();
        let w1 = column(&batch, "word1", &DataType::UInt32)?;
        let w1 = w1.as_primitive::<types::UInt32Type>();
        for i in 0..batch.num_rows() {
            let word = |a: &arrow_array::UInt32Array| (!a.is_null(i)).then(|| a.value(i));
            let name = if group.is_null(i) { "" } else { group.value(i) };
            let kind = match (name, word(w0), word(w1)) {
                ("roundStarted", Some(r), _) => EventKind::RoundStarted { round: r },
                ("characterDeath", Some(k), Some(v)) => EventKind::Death { killer: k, victim: v },
                ("characterUltimateUsed", _, _) => EventKind::UltimateUsed,
                ("spikePlanted", _, _) => EventKind::SpikePlanted,
                ("spikeDefused", _, _) => EventKind::SpikeDefused,
                ("spikeExploded", _, _) => EventKind::SpikeExploded,
                ("switchTeams", _, _) => EventKind::SwitchTeams,
                _ => EventKind::Other,
            };
            out.push(Event {
                time_ms: time.value(i),
                kind,
            });
        }
    }
    out.sort_by_key(|e| e.time_ms);
    Ok(out)
}

// ---------------------------------------------------------------- movement

fn load_movement(path: &Path, pawns: &HashSet<u32>) -> Result<HashMap<u32, Track>> {
    const COLS: [&str; 10] = [
        "time_ms",
        "character_net_guid",
        "pos_x",
        "pos_y",
        "pos_z",
        "yaw",
        "pitch",
        "vel_x",
        "vel_y",
        "vel_z",
    ];
    let mut tracks: HashMap<u32, Track> = pawns.iter().map(|&p| (p, Track::default())).collect();
    for batch in open_reader(path, &COLS)? {
        let batch = batch?;
        let t = column(&batch, "time_ms", &DataType::UInt32)?;
        let t = t.as_primitive::<types::UInt32Type>();
        let g = column(&batch, "character_net_guid", &DataType::UInt32)?;
        let g = g.as_primitive::<types::UInt32Type>();
        let f: Vec<ArrayRef> = COLS[2..]
            .iter()
            .map(|c| column(&batch, c, &DataType::Float32))
            .collect::<Result<_>>()?;
        let f: Vec<&arrow_array::Float32Array> = f.iter().map(|a| a.as_primitive::<types::Float32Type>()).collect();
        for i in 0..batch.num_rows() {
            let Some(track) = tracks.get_mut(&g.value(i)) else {
                continue;
            };
            track.samples.push(Sample {
                time_ms: t.value(i),
                pos: [f[0].value(i), f[1].value(i), f[2].value(i)],
                yaw: f[3].value(i),
                pitch: f[4].value(i),
                vel: [f[5].value(i), f[6].value(i), f[7].value(i)],
            });
        }
    }
    for track in tracks.values_mut() {
        track.samples.sort_by_key(|s| s.time_ms);
    }
    Ok(tracks)
}

// ---------------------------------------------------------------- actors

fn load_actors(path: &Path) -> Result<Actors> {
    let mut out = Actors::default();
    for batch in open_reader(
        path,
        &[
            "time_ms",
            "actor_net_guid",
            "event",
            "class_path",
            "spawn_x",
            "spawn_y",
            "spawn_z",
        ],
    )? {
        let batch = batch?;
        let t = column(&batch, "time_ms", &DataType::UInt32)?;
        let t = t.as_primitive::<types::UInt32Type>();
        let g = column(&batch, "actor_net_guid", &DataType::UInt32)?;
        let g = g.as_primitive::<types::UInt32Type>();
        let class = column(&batch, "class_path", &DataType::Utf8)?;
        let class = class.as_string::<i32>();
        let event = column(&batch, "event", &DataType::Utf8)?;
        let event = event.as_string::<i32>();
        let xyz: Vec<ArrayRef> = ["spawn_x", "spawn_y", "spawn_z"]
            .iter()
            .map(|c| column(&batch, c, &DataType::Float64))
            .collect::<Result<_>>()?;
        let xyz: Vec<&arrow_array::Float64Array> = xyz.iter().map(|a| a.as_primitive::<types::Float64Type>()).collect();
        for i in 0..batch.num_rows() {
            if event.is_valid(i) && event.value(i) == "close" {
                out.closed_at.insert(g.value(i), t.value(i));
            }
            if class.is_null(i) {
                continue;
            }
            let c = class.value(i);
            out.class.entry(g.value(i)).or_insert_with(|| c.to_owned());
            if xyz.iter().all(|a| a.is_valid(i)) {
                out.spawns.push((
                    t.value(i),
                    g.value(i),
                    c.to_owned(),
                    [xyz[0].value(i), xyz[1].value(i), xyz[2].value(i)],
                ));
            }
        }
    }
    Ok(out)
}
