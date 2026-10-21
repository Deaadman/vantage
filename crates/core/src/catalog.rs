//! Agent, map, weapon and season data from valorant-api.com, cached as
//! `catalog.json`. Response shapes were checked against the live API on
//! 2026-10-20 (game version 13.08).
//!
//! Offline use: [`Catalog::load_or_builtin`] falls back to the built-in agent
//! table when there is no cache, so names still resolve; maps, callouts and
//! weapons need a `vantage sync`.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::agents::BUILTIN_AGENTS;

pub const CATALOG_SCHEMA_VERSION: u32 = 2;
const API_BASE: &str = "https://valorant-api.com/v1";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Catalog {
    pub schema_version: u32,
    /// RFC 3339 time of the sync, or `None` for the built-in fallback.
    pub fetched_at: Option<String>,
    /// valorant-api.com `/v1/version` `version`, e.g. `13.08.00.5698904`.
    pub game_version: Option<String>,
    pub agents: Vec<Agent>,
    pub maps: Vec<MapInfo>,
    pub weapons: Vec<Weapon>,
    pub gear: Vec<Gear>,
    pub seasons: Vec<Season>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub uuid: String,
    pub name: String,
    pub role: Option<String>,
    pub icon: Option<String>,
    pub playable: bool,
    pub abilities: Vec<Ability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ability {
    /// `Ability1`, `Ability2`, `Grenade`, `Ultimate`, `Passive`.
    pub slot: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapInfo {
    pub uuid: String,
    pub name: String,
    /// Matches `manifest.level_names_and_times[0].name`, e.g. `/Game/Maps/Jam/Jam`.
    pub map_url: String,
    /// Minimap image (`displayIcon`).
    pub minimap_image: Option<String>,
    /// `None` when the API publishes zero multipliers (modes without a minimap).
    pub projection: Option<Projection>,
    pub callouts: Vec<Callout>,
}

/// World cm to minimap [0, 1]². The axes cross: `u` comes from `y`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Projection {
    pub x_multiplier: f64,
    pub y_multiplier: f64,
    pub x_scalar_to_add: f64,
    pub y_scalar_to_add: f64,
}

impl Projection {
    pub fn uv(&self, pos_x: f64, pos_y: f64) -> [f64; 2] {
        [
            pos_y * self.x_multiplier + self.x_scalar_to_add,
            pos_x * self.y_multiplier + self.y_scalar_to_add,
        ]
    }
}

/// A named point in world cm (same space and axes as `movement.parquet`;
/// verified on Lotus, where a player's nearest callout is a median 7 m away
/// and 26 m with the axes swapped).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Callout {
    pub region: String,
    pub super_region: String,
    pub pos_cm: [f64; 3],
}

