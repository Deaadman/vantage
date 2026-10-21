//! How you compare with the other players in your recent games, who are
//! mostly around your rank: what you do better than most, and what you do
//! worse. For `/rank`.

use crate::export::MatchReview;
use crate::review::{advantage, head_hits, opening_duels};

/// One of my matches, with every player's review of it (mine included).
pub struct Game {
    pub me: MatchReview,
    pub lobby: Vec<MatchReview>,
}

#[derive(Debug, Clone, Default)]
pub struct Standing {
    pub better: Vec<String>,
    pub behind: Vec<String>,
    /// Matches with a lobby to compare with.
    pub games: usize,
}

struct Measure {
    /// Higher is better.
    value: fn(&MatchReview) -> Option<f64>,
    better: &'static str,
    behind: &'static str,
}

fn per_round(r: &MatchReview, n: usize) -> Option<f64> {
    (!r.round_facts.is_empty()).then(|| n as f64 / r.round_facts.len() as f64)
}

const MEASURES: [Measure; 8] = [
    Measure {
        value: |r| {
            per_round(r, 0)
                .map(|_| r.round_facts.iter().map(|f| f.damage_dealt).sum::<f64>() / r.round_facts.len() as f64)
        },
        better: "You do more damage than most players in your games.",
        behind: "You do less damage than most players in your games.",
    },
    Measure {
        value: |r| per_round(r, r.round_facts.iter().filter(|f| f.first_death).count()).map(|v| -v),
        better: "You die first in a round less often than most.",
        behind: "You die first in a round more often than almost anyone in your games.",
    },
    Measure {
        value: |r| {
            let (traded, of) = r.my_trades;
            (of >= 3).then(|| traded as f64 / of as f64)
        },
        better: "When you die, a teammate gets the kill back more often than for most players.",
        behind: "When you die, a teammate gets the kill back less often than for most players.",
    },
    Measure {
        value: |r| {
            let (head, hits) = head_hits(&r.round_facts);
            (hits >= 10).then(|| head as f64 / hits as f64)
        },
        better: "More of your shots hit the head than most players'.",
        behind: "Fewer of your shots hit the head than most players'.",
    },
    Measure {
        value: |r| per_round(r, r.round_facts.iter().filter(|f| f.first_kill).count()),
        better: "You open rounds with the first kill more often than most.",
        behind: "You rarely get the first kill of a round.",
    },
    Measure {
        value: |r| {
            let (w, l) = opening_duels(&r.round_facts);
            (w + l >= 3).then(|| w as f64 / (w + l) as f64)
        },
        better: "You win more of your opening duels than most players.",
        behind: "You lose more of your opening duels than most players.",
    },
    Measure {
        value: |r| {
            let (traded, of) = r.trades_given;
            (of >= 3).then(|| traded as f64 / of as f64)
        },
        better: "When a teammate dies near you, you get the kill back more often than most.",
        behind: "When a teammate dies near you, you get the kill back less often than most.",
    },
    Measure {
        value: |r| {
            let (w, n) = advantage(&r.round_facts);
            (n >= 3).then(|| w as f64 / n as f64)
        },
        better: "Your team wins more of its rounds after the first kill than most.",
        behind: "Your team loses more rounds after getting the first kill than most.",
    },
];

/// Where I sit among every player-match in `games`: the share of the others below me.
fn place(games: &[Game], value: fn(&MatchReview) -> Option<f64>) -> Option<f64> {
    let mine: Vec<f64> = games.iter().filter_map(|g| value(&g.me)).collect();
    let others: Vec<f64> = games
        .iter()
        .flat_map(|g| g.lobby.iter().filter(|r| r.subject != g.me.subject))
        .filter_map(value)
        .collect();
    if mine.len() < 2 || others.len() < 10 {
        return None;
    }
    let me = mine.iter().sum::<f64>() / mine.len() as f64;
    Some(others.iter().filter(|v| **v < me).count() as f64 / others.len() as f64)
}

pub fn standing(games: &[Game]) -> Standing {
    let mut out = Standing {
        games: games.iter().filter(|g| g.lobby.len() > 1).count(),
        ..Standing::default()
    };
    for m in &MEASURES {
        match place(games, m.value) {
            Some(p) if p >= 0.7 => out.better.push(m.better.to_owned()),
            Some(p) if p <= 0.3 => out.behind.push(m.behind.to_owned()),
            _ => {}
        }
    }
    out
}
