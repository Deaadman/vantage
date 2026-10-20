//! The mistakes a player keeps making, by name, counted in every match:
//! `/habits` shows how each is going, `/focus` picks one, and every new
//! match checks the focus first.

use crate::coach;
use crate::export::MatchReview;

/// How many recent matches `/habits` looks at.
pub const WINDOW: usize = 8;
/// "Lately" compares the newest this many matches with the ones before.
pub const RECENT: usize = 3;
/// The focus until one is picked.
pub const DEFAULT_FOCUS: &str = "untraded";

#[derive(Debug, Clone, Copy)]
pub struct Habit {
    pub slug: &'static str,
    pub name: &'static str,
    pub what: &'static str,
    pub why: &'static str,
    pub drill: &'static str,
}

pub const HABITS: [Habit; 8] = [
    Habit {
        slug: "untraded",
        name: "Dying where nobody can trade",
        what: "You died while teammates were alive, but none was close enough to get the kill back.",
        why: "A death your team can't answer is a free kill for the enemy. When someone is close, even losing the fight still evens the round.",
        drill: "Take space a few steps from a teammate, and say which fight you're about to take before you take it.",
    },
    Habit {
        slug: "first-in",
        name: "Going in first with no ability",
        what: "You were the first on your team to meet the enemy on attack, and you hadn't used an ability before the fight.",
        why: "Walking in blind hands the defender the first shot. Abilities are there to take that shot away.",
        drill: "Learn one ability lineup for each site on the maps you play most, and use it before you walk in.",
    },
    Habit {
        slug: "moving",
        name: "Shooting while moving",
        what: "You fired while you were still running.",
        why: "Shots on the move go everywhere. The player who stops first usually wins the fight.",
        drill: "5 minutes in the Range before you queue: strafe, stop, shoot. Tap the opposite key to stop faster.",
    },
    Habit {
        slug: "crosshair",
        name: "Crosshair too low",
        what: "When enemies appeared, your crosshair was usually below their head.",
        why: "Every time you have to flick up, the enemy gets a moment for free.",
        drill: "Walk a map in a custom game with your crosshair on head-height lines: the tops of boxes, the middle of door frames.",
    },
    Habit {
        slug: "repeek",
        name: "Peeking again after a kill",
        what: "You got a kill, stayed where you were, and died in the same spot soon after.",
        why: "After a kill the enemy knows exactly where you are, so your next peek walks into a fight they're ready for.",
        drill: "After every kill, move: step back or change angle before you look again.",
    },
    Habit {
        slug: "ult",
        name: "Dying with your ult ready",
        what: "You died holding a full ultimate.",
        why: "An ult you never use is a whole round's worth of help thrown away.",
        drill: "When your ult is ready, decide at the start of the round where you'll use it.",
    },
    Habit {
        slug: "reload",
        name: "Reloading in the open",
        what: "You died while reloading, or with almost nothing left in your magazine.",
        why: "While you reload you can't fight back, and enemies listen for it.",
        drill: "Reload behind cover, or switch to your pistol to finish a fight.",
    },
    Habit {
        slug: "save",
        name: "Fighting rounds that are lost",
        what: "You were outnumbered with no spike planted, and fought on with a rifle instead of saving it.",
        why: "Losing your rifle in a round you probably couldn't win costs your team the next round's buy too.",
        drill: "At 1 against 3 or worse with no plant, hide and keep your gun.",
    },
];

pub fn habit(slug: &str) -> Option<&'static Habit> {
    HABITS.iter().find(|h| h.slug.eq_ignore_ascii_case(slug.trim()))
}

/// The configured focus, or the default.
pub fn focus(config_focus: Option<&str>) -> &'static Habit {
    config_focus.and_then(habit).unwrap_or_else(|| habit(DEFAULT_FOCUS).expect("a known habit"))
}

