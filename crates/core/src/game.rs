//! Match-level structure derived from the event timeline: rounds, kills and
//! teams. No movement here, beyond where players stood at spawn.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::vrfkit_export::{EventKind, Export, Player};

/// One spike-carrier pair outweighs this many kills: only attackers can carry
/// the spike, while a kill can be a team kill.
const SPIKE_PAIR_WEIGHT: u32 = 5;
/// Groups of players up to this size are coloured exactly (every split tried).
const EXACT_UP_TO: usize = 16;
/// Above this share of kills joining one side, the kills don't come from two teams.
const NOT_TWO_TEAMS_SHARE: f64 = 0.1;
/// A spawn sample older than this counts as missing.
const SPAWN_SAMPLE_AGE_MS: u32 = 1500;

#[derive(Debug, Clone, Copy)]
pub struct Round {
    pub number: u32,
    /// `roundStarted`, i.e. the start of the buy phase.
    pub start_ms: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Kill {
    /// Position in the match's kill list (time order, file order on ties).
    pub index: usize,
    pub time_ms: u32,
    /// Player pawns (a reconnect's older pawn is mapped to the current one).
    pub killer: u32,
    pub victim: u32,
    /// Index into `Game::rounds`; `None` if the kill came before any `roundStarted`.
    pub round_idx: Option<usize>,
}

impl Kill {
    pub fn is_self_kill(&self) -> bool {
        self.killer == self.victim
    }
}

/// Teams inferred by 2-colouring the kill graph, with spike carriers and
/// spawn positions. Side numbers are arbitrary labels (0 and 1); they are not
/// attack/defense.
#[derive(Debug, Clone, Default)]
pub struct Teams {
    side: HashMap<u32, u8>,
    /// Kills between two pawns of known side, and those of them on one side.
    kills: usize,
    team_kills: usize,
}

impl Teams {
    pub fn side(&self, pawn: u32) -> Option<u8> {
        self.side.get(&pawn).copied()
    }

    pub fn same_team(&self, a: u32, b: u32) -> bool {
        matches!((self.side(a), self.side(b)), (Some(x), Some(y)) if x == y)
    }
}

#[derive(Debug, Clone)]
pub struct Game {
    pub export: Export,
    pub rounds: Vec<Round>,
    pub kills: Vec<Kill>,
    pub teams: Teams,
    pub warnings: Vec<String>,
}

impl Game {
    pub fn new(export: Export) -> Self {
        let mut warnings = Vec::new();
        let canonical: HashMap<u32, u32> = export
            .manifest
            .players
            .iter()
            .flat_map(|p| p.pawns.iter().map(move |&g| (g, p.pawn)))
            .collect();
        let canon = |g: u32| canonical.get(&g).copied().unwrap_or(g);

        let mut rounds = Vec::new();
        let mut kills = Vec::new();
        for ev in &export.events {
            match ev.kind {
                EventKind::RoundStarted { round } => rounds.push(Round {
                    number: round,
                    start_ms: ev.time_ms,
                }),
                EventKind::Death { killer, victim } => kills.push(Kill {
                    index: kills.len(),
                    time_ms: ev.time_ms,
                    killer: canon(killer),
                    victim: canon(victim),
                    round_idx: rounds.len().checked_sub(1),
                }),
                _ => {}
            }
        }
        if rounds.is_empty() {
            warnings.push("no roundStarted events; round numbers and round times are unavailable".into());
        }
        for k in &kills {
            for g in [k.killer, k.victim] {
                if !canonical.contains_key(&g) {
                    warnings.push(format!(
                        "kill at {} ms names pawn {g}, which is not a player pawn",
                        k.time_ms
                    ));
                }
            }
        }

        let same_team = spike_carrier_pairs(&export, &rounds, &canon);
        let spawns = spawn_positions(&export, &rounds);
        let teams = infer_teams(&export.manifest.players, &kills, &same_team, &spawns, &mut warnings);
        let mut game = Game {
            export,
            rounds,
            kills,
            teams,
            warnings,
        };
        if let Some(why) = game.unsupported_mode() {
            game.warnings.push(why);
        }
        game
    }

