//! One pass over `fields.parquet` (about 1.8 M rows) that keeps only the rows
//! valreview uses, already typed. Every field read here was checked to exist
//! and carry typed values in a real 13.08 export before it was used.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use arrow_array::{
    Array, ArrayRef, BooleanArray, DictionaryArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt32Array,
    cast::AsArray, types,
};
use arrow_schema::DataType;

use crate::export::{column, open_reader};

/// One `MulticastNotifyDamage_{Point,Base}` invocation (one hit or tick).
#[derive(Debug, Clone, Default)]
pub struct DamageHit {
    pub time_ms: u32,
    /// The damaged actor: a player pawn, or utility (walls, turrets, ...).
    pub victim: u32,
    /// `DamagerPlayerState`, a BombPlayerState actor.
    pub damager_state: Option<u32>,
    /// `EquippableUsed`: weapon or ability actor.
    pub equippable: Option<u32>,
    /// `DamageDealt`: the hit's full damage, as the game's CombatReport sums it.
    pub damage: f64,
    /// `DamageTaken`: what the hit actually removed (capped by remaining health and armour).
    pub damage_applied: f64,
    /// `RegionalDamage`: 0 body, 1 head, 2 legs, 5 not a body hit.
    pub region: Option<i64>,
    pub wallbang: bool,
    pub killed: bool,
}

/// An `AbilityCastsThisRound` element, deduplicated by cast identity.
#[derive(Debug, Clone)]
pub struct AbilityCast {
    /// Account UUID.
    pub player: String,
    /// 3 Grenade (C), 4 Ability1 (Q), 5 Ability2 (E), 9 Ultimate (X); see `utility.rs`.
    pub slot: i64,
    pub round: i64,
    /// Seconds after the barrier drop (`buy_end_ms`).
    pub cast_time_s: f64,
    pub location_cm: Option<[f64; 3]>,
    /// First time this cast was seen in the replay.
    pub seen_ms: u32,
}

