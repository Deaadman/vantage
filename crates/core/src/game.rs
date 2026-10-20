//! Match-level structure derived from the event timeline: rounds, kills and
//! teams. No movement here.

use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::export::{EventKind, Export, Player};

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

/// Teams inferred by 2-colouring the kill graph. Side numbers are arbitrary
/// labels (0 and 1); they are not attack/defense.
#[derive(Debug, Clone, Default)]
pub struct Teams {
    side: HashMap<u32, u8>,
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
        let teams = infer_teams(&export.manifest.players, &kills, &same_team, &mut warnings);
        Game {
            export,
            rounds,
            kills,
            teams,
            warnings,
        }
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

    /// Kills and deaths per pawn, self-kills counted as a death but not a kill.
    pub fn kd(&self, pawn: u32) -> (usize, usize) {
        let k = self
            .kills
            .iter()
            .filter(|k| k.killer == pawn && !k.is_self_kill())
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

/// Two-colouring with parity: a kill joins opposite teams, a spike-carrier
/// pair the same team. Side labels are arbitrary (0/1), not attack/defense.
fn infer_teams(players: &[Player], kills: &[Kill], same_team: &[(u32, u32)], warnings: &mut Vec<String>) -> Teams {
    // (neighbour, 1 = opposite team, 0 = same team)
    let mut adj: BTreeMap<u32, Vec<(u32, u8)>> = players.iter().map(|p| (p.pawn, Vec::new())).collect();
    for k in kills.iter().filter(|k| !k.is_self_kill()) {
        adj.entry(k.killer).or_default().push((k.victim, 1));
        adj.entry(k.victim).or_default().push((k.killer, 1));
    }
    for &(a, b) in same_team {
        adj.entry(a).or_default().push((b, 0));
        adj.entry(b).or_default().push((a, 0));
    }

    let mut side: HashMap<u32, u8> = HashMap::new();
    let mut player_components = 0;
    let mut isolated = 0;
    // Start from manifest order so the labels are deterministic.
    let starts: Vec<(u32, bool)> = players
        .iter()
        .map(|p| (p.pawn, true))
        .chain(adj.keys().map(|&g| (g, false)))
        .collect();
    for (start, is_player) in starts {
        if side.contains_key(&start) {
            continue;
        }
        if adj[&start].is_empty() && players.len() > 1 {
            // Nothing ties this player to anyone: their team is unknown.
            isolated += usize::from(is_player);
            continue;
        }
        player_components += usize::from(is_player);
        side.insert(start, 0);
        let mut queue = VecDeque::from([start]);
        while let Some(x) = queue.pop_front() {
            let sx = side[&x];
            for &(y, flip) in &adj[&x] {
                if let std::collections::hash_map::Entry::Vacant(e) = side.entry(y) {
                    e.insert(sx ^ flip);
                    queue.push_back(y);
                }
            }
        }
    }

    for k in kills.iter().filter(|k| !k.is_self_kill()) {
        if side.contains_key(&k.killer) && side.get(&k.killer) == side.get(&k.victim) {
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
    if player_components > 1 {
        warnings.push(format!(
            "kills and spike carries split the players into {player_components} groups; teams for some players are a guess"
        ));
    }
    if isolated > 0 {
        warnings.push(format!(
            "{isolated} player(s) had no kills, deaths or spike carries; their team is unknown"
        ));
    }
    Teams { side }
}