    /// Why this replay isn't a standard or competitive match, if it isn't.
    /// Everything Vantage derives assumes a spike, rounds and two teams; in
    /// deathmatch the kill graph means nothing and every match reads "drew".
    pub fn unsupported_mode(&self) -> Option<String> {
        let class = &self.export.actors.class;
        let reason = if !class.is_empty() && !class.values().any(|c| c.contains("BombEquippable")) {
            "has no spike"
        } else if self.rounds.len() < 2 {
            "has fewer than two rounds"
        } else if self.teams.kills >= 10 && self.teams.team_kills as f64 > self.teams.kills as f64 * NOT_TWO_TEAMS_SHARE
        {
            "has kills that don't split the players into two teams"
        } else {
            return None;
        };
        Some(format!(
            "Vantage only reviews standard and competitive matches, and this replay {reason} (deathmatch or another mode?)"
        ))
    }

    pub fn round(&self, kill: &Kill) -> Option<&Round> {
        kill.round_idx.map(|i| &self.rounds[i])
    }

    pub fn kills_in_round(&self, round_idx: Option<usize>) -> impl Iterator<Item = &Kill> {
        self.kills.iter().filter(move |k| k.round_idx == round_idx)
    }

    pub fn players(&self) -> &[Player] {
        &self.export.manifest.players
    }

    pub fn player(&self, pawn: u32) -> Option<&Player> {
        self.export.player_by_pawn(pawn)
    }

    pub fn agent_id(&self, pawn: u32) -> Option<&str> {
        self.player(pawn).and_then(|p| p.agent_id.as_deref())
    }

    pub fn team_of(&self, pawn: u32) -> Vec<u32> {
        let Some(side) = self.teams.side(pawn) else {
            return Vec::new();
        };
        self.players()
            .iter()
            .map(|p| p.pawn)
            .filter(|&p| self.teams.side(p) == Some(side))
            .collect()
    }

    /// Every player's team is known, so alive counts by team are complete.
    pub fn teams_complete(&self) -> bool {
        self.players().iter().all(|p| self.teams.side(p.pawn).is_some())
    }

    /// A kill of an enemy: not a self-kill, and not a known teammate.
    pub fn is_cross_team(&self, kill: &Kill) -> bool {
        !kill.is_self_kill() && !self.teams.same_team(kill.killer, kill.victim)
    }