/// How often the habit showed in one match; lower is better. `None` when the
/// match can't tell (no shots, no aim data).
pub fn count(slug: &str, r: &MatchReview) -> Option<f64> {
    let deaths = |pred: &dyn Fn(&crate::deaths::DeathFact) -> bool| -> Option<f64> {
        Some(r.death_facts.iter().filter(|d| pred(d)).count() as f64)
    };
    match slug {
        "untraded" => deaths(&coach::is_alone),
        "first-in" => deaths(&|d| coach::is_entry(r, d)),
        "repeek" => deaths(&coach::is_repeek),
        "ult" => deaths(&coach::is_ult_held),
        "reload" => deaths(&coach::is_low_ammo),
        "save" => deaths(&coach::is_lost_round_fight),
        "moving" => {
            let (m, s) = coach::moving_shots(r);
            (s >= 20).then(|| m as f64 / s as f64)
        }
        // Degrees below the head; on target or above counts as 0.
        "crosshair" => coach::crosshair_offset(r).map(|v| (-v).max(0.0)),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trend {
    Better,
    Worse,
    Steady,
}

impl Trend {
    pub fn word(self) -> &'static str {
        match self {
            Trend::Better => "better",
            Trend::Worse => "worse",
            Trend::Steady => "steady",
        }
    }

    pub fn lately(self) -> &'static str {
        match self {
            Trend::Better => "getting better",
            Trend::Worse => "getting worse",
            Trend::Steady => "about the same",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub habit: &'static Habit,
    /// One value per recent match that could tell, oldest first.
    pub values: Vec<f64>,
    /// How it is lately, in words: `2 a match`.
    pub now: String,
    pub trend: Trend,
}

/// `2 a match`, `about 1 in 3 shots`, `a little low`.
pub fn describe(slug: &str, v: f64) -> String {
    match slug {
        "moving" => match v {
            v if v < 0.15 => "rarely".into(),
            v if v < 0.27 => "about 1 in 5 shots".into(),
            v if v < 0.4 => "about 1 in 3 shots".into(),
            v if v < 0.6 => "about half your shots".into(),
            _ => "most of your shots".into(),
        },
        "crosshair" => match v {
            v if v < 1.0 => "on target".into(),
            v if v < 3.0 => "a little low".into(),
            _ => "low".into(),
        },
        _ => match v {
            v if v < 0.3 => "almost never".into(),
            v if v < 1.5 => "about 1 a match".into(),
            v => format!("{} a match", v.round() as u32),
        },
    }
}

fn mean(v: &[f64]) -> Option<f64> {
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

/// Each habit over the newest `WINDOW` matches; `reviews` oldest first.
pub fn progress(reviews: &[MatchReview]) -> Vec<Progress> {
    let recent = &reviews[reviews.len().saturating_sub(WINDOW)..];
    HABITS
        .iter()
        .map(|h| {
            let values: Vec<f64> = recent.iter().filter_map(|r| count(h.slug, r)).collect();
            let split = values.len().saturating_sub(RECENT);
            let (before, lately) = values.split_at(split);
            let trend = match (mean(before), mean(lately)) {
                (Some(b), Some(l)) => {
                    // A tenth of the old level, and at least a small step, counts as a change.
                    let step = (b * 0.1).max(if h.slug == "moving" { 0.03 } else { 0.3 });
                    if l < b - step {
                        Trend::Better
                    } else if l > b + step {
                        Trend::Worse
                    } else {
                        Trend::Steady
                    }
                }
                _ => Trend::Steady,
            };
            Progress {
                habit: h,
                now: mean(lately).map_or_else(|| "not enough matches".into(), |v| describe(h.slug, v)),
                values,
                trend,
            }
        })
        .collect()
}

/// The newest match against the focus: its count, and the ones before it.
#[derive(Debug, Clone)]
pub struct Check {
    pub habit: &'static Habit,
    pub this_match: Option<f64>,
    /// The matches before it (up to `WINDOW - 1`), oldest first.
    pub before: Vec<f64>,
    /// Matches in a row, ending with this one, at or under the average before.
    pub streak: usize,
}

impl Check {
    pub fn usual(&self) -> Option<f64> {
        mean(&self.before)
    }

    /// `Better than before.`, `Same as before.` or `Worse than before.`
    pub fn verdict(&self) -> &'static str {
        match (self.this_match, self.usual()) {
            (Some(n), Some(u)) if n + 0.5 < u => "Better than before.",
            (Some(n), Some(u)) if n > u + 0.5 => "Worse than before.",
            (Some(_), Some(_)) => "Same as before.",
            _ => "The first match it's checked in.",
        }
    }
}

/// Checks `focus` in the newest of `reviews` (oldest first).
pub fn check(focus: &'static Habit, reviews: &[MatchReview]) -> Option<Check> {
    let (newest, older) = reviews.split_last()?;
    let older = &older[older.len().saturating_sub(WINDOW - 1)..];
    let before: Vec<f64> = older.iter().filter_map(|r| count(focus.slug, r)).collect();
    let mut streak = 0;
    let all: Vec<Option<f64>> = older
        .iter()
        .chain(std::iter::once(newest))
        .map(|r| count(focus.slug, r))
        .collect();
    let base = mean(&before);
    for v in all.iter().rev() {
        match (v, base) {
            (Some(v), Some(b)) if *v <= b => streak += 1,
            _ => break,
        }
    }
    Some(Check {
        habit: focus,
        this_match: count(focus.slug, newest),
        before,
        streak,
    })
}

/// A sparkline of values, oldest first.
pub fn spark(values: &[f64]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    values
        .iter()
        .map(|v| {
            if hi - lo < 1e-9 {
                BARS[3]
            } else {
                BARS[((v - lo) / (hi - lo) * 7.0).round() as usize]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Side;
    use crate::coach::tests::{death, review};

    fn with_alone(n: usize) -> MatchReview {
        let deaths = (0..n as u32)
            .map(|r| {
                let mut d = death(r, "Mid");
                d.nearest_teammates.clear();
                d
            })
            .collect();
        review(&[(true, Side::Attack); 13], deaths)
    }

    #[test]
    fn counts_and_trends() {
        let reviews: Vec<MatchReview> = [6, 5, 5, 4, 4, 3, 2, 1].map(with_alone).into();
        assert_eq!(count("untraded", &reviews[0]), Some(6.0));
        let p = progress(&reviews);
        let untraded = p.iter().find(|p| p.habit.slug == "untraded").unwrap();
        assert_eq!(untraded.values, [6.0, 5.0, 5.0, 4.0, 4.0, 3.0, 2.0, 1.0]);
        assert_eq!(untraded.trend, Trend::Better);
        assert_eq!(untraded.now, "2 a match");
        let c = check(focus(None), &reviews).unwrap();
        assert_eq!(c.this_match, Some(1.0));
        assert_eq!(c.verdict(), "Better than before.");
        assert!(c.streak >= 3);
        assert_eq!(spark(&[1.0, 2.0, 3.0]), "▁▅█");
    }

    #[test]
    fn habits_by_slug() {
        assert_eq!(habit("First-In").unwrap().name, "Going in first with no ability");
        assert!(habit("nope").is_none());
        assert_eq!(focus(Some("nope")).slug, DEFAULT_FOCUS);
        assert_eq!(describe("moving", 0.34), "about 1 in 3 shots");
    }
}