impl Callout {
    /// `A Site`, `Mid Top`, ... as the in-game minimap labels it.
    pub fn label(&self) -> String {
        if self.super_region.is_empty() {
            self.region.clone()
        } else {
            format!("{} {}", self.super_region, self.region)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Weapon {
    pub uuid: String,
    pub name: String,
    /// e.g. `EEquippableCategory::Rifle`.
    pub category: String,
    /// Asset folder under `/Game`, e.g. `Equippables/Guns/Rifles/AK`; matches replay class paths.
    pub asset_folder: Option<String>,
    pub cost: Option<u32>,
    pub magazine_size: Option<u32>,
    pub fire_rate: Option<f64>,
    pub damage_ranges: Vec<DamageRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DamageRange {
    pub start_m: f64,
    pub end_m: f64,
    pub head: f64,
    pub body: f64,
    pub leg: f64,
}

/// Armour and other shop gear.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gear {
    pub uuid: String,
    pub name: String,
    pub cost: Option<u32>,
    /// Prefix of the replay's class leaf, e.g. `HeavyArmor` for `HeavyArmorItem_C`.
    pub class_prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Season {
    pub uuid: String,
    pub name: String,
    pub kind: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub parent_uuid: Option<String>,
}

/// How a catalog was obtained, for one-line status messages.
pub enum CatalogSource {
    Cache,
    BuiltinNoCache,
    BuiltinBadCache(String),
}

impl Catalog {
    /// Agent names and roles only, from the compiled-in table.
    pub fn builtin() -> Self {
        Catalog {
            schema_version: CATALOG_SCHEMA_VERSION,
            agents: BUILTIN_AGENTS
                .iter()
                .map(|(uuid, name, role)| Agent {
                    uuid: (*uuid).into(),
                    name: (*name).into(),
                    role: Some((*role).into()),
                    icon: None,
                    playable: true,
                    abilities: Vec::new(),
                })
                .collect(),
            ..Default::default()
        }
    }

    /// Whole days since the sync, if the sync time is known.
    pub fn age_days(&self, now_unix: i64) -> Option<i64> {
        let t = parse_rfc3339(self.fetched_at.as_deref()?)?;
        Some((now_unix - t).max(0) / 86_400)
    }

    pub fn is_builtin(&self) -> bool {
        self.fetched_at.is_none()
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        // Check the version before the shape, so an old cache gets a clear message.
        #[derive(Deserialize)]
        struct Version {
            schema_version: u32,
        }
        let v: Version = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if v.schema_version != CATALOG_SCHEMA_VERSION {
            bail!(
                "{} is catalog schema {} but this version of Vantage uses {CATALOG_SCHEMA_VERSION}",
                path.display(),
                v.schema_version
            );
        }
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Never fails: a missing or unreadable cache gives the built-in table.
    pub fn load_or_builtin(path: &Path) -> (Self, CatalogSource) {
        if !path.is_file() {
            return (Self::builtin(), CatalogSource::BuiltinNoCache);
        }
        match Self::load(path) {
            Ok(c) => (c, CatalogSource::Cache),
            Err(e) => (Self::builtin(), CatalogSource::BuiltinBadCache(format!("{e:#}"))),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    pub fn agent(&self, uuid: &str) -> Option<&Agent> {
        // The API lists some agents twice (KAY/O has a non-playable twin with
        // another uuid); uuids themselves are unique.
        self.agents.iter().find(|a| a.uuid.eq_ignore_ascii_case(uuid))
    }

    /// Display name, or `agent_<first 8 hex>` when unknown.
    pub fn agent_name(&self, uuid: Option<&str>) -> String {
        let Some(uuid) = uuid else { return "unknown".into() };
        match self.agent(uuid) {
            Some(a) => a.name.clone(),
            None => format!(
                "agent_{}",
                uuid.chars()
                    .filter(|c| c.is_ascii_hexdigit())
                    .take(8)
                    .collect::<String>()
            ),
        }
    }

    pub fn ability_name(&self, agent_uuid: &str, slot: &str) -> Option<&str> {
        self.agent(agent_uuid)?
            .abilities
            .iter()
            .find(|a| a.slot.eq_ignore_ascii_case(slot))
            .map(|a| a.name.as_str())
    }

    pub fn map_by_url(&self, map_url: &str) -> Option<&MapInfo> {
        self.maps.iter().find(|m| m.map_url.eq_ignore_ascii_case(map_url))
    }

    pub fn weapon(&self, uuid: &str) -> Option<&Weapon> {
        self.weapons.iter().find(|w| w.uuid.eq_ignore_ascii_case(uuid))
    }
}

impl MapInfo {
    pub fn uv(&self, pos_cm: [f64; 3]) -> Option<[f64; 2]> {
        self.projection.map(|p| p.uv(pos_cm[0], pos_cm[1]))
    }

    /// Nearest callout point on the plane. This is the nearest labelled
    /// point, not containment in a region; the API has no region outlines.
    pub fn nearest_callout(&self, pos_cm: [f64; 3]) -> Option<(&Callout, f64)> {
        self.callouts
            .iter()
            .map(|c| (c, (c.pos_cm[0] - pos_cm[0]).hypot(c.pos_cm[1] - pos_cm[1])))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// Quarter turns clockwise that put the attackers' spawn at the bottom of
    /// the minimap, so the map reads the way attackers walk it. valorant-api.com's
    /// images face different ways (Ascent's attackers start on the right, Lotus's
    /// at the bottom); 0 when the map has no spawn callouts.
    pub fn attacker_turns(&self) -> u8 {
        let spawn = |side: &str| {
            self.callouts
                .iter()
                .find(|c| c.super_region == side && c.region == "Spawn")
                .and_then(|c| self.uv(c.pos_cm))
        };
        let (Some(attack), Some(defense)) = (spawn("Attacker Side"), spawn("Defender Side")) else {
            return 0;
        };
        // From the defenders' spawn to the attackers': the turn that points it most downward.
        let mut dir = [attack[0] - defense[0], attack[1] - defense[1]];
        let mut best = (0, dir[1]);
        for k in 1..4 {
            dir = [-dir[1], dir[0]];
            if dir[1] > best.1 {
                best = (k, dir[1]);
            }
        }
        best.0
    }
}

/// A minimap position after `turns` quarter turns clockwise.
pub fn turn_uv(uv: [f64; 2], turns: u8) -> [f64; 2] {
    (0..turns % 4).fold(uv, |[u, v], _| [1.0 - v, u])
}

// ---------------------------------------------------------------- fetching

/// Downloads everything and returns a fresh catalog. Does not touch the cache.
pub fn fetch() -> Result<Catalog> {
    let version: Envelope<RawVersion> = get_json("version")?;
    let agents: Envelope<Vec<RawAgent>> = get_json("agents")?;
    let maps: Envelope<Vec<RawMap>> = get_json("maps")?;
    let weapons: Envelope<Vec<RawWeapon>> = get_json("weapons")?;
    let gear: Envelope<Vec<RawGear>> = get_json("gear")?;
    let seasons: Envelope<Vec<RawSeason>> = get_json("seasons")?;
    Ok(Catalog::from_raw(
        Some(crate::time::now_rfc3339()),
        version.data,
        agents.data,
        maps.data,
        weapons.data,
        gear.data,
        seasons.data,
    ))
}

fn get_json<T: for<'de> Deserialize<'de>>(endpoint: &str) -> Result<T> {
    let url = format!("{API_BASE}/{endpoint}");
    let mut resp = ureq::get(&url)
        .header("User-Agent", concat!("vantage/", env!("CARGO_PKG_VERSION")))
        .call()
        .with_context(|| format!("GET {url}"))?;
    let text = resp
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_string()
        .with_context(|| format!("reading {url}"))?;
    serde_json::from_str(&text).with_context(|| format!("unexpected response shape from {url}"))
}

// ---------------------------------------------------------------- raw API shapes

/// The API sends explicit `null` for some lists and numbers; `#[serde(default)]`
/// alone only covers missing keys.
fn null_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Deserialize)]
struct RawVersion {
    version: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAgent {
    uuid: String,
    display_name: String,
    display_icon: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    is_playable_character: bool,
    role: Option<RawNamed>,
    #[serde(default, deserialize_with = "null_default")]
    abilities: Vec<RawAbility>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawNamed {
    display_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAbility {
    slot: String,
    display_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMap {
    uuid: String,
    display_name: String,
    map_url: Option<String>,
    display_icon: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    x_multiplier: f64,
    #[serde(default, deserialize_with = "null_default")]
    y_multiplier: f64,
    #[serde(default, deserialize_with = "null_default")]
    x_scalar_to_add: f64,
    #[serde(default, deserialize_with = "null_default")]
    y_scalar_to_add: f64,
    callouts: Option<Vec<RawCallout>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCallout {
    region_name: String,
    #[serde(default, deserialize_with = "null_default")]
    super_region_name: String,
    location: RawXyz,
}

#[derive(Deserialize)]
struct RawXyz {
    x: f64,
    y: f64,
    #[serde(default, deserialize_with = "null_default")]
    z: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawWeapon {
    uuid: String,
    display_name: String,
    #[serde(default, deserialize_with = "null_default")]
    category: String,
    asset_path: Option<String>,
    weapon_stats: Option<RawWeaponStats>,
    shop_data: Option<RawShop>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawGear {
    uuid: String,
    display_name: String,
    asset_path: Option<String>,
    shop_data: Option<RawShop>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawWeaponStats {
    fire_rate: Option<f64>,
    magazine_size: Option<u32>,
    #[serde(default, deserialize_with = "null_default")]
    damage_ranges: Vec<RawDamageRange>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDamageRange {
    range_start_meters: f64,
    range_end_meters: f64,
    head_damage: f64,
    body_damage: f64,
    leg_damage: f64,
}

#[derive(Deserialize)]
struct RawShop {
    cost: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSeason {
    uuid: String,
    display_name: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    start_time: Option<String>,
    end_time: Option<String>,
    parent_uuid: Option<String>,
}

impl Catalog {
    fn from_raw(
        fetched_at: Option<String>,
        version: RawVersion,
        agents: Vec<RawAgent>,
        maps: Vec<RawMap>,
        weapons: Vec<RawWeapon>,
        gear: Vec<RawGear>,
        seasons: Vec<RawSeason>,
    ) -> Self {
        Catalog {
            schema_version: CATALOG_SCHEMA_VERSION,
            fetched_at,
            game_version: version.version,
            agents: agents
                .into_iter()
                .map(|a| Agent {
                    uuid: a.uuid.to_ascii_lowercase(),
                    name: a.display_name,
                    role: a.role.map(|r| r.display_name),
                    icon: a.display_icon,
                    playable: a.is_playable_character,
                    abilities: a
                        .abilities
                        .into_iter()
                        .map(|b| Ability {
                            slot: b.slot,
                            name: b.display_name,
                        })
                        .collect(),
                })
                .collect(),
            maps: maps
                .into_iter()
                .filter_map(|m| {
                    let projection = (m.x_multiplier != 0.0 && m.y_multiplier != 0.0).then_some(Projection {
                        x_multiplier: m.x_multiplier,
                        y_multiplier: m.y_multiplier,
                        x_scalar_to_add: m.x_scalar_to_add,
                        y_scalar_to_add: m.y_scalar_to_add,
                    });
                    Some(MapInfo {
                        uuid: m.uuid.to_ascii_lowercase(),
                        name: m.display_name,
                        map_url: m.map_url?,
                        minimap_image: m.display_icon,
                        projection,
                        callouts: m
                            .callouts
                            .unwrap_or_default()
                            .into_iter()
                            .map(|c| Callout {
                                region: c.region_name,
                                super_region: c.super_region_name,
                                pos_cm: [c.location.x, c.location.y, c.location.z],
                            })
                            .collect(),
                    })
                })
                .collect(),
            weapons: weapons
                .into_iter()
                .map(|w| {
                    let stats = w.weapon_stats;
                    Weapon {
                        uuid: w.uuid.to_ascii_lowercase(),
                        name: w.display_name,
                        category: w.category,
                        asset_folder: w.asset_path.as_deref().map(asset_folder),
                        cost: w.shop_data.and_then(|s| s.cost),
                        magazine_size: stats.as_ref().and_then(|s| s.magazine_size),
                        fire_rate: stats.as_ref().and_then(|s| s.fire_rate),
                        damage_ranges: stats
                            .map(|s| s.damage_ranges)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|d| DamageRange {
                                start_m: d.range_start_meters,
                                end_m: d.range_end_meters,
                                head: d.head_damage,
                                body: d.body_damage,
                                leg: d.leg_damage,
                            })
                            .collect(),
                    }
                })
                .collect(),
            gear: gear
                .into_iter()
                .map(|g| Gear {
                    uuid: g.uuid.to_ascii_lowercase(),
                    name: g.display_name,
                    cost: g.shop_data.and_then(|s| s.cost),
                    class_prefix: g.asset_path.as_deref().map(gear_prefix),
                })
                .collect(),
            seasons: seasons
                .into_iter()
                .map(|s| Season {
                    uuid: s.uuid.to_ascii_lowercase(),
                    name: s.display_name,
                    kind: s.kind,
                    start: s.start_time,
                    end: s.end_time,
                    parent_uuid: s.parent_uuid.map(|p| p.to_ascii_lowercase()),
                })
                .collect(),
        }
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` -> unix seconds (days-from-civil, Howard Hinnant).
fn parse_rfc3339(s: &str) -> Option<i64> {
    let (date, time) = s.trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let mut t = time.split(':').map(|x| x.parse::<i64>());
    let (hh, mm, ss) = (t.next()?.ok()?, t.next()?.ok()?, t.next()?.ok()?);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// `ShooterGame/Content/Equippables/Guns/Rifles/AK/AKPrimaryAsset` -> `Equippables/Guns/Rifles/AK`.
fn asset_folder(asset_path: &str) -> String {
    let p = asset_path.strip_prefix("ShooterGame/Content/").unwrap_or(asset_path);
    p.rsplit_once('/').map(|(f, _)| f).unwrap_or("").to_owned()
}

/// `ShooterGame/Content/Gear/HeavyArmor_PrimaryAsset` -> `HeavyArmor`.
fn gear_prefix(asset_path: &str) -> String {
    let stem = asset_path.rsplit('/').next().unwrap_or(asset_path);
    stem.trim_end_matches("PrimaryAsset").trim_end_matches('_').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from the live API (2026-10-20); field names and types unchanged.
    const AGENTS: &str = r#"{"status":200,"data":[
        {"uuid":"320b2a48-4d9b-a075-30f1-1f93a9b638fa","displayName":"Sova","displayIcon":"https://x/sova.png",
         "isPlayableCharacter":true,"role":{"uuid":"1b47567f","displayName":"Initiator","description":"","displayIcon":"","assetPath":""},
         "abilities":[{"slot":"Ability1","displayName":"Shock Bolt","description":"","displayIcon":null},
                      {"slot":"Ultimate","displayName":"Hunter's Fury","description":"","displayIcon":null}],
         "voiceLine":null,"recruitmentData":null},
        {"uuid":"773f0c78-4486-752b-68ef-4585d7f4b848","displayName":"KAY/O","displayIcon":null,
         "isPlayableCharacter":false,"role":null,"abilities":null}]}"#;
    const MAPS: &str = r#"{"status":200,"data":[
        {"uuid":"2fe4ed3a-450a-948b-6d6b-e89a78e680a9","displayName":"Lotus","mapUrl":"/Game/Maps/Jam/Jam",
         "displayIcon":"https://x/lotus.png","coordinates":null,"xMultiplier":7.2e-05,"yMultiplier":-7.2e-05,
         "xScalarToAdd":0.454789,"yScalarToAdd":0.917752,
         "callouts":[{"regionName":"Site","superRegion":"ECalloutSuperRegion::A","superRegionName":"A",
                      "location":{"x":7735.5396,"y":5557.309,"z":232.63},"scale3D":null,"rotation":null},
                     {"regionName":"Top","superRegion":"ECalloutSuperRegion::A","superRegionName":"A",
                      "location":{"x":9260.3,"y":5045.5884,"z":80.35567},"scale3D":null,"rotation":null}]},
        {"uuid":"x","displayName":"The Range","mapUrl":"/Game/Maps/Poveglia/Range","displayIcon":null,
         "xMultiplier":0,"yMultiplier":0,"xScalarToAdd":0,"yScalarToAdd":0,"callouts":null}]}"#;
    const WEAPONS: &str = r#"{"status":200,"data":[
        {"uuid":"9c82e19d-4575-0200-1a81-3eacf00cf872","displayName":"Vandal","category":"EEquippableCategory::Rifle",
         "assetPath":"ShooterGame/Content/Equippables/Guns/Rifles/AK/AKPrimaryAsset",
         "weaponStats":{"fireRate":9.75,"magazineSize":25,"damageRanges":[{"rangeStartMeters":0,"rangeEndMeters":50,
         "headDamage":160,"bodyDamage":40,"legDamage":34}]},"shopData":{"cost":2900,"category":"Rifles"},"skins":[]},
        {"uuid":"2f59173c-4bed-b6c3-2191-dea9b58be9c7","displayName":"Melee","category":"EEquippableCategory::Melee",
         "weaponStats":null,"shopData":null,"skins":[]}]}"#;
    const GEAR: &str = r#"{"status":200,"data":[{"uuid":"822bcab2-40a2-324e-c137-e09195ad7692","displayName":"Heavy Armor",
        "assetPath":"ShooterGame/Content/Gear/HeavyArmor_PrimaryAsset","shopData":{"cost":1000}}]}"#;
    const SEASONS: &str = r#"{"status":200,"data":[{"uuid":"3737c391-497a-6e82-aeb5-cc9f701f72e2","displayName":"V26",
        "title":null,"type":null,"startTime":"2026-06-24T00:00:00Z","endTime":"2027-01-06T00:00:00Z","parentUuid":null,"assetPath":""}]}"#;
    const VERSION: &str =
        r#"{"status":200,"data":{"manifestId":"D80C","branch":"release-13.08","version":"13.08.00.5698904"}}"#;

    fn parsed() -> Catalog {
        let p = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        Catalog::from_raw(
            Some("1970-01-01T00:00:00Z".into()),
            serde_json::from_value::<Envelope<RawVersion>>(p(VERSION)).unwrap().data,
            serde_json::from_value::<Envelope<Vec<RawAgent>>>(p(AGENTS))
                .unwrap()
                .data,
            serde_json::from_value::<Envelope<Vec<RawMap>>>(p(MAPS)).unwrap().data,
            serde_json::from_value::<Envelope<Vec<RawWeapon>>>(p(WEAPONS))
                .unwrap()
                .data,
            serde_json::from_value::<Envelope<Vec<RawGear>>>(p(GEAR)).unwrap().data,
            serde_json::from_value::<Envelope<Vec<RawSeason>>>(p(SEASONS))
                .unwrap()
                .data,
        )
    }

    #[test]
    fn parses_live_shapes() {
        let c = parsed();
        assert_eq!(c.game_version.as_deref(), Some("13.08.00.5698904"));
        assert_eq!(c.agent_name(Some("320B2A48-4D9B-A075-30F1-1F93A9B638FA")), "Sova");
        assert_eq!(
            c.agent("320b2a48-4d9b-a075-30f1-1f93a9b638fa").unwrap().role.as_deref(),
            Some("Initiator")
        );
        assert_eq!(
            c.ability_name("320b2a48-4d9b-a075-30f1-1f93a9b638fa", "ultimate"),
            Some("Hunter's Fury")
        );
        assert_eq!(c.agent_name(Some("deadbeef-0000")), "agent_deadbeef");
        let lotus = c.map_by_url("/game/maps/jam/jam").unwrap();
        assert_eq!(lotus.name, "Lotus");
        assert_eq!(lotus.callouts[0].label(), "A Site");
        assert!(c.map_by_url("/Game/Maps/Poveglia/Range").unwrap().projection.is_none());
        let vandal = c.weapon("9c82e19d-4575-0200-1a81-3eacf00cf872").unwrap();
        assert_eq!(
            (vandal.cost, vandal.magazine_size, vandal.damage_ranges[0].head),
            (Some(2900), Some(25), 160.0)
        );
        assert!(
            c.weapon("2f59173c-4bed-b6c3-2191-dea9b58be9c7")
                .unwrap()
                .damage_ranges
                .is_empty()
        );
        assert_eq!(c.seasons[0].name, "V26");
        assert_eq!(vandal.asset_folder.as_deref(), Some("Equippables/Guns/Rifles/AK"));
        assert_eq!(c.gear[0].class_prefix.as_deref(), Some("HeavyArmor"));
        let item = crate::items::identify("/Game/Equippables/Guns/Rifles/AK/AssaultRifle_AK.AssaultRifle_AK_C", &c);
        assert_eq!((item.name.as_str(), item.cost), ("Vandal", Some(2900)));
        // Round trip through the cache format.
        let back: Catalog = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.maps.len(), 2);
    }

    #[test]
    fn projection_axes_cross() {
        let c = parsed();
        let lotus = c.map_by_url("/Game/Maps/Jam/Jam").unwrap();
        // Centre of Sova's repeated death spot on the oracle replay.
        let [u, v] = lotus.uv([7944.0, 5708.0, 402.0]).unwrap();
        assert!((u - (5708.0 * 7.2e-5 + 0.454789)).abs() < 1e-12);
        assert!((v - (7944.0 * -7.2e-5 + 0.917752)).abs() < 1e-12);
        assert!((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v));
        let (callout, d) = lotus.nearest_callout([7944.0, 5708.0, 402.0]).unwrap();
        assert_eq!(callout.label(), "A Site");
        assert!(d < 300.0);
    }

    #[test]
    fn maps_turn_so_attackers_start_at_the_bottom() {
        // The two spawns as valorant-api.com places them on Ascent's minimap
        // (attackers right, defenders left) and on Lotus's (attackers already at the bottom).
        let map = |attack: [f64; 2], defense: [f64; 2]| MapInfo {
            uuid: String::new(),
            name: String::new(),
            map_url: String::new(),
            minimap_image: None,
            // The identity, with the axes crossed: u = y, v = x.
            projection: Some(Projection {
                x_multiplier: 1.0,
                y_multiplier: 1.0,
                x_scalar_to_add: 0.0,
                y_scalar_to_add: 0.0,
            }),
            callouts: [("Attacker Side", attack), ("Defender Side", defense)]
                .into_iter()
                .map(|(side, [u, v])| Callout {
                    region: "Spawn".into(),
                    super_region: side.into(),
                    pos_cm: [v, u, 0.0],
                })
                .collect(),
        };
        let ascent = map([0.82, 0.57], [0.13, 0.43]);
        assert_eq!(ascent.attacker_turns(), 1);
        let [_, v] = turn_uv([0.82, 0.57], ascent.attacker_turns());
        assert!(v > 0.75, "attackers end up at the bottom");
        assert_eq!(map([0.51, 0.82], [0.58, 0.22]).attacker_turns(), 0);
        assert_eq!(map([0.47, 0.13], [0.5, 0.9]).attacker_turns(), 2);
        assert_eq!(map([0.14, 0.59], [0.97, 0.57]).attacker_turns(), 3);
        assert_eq!(turn_uv([0.2, 0.1], 4), [0.2, 0.1]);
        let mut no_spawns = ascent.clone();
        no_spawns.callouts.clear();
        assert_eq!(no_spawns.attacker_turns(), 0);
    }

    #[test]
    fn age() {
        assert_eq!(parse_rfc3339("2026-10-20T00:00:00Z"), Some(1_792_454_400));
        let c = Catalog {
            fetched_at: Some("2026-10-01T00:00:00Z".into()),
            ..Catalog::builtin()
        };
        assert_eq!(c.age_days(1_792_454_400), Some(19));
        assert_eq!(Catalog::builtin().age_days(0), None);
    }

    #[test]
    fn missing_cache_falls_back() {
        let (c, src) = Catalog::load_or_builtin(Path::new("definitely/not/here/catalog.json"));
        assert!(matches!(src, CatalogSource::BuiltinNoCache));
        assert!(c.is_builtin());
        assert_eq!(c.agent_name(Some("320b2a48-4d9b-a075-30f1-1f93a9b638fa")), "Sova");
        assert!(c.map_by_url("/Game/Maps/Jam/Jam").is_none());
    }
}