    /// Kills and deaths per pawn: self-kills and team kills count as a death but not a kill.
    pub fn kd(&self, pawn: u32) -> (usize, usize) {
        let k = self
            .kills
            .iter()
            .filter(|k| k.killer == pawn && self.is_cross_team(k))
            .count();
        let d = self.kills.iter().filter(|k| k.victim == pawn).count();
        (k, d)
    }
}

/// Pairs of players who carried the spike in the same round: only attackers
/// can, so each pair is on one team. This places players the kill graph
/// cannot (no kills or deaths against a connected player).
fn spike_carrier_pairs(export: &Export, rounds: &[Round], canon: &dyn Fn(u32) -> u32) -> Vec<(u32, u32)> {
    let players: Vec<u32> = export.manifest.players.iter().map(|p| p.pawn).collect();
    let bombs: Vec<u32> = export
        .actors
        .class
        .iter()
        .filter(|(_, c)| c.contains("BombEquippable"))
        .map(|(&g, _)| g)
        .collect();
    let mut by_round: BTreeMap<usize, Vec<u32>> = BTreeMap::new();
    for c in export.fields.owners.iter().filter(|c| bombs.contains(&c.actor)) {
        let holder = canon(c.value);
        let Some(r) = rounds.iter().rposition(|r| r.start_ms <= c.time_ms) else {
            continue;
        };
        if players.contains(&holder) {
            let v = by_round.entry(r).or_default();
            if !v.contains(&holder) {
                v.push(holder);
            }
        }
    }
    by_round
        .values()
        .flat_map(|v| v.windows(2).map(|w| (w[0], w[1])))
        .collect()
}

/// Where each player stood halfway through each buy phase. Teams spawn at
/// opposite ends of the map, so this places players the kills and spike
/// carries cannot.
fn spawn_positions(export: &Export, rounds: &[Round]) -> Vec<HashMap<u32, [f32; 3]>> {
    rounds
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let end = rounds
                .get(i + 1)
                .map(|n| n.start_ms)
                .unwrap_or(export.manifest.duration_ms as u32);
            let buy_end = export
                .fields
                .phases
                .iter()
                .find(|c| c.value == 4 && c.time_ms >= r.start_ms && c.time_ms < end)
                .map(|c| c.time_ms);
            let t = match buy_end {
                Some(b) => r.start_ms + (b - r.start_ms) / 2,
                None => (r.start_ms + 10_000).min(end.saturating_sub(1)),
            };
            export
                .manifest
                .players
                .iter()
                .filter_map(|p| {
                    let s = std::iter::once(p.pawn)
                        .chain(p.pawns.iter().copied())
                        .find_map(|g| export.tracks.get(&g)?.at(t, SPAWN_SAMPLE_AGE_MS))?;
                    Some((p.pawn, s.pos))
                })
                .collect()
        })
        .collect()
}

/// Kills (opposite teams) and spike rounds together (same team) per pair, keyed low pawn first.
type Pairs = BTreeMap<(u32, u32), (u32, u32)>;