#[derive(Debug, Clone, Default)]
pub struct RoundResultRaw {
    pub number: Option<i64>,
    pub result: Option<String>,
    pub winning_team: Option<String>,
    pub winning_role: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct Change<T> {
    pub time_ms: u32,
    pub actor: u32,
    pub value: T,
}

#[derive(Debug, Clone, Copy)]
pub struct AmmoRow {
    pub time_ms: u32,
    /// The weapon actor.
    pub weapon: u32,
    /// The AmmoComponent subobject: one for the magazine, one for reserve.
    pub component: u32,
    pub amount: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Fields {
    /// `BombPlayerState.Ping` by player-state actor.
    pub pings: HashMap<u32, Vec<u32>>,
    pub damage: Vec<DamageHit>,
    pub casts: Vec<AbilityCast>,
    /// `MulticastSetPhase.NewPhase`: 2 reset, 3 round start, 4 buy end, 5 post-round, 6 side switch.
    pub phases: Vec<Change<i64>>,
    pub round_results: Vec<RoundResultRaw>,
    /// `TimedBomb.PlantedAtSite` (site index).
    pub planted_at_site: Vec<Change<i64>>,
    /// `BombDefusedRPC.DefusingCharacter` (pawn).
    pub defused_by: Vec<Change<u32>>,
    /// `Owner` writes on any actor (weapons, armour, the spike, ...). 0 = none.
    pub owners: Vec<Change<u32>>,
    /// `AresInventory.NewCurrentEquippable`: actor is the pawn, value the item put in hand (0 = none).
    pub in_hand: Vec<Change<u32>>,
    pub ammo: Vec<AmmoRow>,
    /// `MoneyManagementComponent.Money` by player-state actor.
    pub money: Vec<Change<i64>>,
    /// `MoneyManagementComponent.StartOfRoundMoney` by player-state actor.
    pub start_money: Vec<Change<i64>>,
    /// `BombPlayerState.NumUltimatePoints`.
    pub ult_points: Vec<Change<i64>>,
    /// `BombPlayerState.bUltimateActive`.
    pub ult_active: Vec<Change<bool>>,
    /// Last `BasicCombatStatsComponent.AggregateKills` / `AggregateDeaths` per actor (cross-checks).
    pub aggregate_kills: HashMap<u32, i64>,
    pub aggregate_deaths: HashMap<u32, i64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DamageFn {
    Point,
    Base,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DamageParam {
    DamagerPlayerState,
    EquippableUsed,
    DamageDealt,
    DamageTaken,
    RegionalDamage,
    WallPen,
    Killed,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CastMember {
    Player,
    Slot,
    Round,
    CastTime,
    CastLocation,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResultMember {
    Number,
    Result,
    Team,
    Role,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ping,
    Phase,
    RoundResult(usize, ResultMember),
    PlantedAtSite,
    DefusedBy,
    Owner,
    NewCurrentEquippable,
    Ammo,
    Money,
    StartMoney,
    UltPoints,
    UltActive,
    Damage(DamageFn, DamageParam),
    Cast(usize, CastMember),
    AggregateKills,
    AggregateDeaths,
}

/// `CastTime_4_5AE2…` -> `CastTime`: drops Blueprint hash suffixes from every path segment.
pub fn normalize_name(name: &str) -> String {
    name.split('.').map(strip_hash_suffix).collect::<Vec<_>>().join(".")
}

fn strip_hash_suffix(seg: &str) -> &str {
    // <name>_<digits>_<32 hex>
    let Some((head, hash)) = seg.rsplit_once('_') else {
        return seg;
    };
    if hash.len() != 32 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return seg;
    }
    match head.rsplit_once('_') {
        Some((name, num)) if !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => seg,
    }
}

/// `Foo[12].Bar` -> (12, "Bar") for a given array prefix `Foo`.
fn array_member<'a>(name: &'a str, array: &str) -> Option<(usize, &'a str)> {
    let rest = name.strip_prefix(array)?.strip_prefix('[')?;
    let (idx, member) = rest.split_once("].")?;
    Some((idx.parse().ok()?, member))
}

fn classify(name: &str) -> Option<Kind> {
    Some(match name {
        "Ping" => Kind::Ping,
        "MulticastSetPhase.NewPhase" => Kind::Phase,
        "PlantedAtSite" => Kind::PlantedAtSite,
        "BombDefusedRPC.DefusingCharacter" => Kind::DefusedBy,
        "Owner" => Kind::Owner,
        "NewCurrentEquippable" => Kind::NewCurrentEquippable,
        "AuthResourceAmount" => Kind::Ammo,
        "Money" => Kind::Money,
        "StartOfRoundMoney" => Kind::StartMoney,
        "NumUltimatePoints" => Kind::UltPoints,
        "bUltimateActive" => Kind::UltActive,
        "AggregateKills" => Kind::AggregateKills,
        "AggregateDeaths" => Kind::AggregateDeaths,
        _ => {
            if let Some((idx, member)) = array_member(name, "RoundResults") {
                let m = match member {
                    "RoundNumber" => ResultMember::Number,
                    "RoundResult" => ResultMember::Result,
                    "WinningTeam" => ResultMember::Team,
                    "WinningTeamRole" => ResultMember::Role,
                    _ => return None,
                };
                return Some(Kind::RoundResult(idx, m));
            }
            if let Some((idx, member)) = array_member(name, "AbilityCastsThisRound") {
                let m = match member {
                    "Player" => CastMember::Player,
                    "Slot" => CastMember::Slot,
                    "Round" => CastMember::Round,
                    "CastTime" => CastMember::CastTime,
                    "CastLocation" => CastMember::CastLocation,
                    _ => return None,
                };
                return Some(Kind::Cast(idx, m));
            }
            let (func, param) = name.split_once('.')?;
            let f = match func {
                "MulticastNotifyDamage_Point" => DamageFn::Point,
                "MulticastNotifyDamage_Base" => DamageFn::Base,
                _ => return None,
            };
            if param.contains('[') || param == "LifeChangeEvents" || param == "AssistsList" {
                return None;
            }
            let p = match param {
                "DamagerPlayerState" => DamageParam::DamagerPlayerState,
                "EquippableUsed" => DamageParam::EquippableUsed,
                "DamageDealt" => DamageParam::DamageDealt,
                "DamageTaken" => DamageParam::DamageTaken,
                "RegionalDamage" => DamageParam::RegionalDamage,
                "bIsWallPenetration" => DamageParam::WallPen,
                "bDamageKilledTarget" => DamageParam::Killed,
                _ => DamageParam::Other,
            };
            Kind::Damage(f, p)
        }
    })
}

/// Group paths that must accompany an ambiguous field name.
fn group_ok(kind: Kind, group: &str) -> bool {
    match kind {
        Kind::Ping | Kind::UltPoints | Kind::UltActive => group.contains("PlayerState"),
        Kind::Money | Kind::StartMoney => group.contains("MoneyManagementComponent"),
        Kind::NewCurrentEquippable => group.contains("AresInventory"),
        Kind::Ammo => group.contains("AmmoComponent"),
        Kind::Phase | Kind::RoundResult(..) => group.contains("GameState"),
        Kind::PlantedAtSite => group.contains("TimedBomb"),
        Kind::Cast(..) => group.contains("AbilityStatisticsReplicator"),
        Kind::AggregateKills | Kind::AggregateDeaths => group.contains("CombatStats"),
        Kind::DefusedBy | Kind::Owner | Kind::Damage(..) => true,
    }
}

/// String column that may be dictionary-encoded.
enum Strs<'a> {
    Dict(&'a DictionaryArray<types::Int32Type>, Vec<Option<String>>),
    Plain(ArrayRef),
}

impl<'a> Strs<'a> {
    fn new(col: &'a ArrayRef) -> Result<Self> {
        if let Some(dict) = col.as_any().downcast_ref::<DictionaryArray<types::Int32Type>>() {
            let values = arrow_cast::cast(dict.values(), &DataType::Utf8)?;
            let values = values.as_string::<i32>();
            let decoded = (0..values.len())
                .map(|i| values.is_valid(i).then(|| values.value(i).to_owned()))
                .collect();
            return Ok(Strs::Dict(dict, decoded));
        }
        Ok(Strs::Plain(arrow_cast::cast(col, &DataType::Utf8)?))
    }

    /// Dictionary key for row `i`, or the row itself for plain columns; used to memoise per value.
    fn key(&self, i: usize) -> Option<usize> {
        match self {
            Strs::Dict(d, _) => d.keys().is_valid(i).then(|| d.keys().value(i) as usize),
            Strs::Plain(a) => a.is_valid(i).then_some(i),
        }
    }

    fn get(&self, i: usize) -> Option<&str> {
        match self {
            Strs::Dict(d, values) => {
                let k = d.keys();
                if !k.is_valid(i) {
                    return None;
                }
                values.get(k.value(i) as usize)?.as_deref()
            }
            Strs::Plain(a) => {
                let s: &StringArray = a.as_string::<i32>();
                s.is_valid(i).then(|| s.value(i))
            }
        }
    }

    fn is_dict(&self) -> bool {
        matches!(self, Strs::Dict(..))
    }
}

/// Per-batch typed views of the value columns.
struct Values<'a> {
    i64: &'a Int64Array,
    f64: &'a Float64Array,
    bool: &'a BooleanArray,
    str: Strs<'a>,
}

impl Values<'_> {
    fn i(&self, r: usize) -> Option<i64> {
        self.i64.is_valid(r).then(|| self.i64.value(r))
    }
    fn f(&self, r: usize) -> Option<f64> {
        self.f64.is_valid(r).then(|| self.f64.value(r))
    }
    fn b(&self, r: usize) -> Option<bool> {
        self.bool.is_valid(r).then(|| self.bool.value(r))
    }
    fn s(&self, r: usize) -> Option<&str> {
        self.str.get(r)
    }
    fn guid(&self, r: usize) -> Option<u32> {
        self.i(r).and_then(|v| u32::try_from(v).ok())
    }
}

/// State for grouping damage parameter rows into invocations.
#[derive(Default)]
struct DamageRun {
    key: Option<(u32, u32, Option<u32>, bool)>,
    seen: Vec<DamageParam>,
    hit: DamageHit,
    other_params: usize,
}

/// State for the delta-replicated `AbilityCastsThisRound` array.
#[derive(Default, Clone)]
struct CastSlotState {
    player: Option<String>,
    slot: Option<i64>,
    round: Option<i64>,
    cast_time: Option<f64>,
    location: Option<[f64; 3]>,
}

pub fn load_fields(path: &Path) -> Result<Fields> {
    const COLS: [&str; 10] = [
        "time_ms",
        "packet_id",
        "actor_net_guid",
        "object_net_guid",
        "group_path",
        "field_name",
        "value_i64",
        "value_f64",
        "value_bool",
        "value_str",
    ];
    let mut out = Fields::default();
    let mut results: HashMap<usize, RoundResultRaw> = HashMap::new();
    let mut run = DamageRun::default();
    let mut cast_state: HashMap<(u32, usize), CastSlotState> = HashMap::new();
    let mut cast_touched: Vec<(u32, usize)> = Vec::new();
    // (packet, actor, time) of the snapshot being collected.
    let mut cast_packet: Option<(u32, u32, u32)> = None;
    let mut cast_seen: std::collections::HashSet<(String, i64, i64, u64)> = Default::default();

    for batch in open_reader(path, &COLS)? {
        let batch: RecordBatch = batch?;
        let t = column(&batch, "time_ms", &DataType::UInt32)?;
        let t: &UInt32Array = t.as_primitive::<types::UInt32Type>();
        let pk = column(&batch, "packet_id", &DataType::UInt32)?;
        let pk: &UInt32Array = pk.as_primitive::<types::UInt32Type>();
        let actor = column(&batch, "actor_net_guid", &DataType::UInt32)?;
        let actor: &UInt32Array = actor.as_primitive::<types::UInt32Type>();
        let object = column(&batch, "object_net_guid", &DataType::UInt32)?;
        let object: &UInt32Array = object.as_primitive::<types::UInt32Type>();
        let group_col = batch.column_by_name("group_path").expect("projected").clone();
        let name_col = batch.column_by_name("field_name").expect("projected").clone();
        let groups = Strs::new(&group_col)?;
        let names = Strs::new(&name_col)?;
        let vi = column(&batch, "value_i64", &DataType::Int64)?;
        let vf = column(&batch, "value_f64", &DataType::Float64)?;
        let vb = column(&batch, "value_bool", &DataType::Boolean)?;
        let vs = batch.column_by_name("value_str").expect("projected").clone();
        let v = Values {
            i64: vi.as_primitive::<types::Int64Type>(),
            f64: vf.as_primitive::<types::Float64Type>(),
            bool: vb.as_boolean(),
            str: Strs::new(&vs)?,
        };

        // Classify each distinct field name once per batch.
        let mut memo: HashMap<usize, Option<Kind>> = HashMap::new();
        for r in 0..batch.num_rows() {
            let Some(key) = names.key(r) else { continue };
            let kind = if names.is_dict() {
                *memo
                    .entry(key)
                    .or_insert_with(|| names.get(r).and_then(|n| classify(&normalize_name(n))))
            } else {
                names.get(r).and_then(|n| classify(&normalize_name(n)))
            };
            let Some(kind) = kind else { continue };
            if !group_ok(kind, groups.get(r).unwrap_or("")) {
                continue;
            }
            let time_ms = t.value(r);
            let a = actor.value(r);
            match kind {
                Kind::Ping => {
                    if let Some(p) = v.i(r).and_then(|p| u32::try_from(p).ok()) {
                        out.pings.entry(a).or_default().push(p);
                    }
                }
                Kind::Phase => push_i(&mut out.phases, time_ms, a, v.i(r)),
                Kind::PlantedAtSite => push_i(&mut out.planted_at_site, time_ms, a, v.i(r)),
                Kind::DefusedBy => {
                    if let Some(g) = v.guid(r) {
                        out.defused_by.push(Change {
                            time_ms,
                            actor: a,
                            value: g,
                        });
                    }
                }
                Kind::Owner => {
                    if let Some(g) = v.guid(r) {
                        out.owners.push(Change {
                            time_ms,
                            actor: a,
                            value: g,
                        });
                    }
                }
                Kind::NewCurrentEquippable => {
                    if let Some(g) = v.guid(r) {
                        out.in_hand.push(Change {
                            time_ms,
                            actor: a,
                            value: g,
                        });
                    }
                }
                Kind::Ammo => {
                    if let (Some(amount), true) = (v.i(r), object.is_valid(r)) {
                        out.ammo.push(AmmoRow {
                            time_ms,
                            weapon: a,
                            component: object.value(r),
                            amount,
                        });
                    }
                }
                Kind::Money => push_i(&mut out.money, time_ms, a, v.i(r)),
                Kind::StartMoney => push_i(&mut out.start_money, time_ms, a, v.i(r)),
                Kind::UltPoints => push_i(&mut out.ult_points, time_ms, a, v.i(r)),
                Kind::UltActive => {
                    if let Some(b) = v.b(r) {
                        out.ult_active.push(Change {
                            time_ms,
                            actor: a,
                            value: b,
                        });
                    }
                }
                Kind::AggregateKills => {
                    if let Some(x) = v.i(r) {
                        out.aggregate_kills.insert(a, x);
                    }
                }
                Kind::AggregateDeaths => {
                    if let Some(x) = v.i(r) {
                        out.aggregate_deaths.insert(a, x);
                    }
                }
                Kind::RoundResult(idx, m) => {
                    let e = results.entry(idx).or_default();
                    match m {
                        ResultMember::Number => e.number = v.i(r),
                        ResultMember::Result => e.result = v.s(r).map(str::to_owned),
                        ResultMember::Team => e.winning_team = v.s(r).map(str::to_owned),
                        ResultMember::Role => e.winning_role = v.s(r).map(str::to_owned),
                    }
                }
                Kind::Damage(f, p) => {
                    let key = (
                        pk.value(r),
                        a,
                        object.is_valid(r).then(|| object.value(r)),
                        f == DamageFn::Point,
                    );
                    let repeat = p != DamageParam::Other && run.seen.contains(&p);
                    if run.key != Some(key) || repeat {
                        flush_damage(&mut run, &mut out.damage);
                        run.key = Some(key);
                        run.hit = DamageHit {
                            time_ms,
                            victim: a,
                            ..Default::default()
                        };
                    }
                    if p != DamageParam::Other {
                        run.seen.push(p);
                    } else {
                        run.other_params += 1;
                    }
                    match p {
                        DamageParam::DamagerPlayerState => run.hit.damager_state = v.guid(r).filter(|&g| g != 0),
                        DamageParam::EquippableUsed => run.hit.equippable = v.guid(r).filter(|&g| g != 0),
                        DamageParam::DamageDealt => run.hit.damage = v.f(r).unwrap_or(0.0),
                        DamageParam::DamageTaken => run.hit.damage_applied = v.f(r).unwrap_or(0.0),
                        DamageParam::RegionalDamage => run.hit.region = v.i(r),
                        DamageParam::WallPen => run.hit.wallbang = v.b(r).unwrap_or(false),
                        DamageParam::Killed => run.hit.killed = v.b(r).unwrap_or(false),
                        DamageParam::Other => {}
                    }
                }
                Kind::Cast(idx, m) => {
                    let packet = (pk.value(r), a);
                    if cast_packet.map(|(p, act, _)| (p, act)) != Some(packet) {
                        if let Some((_, _, seen)) = cast_packet {
                            flush_casts(&cast_state, &mut cast_touched, &mut cast_seen, &mut out.casts, seen);
                        }
                        cast_packet = Some((packet.0, packet.1, time_ms));
                    }
                    let s = cast_state.entry((a, idx)).or_default();
                    match m {
                        CastMember::Player => s.player = v.s(r).map(str::to_owned),
                        CastMember::Slot => s.slot = v.i(r),
                        CastMember::Round => s.round = v.i(r),
                        CastMember::CastTime => s.cast_time = v.f(r),
                        CastMember::CastLocation => s.location = v.s(r).and_then(parse_vector),
                    }
                    if !cast_touched.contains(&(a, idx)) {
                        cast_touched.push((a, idx));
                    }
                }
            }
        }
    }
    flush_damage(&mut run, &mut out.damage);
    if let Some((_, _, seen)) = cast_packet {
        flush_casts(&cast_state, &mut cast_touched, &mut cast_seen, &mut out.casts, seen);
    }

    let mut results: Vec<(usize, RoundResultRaw)> = results.into_iter().collect();
    results.sort_by_key(|(i, _)| *i);
    out.round_results = results.into_iter().map(|(_, r)| r).collect();
    for list in [
        &mut out.phases,
        &mut out.planted_at_site,
        &mut out.money,
        &mut out.start_money,
        &mut out.ult_points,
    ] {
        list.sort_by_key(|c| c.time_ms);
    }
    out.owners.sort_by_key(|c| c.time_ms);
    out.in_hand.sort_by_key(|c| c.time_ms);
    out.ult_active.sort_by_key(|c| c.time_ms);
    out.ammo.sort_by_key(|c| c.time_ms);
    out.damage.sort_by_key(|d| d.time_ms);
    out.casts
        .sort_by(|a, b| a.round.cmp(&b.round).then(a.cast_time_s.total_cmp(&b.cast_time_s)));
    Ok(out)
}

fn push_i(list: &mut Vec<Change<i64>>, time_ms: u32, actor: u32, value: Option<i64>) {
    if let Some(value) = value {
        list.push(Change { time_ms, actor, value });
    }
}

fn flush_damage(run: &mut DamageRun, out: &mut Vec<DamageHit>) {
    if run.key.is_some() && !run.seen.is_empty() {
        out.push(std::mem::take(&mut run.hit));
    }
    run.key = None;
    run.seen.clear();
    run.other_params = 0;
}

/// After a packet's updates, every touched element whose identity members are
/// all known is a cast; the same cast re-sent in later snapshots is dropped.
fn flush_casts(
    state: &HashMap<(u32, usize), CastSlotState>,
    touched: &mut Vec<(u32, usize)>,
    seen: &mut std::collections::HashSet<(String, i64, i64, u64)>,
    out: &mut Vec<AbilityCast>,
    time_ms: u32,
) {
    for key in touched.drain(..) {
        let s = &state[&key];
        let (Some(player), Some(slot), Some(round), Some(cast_time)) = (&s.player, s.slot, s.round, s.cast_time) else {
            continue;
        };
        // Identity: the same cast always carries the same CastTime; round it to absorb f32 noise.
        let id = (player.clone(), round, slot, (cast_time * 1000.0).round() as u64);
        if seen.insert(id) {
            out.push(AbilityCast {
                player: player.clone(),
                slot,
                round,
                cast_time_s: cast_time,
                location_cm: s.location,
                seen_ms: time_ms,
            });
        }
    }
}

/// `(x,y,z)` as vrfkit writes vectors.
pub fn parse_vector(s: &str) -> Option<[f64; 3]> {
    let mut it = s
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .map(|p| p.trim().parse::<f64>());
    let v = [it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?];
    it.next().is_none().then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(
            normalize_name("AbilityCastsThisRound[0].CastTime_4_5AE288704801A9B74D6D159DFC2BD147"),
            "AbilityCastsThisRound[0].CastTime"
        );
        assert_eq!(
            normalize_name("MulticastNotifyDamage_Point.DamageTaken"),
            "MulticastNotifyDamage_Point.DamageTaken"
        );
        assert_eq!(normalize_name("Some_Name_12"), "Some_Name_12");
        assert_eq!(
            array_member("RoundResults[12].WinningTeam", "RoundResults"),
            Some((12, "WinningTeam"))
        );
        assert_eq!(
            parse_vector("(8757.9755859375,2488.5754,600.1)"),
            Some([8757.9755859375, 2488.5754, 600.1])
        );
        assert_eq!(parse_vector("nope"), None);
    }
}