/// Two-colouring of the kill graph: a kill joins opposite teams, a
/// spike-carrier pair the same team. Each connected group gets the split that
/// contradicts the least evidence, so a team kill doesn't flip anyone. Groups
/// the kills don't connect, and players with no kills at all, are placed by
/// where they spawned. Side labels are arbitrary (0/1), not attack/defense.
fn infer_teams(
    players: &[Player],
    kills: &[Kill],
    same_team: &[(u32, u32)],
    spawns: &[HashMap<u32, [f32; 3]>],
    warnings: &mut Vec<String>,
) -> Teams {
    let key = |a: u32, b: u32| (a.min(b), a.max(b));
    let mut pairs: Pairs = BTreeMap::new();
    for k in kills.iter().filter(|k| !k.is_self_kill()) {
        pairs.entry(key(k.killer, k.victim)).or_default().0 += 1;
    }
    for &(a, b) in same_team.iter().filter(|(a, b)| a != b) {
        pairs.entry(key(a, b)).or_default().1 += 1;
    }
    let mut adj: BTreeMap<u32, Vec<u32>> = players.iter().map(|p| (p.pawn, Vec::new())).collect();
    for &(a, b) in pairs.keys() {
        adj.entry(a).or_default().push(b);
        adj.entry(b).or_default().push(a);
    }

    // Connected groups, from manifest order so the labels are deterministic.
    let mut seen: HashSet<u32> = HashSet::new();
    let mut groups: Vec<Vec<(u32, u8)>> = Vec::new();
    let mut isolated: Vec<u32> = Vec::new();
    let is_player = |g: u32| players.iter().any(|p| p.pawn == g);
    for start in players.iter().map(|p| p.pawn).chain(adj.keys().copied()) {
        if !seen.insert(start) {
            continue;
        }
        if adj[&start].is_empty() {
            if players.len() > 1 {
                isolated.push(start);
            } else {
                groups.push(vec![(start, 0)]);
            }
            continue;
        }
        let mut members = vec![start];
        let mut i = 0;
        while i < members.len() {
            for &y in &adj[&members[i]] {
                if seen.insert(y) {
                    members.push(y);
                }
            }
            i += 1;
        }
        let colours = colour(&members, &pairs);
        groups.push(members.into_iter().zip(colours).collect());
    }

    // The group with the most players sets the labels; the others line up with it by spawn.
    let mut side: HashMap<u32, u8> = HashMap::new();
    let players_in = |g: &[(u32, u8)]| g.iter().filter(|(p, _)| is_player(*p)).count();
    let anchor = (0..groups.len()).rev().max_by_key(|&i| players_in(&groups[i]));
    let mut guessed = 0;
    if let Some(a) = anchor {
        side.extend(groups[a].iter().copied());
        for g in groups.iter().enumerate().filter(|(i, _)| *i != a).map(|(_, g)| g) {
            let vote = spawn_vote(g, &side, spawns);
            if vote == 0 && players_in(g) > 0 {
                guessed += 1;
            }
            let flip = u8::from(vote < 0);
            side.extend(g.iter().map(|&(p, c)| (p, c ^ flip)));
        }
    }
    let mut unknown = 0;
    for p in isolated {
        let vote = if side.is_empty() {
            0
        } else {
            spawn_vote(&[(p, 0)], &side, spawns)
        };
        match vote.cmp(&0) {
            std::cmp::Ordering::Greater => {
                side.insert(p, 0);
            }
            std::cmp::Ordering::Less => {
                side.insert(p, 1);
            }
            std::cmp::Ordering::Equal => unknown += 1,
        }
    }

    let (mut cross, mut team_kills) = (0, 0);
    for k in kills.iter().filter(|k| !k.is_self_kill()) {
        let (Some(a), Some(b)) = (side.get(&k.killer), side.get(&k.victim)) else {
            continue;
        };
        cross += 1;
        if a == b {
            team_kills += 1;
            warnings.push(format!(
                "kill at {} ms ({} -> {}) joins pawns already on the same side: a team kill, or teams are wrong",
                k.time_ms, k.killer, k.victim
            ));
        }
    }
    for &(a, b) in same_team {
        if side.get(&a) != side.get(&b) {
            warnings.push(format!(
                "spike carriers {a} and {b} of one round ended up on different sides; teams are wrong"
            ));
        }
    }
    if guessed > 0 {
        warnings.push(format!(
            "kills and spike carries split the players into {} groups; teams for some players are a guess",
            guessed + 1
        ));
    }
    if unknown > 0 {
        warnings.push(format!(
            "{unknown} player(s) had no kills, deaths, spike carries or spawn positions; their team is unknown"
        ));
    }
    Teams {
        side,
        kills: cross,
        team_kills,
    }
}

/// The 0/1 split of one connected group that contradicts the least evidence:
/// a kill within a side costs 1, a spike pair across sides `SPIKE_PAIR_WEIGHT`.
/// Exact for small groups; larger ones start from a spanning-tree colouring
/// and flip single players while that helps. Ties go to the more even split.
fn colour(members: &[u32], pairs: &Pairs) -> Vec<u8> {
    let index: HashMap<u32, usize> = members.iter().enumerate().map(|(i, &p)| (p, i)).collect();
    // (i, j, cost if on one side, cost if on opposite sides)
    let edges: Vec<(usize, usize, u32, u32)> = pairs
        .iter()
        .filter_map(|(&(a, b), &(kills, spike))| {
            Some((*index.get(&a)?, *index.get(&b)?, kills, spike * SPIKE_PAIR_WEIGHT))
        })
        .collect();
    let n = members.len();
    let score = |c: &[u8]| {
        let cost: u32 = edges
            .iter()
            .map(|&(i, j, same, opposite)| if c[i] == c[j] { same } else { opposite })
            .sum();
        let ones = c.iter().filter(|&&x| x == 1).count();
        (cost, ones.abs_diff(n - ones))
    };
    if n <= EXACT_UP_TO {
        let mut best: Option<(Vec<u8>, (u32, usize))> = None;
        // The first member stays on side 0.
        for mask in 0..1u32 << (n - 1) {
            let c: Vec<u8> = (0..n)
                .map(|i| if i == 0 { 0 } else { ((mask >> (i - 1)) & 1) as u8 })
                .collect();
            let s = score(&c);
            if best.as_ref().is_none_or(|(_, b)| s < *b) {
                best = Some((c, s));
            }
        }
        return best.map(|b| b.0).unwrap_or_default();
    }
    let mut c: Vec<Option<u8>> = vec![None; n];
    c[0] = Some(0);
    let mut queue = std::collections::VecDeque::from([0]);
    while let Some(x) = queue.pop_front() {
        for &(i, j, same, opposite) in &edges {
            let y = if i == x {
                j
            } else if j == x {
                i
            } else {
                continue;
            };
            if c[y].is_none() {
                c[y] = Some(c[x].unwrap_or(0) ^ u8::from(same > opposite));
                queue.push_back(y);
            }
        }
    }
    let mut c: Vec<u8> = c.into_iter().map(Option::unwrap_or_default).collect();
    let mut best = score(&c);
    for _ in 0..n {
        let mut improved = false;
        for i in 1..n {
            c[i] ^= 1;
            let s = score(&c);
            if s < best {
                best = s;
                improved = true;
            } else {
                c[i] ^= 1;
            }
        }
        if !improved {
            break;
        }
    }
    c
}

/// Rounds where `group`'s labels match the placed players' (players with the
/// same label spawned closer together than those with the other), minus
/// rounds where they are the other way round.
fn spawn_vote(group: &[(u32, u8)], side: &HashMap<u32, u8>, spawns: &[HashMap<u32, [f32; 3]>]) -> i32 {
    let mean = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
    let mut vote = 0;
    for pos in spawns {
        let (mut same, mut other) = (Vec::new(), Vec::new());
        for &(x, cx) in group {
            let Some(px) = pos.get(&x) else { continue };
            for (y, &sy) in side {
                let Some(py) = pos.get(y) else { continue };
                let d = (px[0] as f64 - py[0] as f64).hypot(px[1] as f64 - py[1] as f64);
                if cx == sy { same.push(d) } else { other.push(d) }
            }
        }
        if let (Some(s), Some(o)) = (mean(&same), mean(&other)) {
            vote += match s.total_cmp(&o) {
                std::cmp::Ordering::Less => 1,
                std::cmp::Ordering::Greater => -1,
                std::cmp::Ordering::Equal => 0,
            };
        }
    }
    vote
}

/// A made-up export for tests: players with pawns 1..=n, events in time order.
#[cfg(test)]
pub(crate) mod sample {
    use std::collections::HashMap;

    use crate::fields::Fields;
    use crate::vrfkit_export::{Actors, Event, EventKind, Export, Manifest, Player, Sample, Track};

    pub fn export(players: u32, events: Vec<(u32, EventKind)>) -> Export {
        let mut actors = Actors::default();
        actors
            .class
            .insert(9000, "/Game/Equippables/Bomb/BombEquippable.BombEquippable_C".into());
        Export {
            manifest: Manifest {
                match_id: "m1".into(),
                replay_build: "13.08".into(),
                duration_ms: 1_000_000,
                map_path: None,
                timestamp_ticks: None,
                players: (1..=players)
                    .map(|p| Player {
                        subject: format!("p{p}"),
                        actor_net_guid: 100 + p,
                        pawn: p,
                        pawns: vec![p],
                        agent_id: None,
                    })
                    .collect(),
            },
            events: events
                .into_iter()
                .map(|(time_ms, kind)| Event { time_ms, kind })
                .collect(),
            tracks: HashMap::new(),
            fields: Fields::default(),
            actors,
        }
    }

    pub fn kill(time_ms: u32, killer: u32, victim: u32) -> (u32, EventKind) {
        (time_ms, EventKind::Death { killer, victim })
    }

    pub fn round(time_ms: u32, round: u32) -> (u32, EventKind) {
        (time_ms, EventKind::RoundStarted { round })
    }

    /// Puts `pawn` at `x` (metres along one axis) for the whole match.
    pub fn stand(ex: &mut Export, pawn: u32, x: f32) {
        let samples = (0..1000)
            .map(|i| Sample {
                time_ms: i * 1000,
                pos: [x * 100.0, 0.0, 0.0],
                yaw: 0.0,
                pitch: 0.0,
                vel: [0.0; 3],
            })
            .collect();
        ex.tracks.insert(pawn, Track { samples });
    }
}

#[cfg(test)]
mod tests {
    use super::sample::{export, kill, round, stand};
    use super::*;

    /// Teams 1-5 and 6-10: each of 1-5 kills two enemies and is killed by one.
    fn five_v_five() -> Vec<(u32, EventKind)> {
        let mut ev = vec![round(0, 0)];
        let mut t = 1000;
        for i in 1..=5 {
            for (k, v) in [(i, 5 + i), (i, 5 + i % 5 + 1), (5 + i, i)] {
                ev.push(kill(t, k, v));
                t += 1000;
            }
        }
        ev.push(round(100_000, 1));
        ev
    }

    fn sides(g: &Game) -> Vec<Option<u8>> {
        (1..=10).map(|p| g.teams.side(p)).collect()
    }

    #[test]
    fn a_team_kill_does_not_flip_a_team() {
        let mut ev = five_v_five();
        // 1 kills teammate 2 before anything else.
        ev.insert(1, kill(500, 1, 2));
        let g = Game::new(export(10, ev));
        let s = sides(&g);
        assert!(s[..5].iter().all(|x| *x == s[0] && x.is_some()), "{s:?}");
        assert!(s[5..].iter().all(|x| *x == s[5] && x.is_some() && *x != s[0]), "{s:?}");
        assert!(!g.is_cross_team(&g.kills[0]));
        assert_eq!(g.kd(1), (2, 1), "the team kill isn't a kill");
        assert_eq!(g.unsupported_mode(), None);
    }

    #[test]
    fn players_with_no_kills_are_placed_by_spawn() {
        let mut ex = export(11, five_v_five());
        for p in 1..=5 {
            stand(&mut ex, p, 0.0);
        }
        for p in 6..=10 {
            stand(&mut ex, p, 100.0);
        }
        // 11 never kills or dies but spawns with 6-10.
        stand(&mut ex, 11, 98.0);
        let g = Game::new(ex);
        assert_eq!(g.teams.side(11), g.teams.side(6));
        assert!(g.teams_complete());
        assert!(g.warnings.iter().all(|w| !w.contains("unknown")), "{:?}", g.warnings);
        // Without positions its team stays unknown.
        let g = Game::new(export(11, five_v_five()));
        assert_eq!(g.teams.side(11), None);
        assert!(!g.teams_complete());
    }

    #[test]
    fn free_for_all_and_no_spike_are_not_reviewed() {
        // Everyone kills everyone: no two-team split fits.
        let mut ev = vec![round(0, 0), round(100_000, 1)];
        let mut t = 1000;
        for k in 1..=6 {
            for v in 1..=6 {
                if k != v {
                    ev.push(kill(t, k, v));
                    t += 1000;
                }
            }
        }
        let g = Game::new(export(6, ev));
        assert!(g.unsupported_mode().unwrap().contains("two teams"));
        let mut ex = export(10, five_v_five());
        ex.actors.class.clear();
        ex.actors
            .class
            .insert(1, "/Game/Equippables/Guns/Vandal.Vandal_C".into());
        let g = Game::new(ex);
        assert!(g.unsupported_mode().unwrap().contains("no spike"));
        assert!(g.warnings.iter().any(|w| w.starts_with("Vantage only reviews")));
    }
}
