//! The mistakes a player keeps making, by name, counted in every match:
//! `/habits` shows how each is going, `/focus` picks one, and every new
//! match checks the focus first.

use crate::analysis::Side;
use crate::coach;
use crate::export::MatchReview;

/// How many recent matches `/habits` looks at.
pub const WINDOW: usize = 8;
/// "Lately" compares the newest this many matches with the ones before.
pub const RECENT: usize = 3;
/// The focus until one is picked.
pub const DEFAULT_FOCUS: &str = "untraded";
/// Killed by someone this far off your crosshair counts as looking the wrong way.
pub const WRONG_WAY_DEG: f64 = 60.0;
/// A match needs this many hits on players to tell how many were headshots.
const MIN_HITS: u32 = 20;

/// The groups `/habits` lists the habits under, in order.
pub const GROUPS: [&str; 4] = ["Positioning", "Utility", "Aim", "Rounds and economy"];

#[derive(Debug, Clone, Copy)]
pub struct Habit {
    pub slug: &'static str,
    pub name: &'static str,
    /// One of `GROUPS`.
    pub group: &'static str,
    pub what: &'static str,
    pub why: &'static str,
    pub drill: &'static str,
}

/// Every habit, group by group.
pub const HABITS: [Habit; 16] = [
    Habit {
        slug: "untraded",
        name: "Dying where nobody can trade",
        group: "Positioning",
        what: "You died while teammates were alive, but none was close enough to get the kill back.",
        why: "A death your team can't answer is a free kill for the enemy. When someone is close, even losing the fight still evens the round.",
        drill: "Take space a few steps from a teammate, and say which fight you're about to take before you take it.",
    },
    Habit {
        slug: "wrong-way",
        name: "Caught looking the wrong way",
        group: "Positioning",
        what: "You were killed from the side or behind: your crosshair was more than 60° away from the player who killed you.",
        why: "You can't win a fight you don't see coming. Usually it means you checked one angle and left another open behind you.",
        drill: "Before you hold an angle, ask which other angles can see you. Clear them, or pick a spot where only one can.",
    },
    Habit {
        slug: "wallbang",
        name: "Holding spots enemies can shoot through",
        group: "Positioning",
        what: "You were killed through a wall or a box.",
        why: "Some cover only hides you. Players at every rank spam the common wallbang spots once they hear you.",
        drill: "When you're hit through cover, don't stay: that spot is known. Learn which walls on your maps a rifle goes through.",
    },
    Habit {
        slug: "push-out",
        name: "Pushing out on defense",
        group: "Positioning",
        what: "On defense you were your team's first death, further forward than your teammates.",
        why: "Early picks are what attackers want. Dying forward on defense leaves your site a player down before they even arrive.",
        drill: "Hold your first angle from site. Push only with a teammate and an ability, and fall back after the first shot.",
    },
    Habit {
        slug: "repeek",
        name: "Peeking again after a kill",
        group: "Positioning",
        what: "You got a kill, stayed where you were, and died in the same spot soon after.",
        why: "After a kill the enemy knows exactly where you are, so your next peek walks into a fight they're ready for.",
        drill: "After every kill, move: step back or change angle before you look again.",
    },
    Habit {
        slug: "first-in",
        name: "Going in first with no ability",
        group: "Utility",
        what: "You were the first on your team to meet the enemy on attack, and you hadn't used an ability before the fight.",
        why: "Walking in blind hands the defender the first shot. Abilities are there to take that shot away.",
        drill: "Learn one ability lineup for each site on the maps you play most, and use it before you walk in.",
    },
    Habit {
        slug: "utility",
        name: "Dying with abilities unused",
        group: "Utility",
        what: "You died in a round where you hadn't used any of your abilities.",
        why: "Utility you take to the grave is credits and help thrown away. A flash or a smoke in that fight could have changed it.",
        drill: "Before each fight, use one ability. If you're about to die anyway, throw what you have for your team.",
    },
    Habit {
        slug: "ult",
        name: "Dying with your ult ready",
        group: "Utility",
        what: "You died holding a full ultimate.",
        why: "An ult you never use is a whole round's worth of help thrown away.",
        drill: "When your ult is ready, decide at the start of the round where you'll use it.",
    },
    Habit {
        slug: "rotation",
        name: "Rotating late",
        group: "Utility",
        what: "On defense, after the spike went down, you took a long time to get within 15 m of it.",
        why: "Every second after the plant is a second the attackers get to set up their crossfire.",
        drill: "Leave as soon as the plant is called, and take the quickest route, not the safest-looking one.",
    },
    Habit {
        slug: "moving",
        name: "Shooting while moving",
        group: "Aim",
        what: "You fired while you were still running.",
        why: "Shots on the move go everywhere. The player who stops first usually wins the fight.",
        drill: "5 minutes in the Range before you queue: strafe, stop, shoot. Tap the opposite key to stop faster.",
    },
    Habit {
        slug: "crosshair",
        name: "Crosshair too low",
        group: "Aim",
        what: "When enemies appeared, your crosshair was usually below their head.",
        why: "Every time you have to flick up, the enemy gets a moment for free.",
        drill: "Walk a map in a custom game with your crosshair on head-height lines: the tops of boxes, the middle of door frames.",
    },
    Habit {
        slug: "bodies",
        name: "Aiming at bodies",
        group: "Aim",
        what: "Most of your hits landed on bodies and legs instead of heads.",
        why: "A Vandal headshot kills in one bullet; body shots take four. Fewer headshots means longer fights you lose more often.",
        drill: "Aim for head height before the enemy appears, and tap or burst at range instead of spraying.",
    },
    Habit {
        slug: "reload",
        name: "Reloading in the open",
        group: "Aim",
        what: "You died while reloading, or with almost nothing left in your magazine.",
        why: "While you reload you can't fight back, and enemies listen for it.",
        drill: "Reload behind cover, or switch to your pistol to finish a fight.",
    },
    Habit {
        slug: "save",
        name: "Fighting rounds that are lost",
        group: "Rounds and economy",
        what: "You were outnumbered with no spike planted, and fought on with a rifle instead of saving it.",
        why: "Losing your rifle in a round you probably couldn't win costs your team the next round's buy too.",
        drill: "At 1 against 3 or worse with no plant, hide and keep your gun.",
    },
    Habit {
        slug: "post-plant",
        name: "Peeking after the plant",
        group: "Rounds and economy",
        what: "On attack after the plant, you went looking for a fight and died while teammates were still alive.",
        why: "After the plant the defenders have to come to you. Peeking gives up that advantage.",
        drill: "After the plant, hold an angle on the spike and let the defenders walk into you.",
    },
    Habit {
        slug: "buy-sync",
        name: "Buying out of sync with your team",
        group: "Rounds and economy",
        what: "You bought when most of your team saved, or saved when most of them bought.",
        why: "Five players on different buys lose to five on the same one. Your full buy next round matters more than a lone rifle now.",
        drill: "Before you buy, check what your team is doing, and match them.",
    },
];

pub fn habit(slug: &str) -> Option<&'static Habit> {
    HABITS.iter().find(|h| h.slug.eq_ignore_ascii_case(slug.trim()))
}

/// The configured focus, or the default.
pub fn focus(config_focus: Option<&str>) -> &'static Habit {
    config_focus
        .and_then(habit)
        .unwrap_or_else(|| habit(DEFAULT_FOCUS).expect("a known habit"))
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
        "wrong-way" => deaths(&|d| !d.self_kill && d.crosshair_offset_to_killer_deg.is_some_and(|a| a > WRONG_WAY_DEG)),
        "wallbang" => deaths(&|d| d.context.killing_blow_wallbang),
        "push-out" => deaths(&|d| {
            let round = r.round_facts.iter().find(|f| Some(f.round) == d.round);
            d.context.side == Some(Side::Defense)
                && !d.context.post_plant
                && round.is_some_and(|f| f.first_death && f.forward_rank == Some(1))
        }),
        "post-plant" => deaths(&|d| {
            d.context.side == Some(Side::Attack) && d.context.post_plant && d.teammates_alive > 0 && !d.traded_within_3s
        }),
        "utility" => Some(
            r.round_facts
                .iter()
                .filter(|f| f.died && f.casts.iter().filter(|(k, _)| k.as_str() != "X").all(|(_, n)| *n == 0))
                .count() as f64,
        ),
        "buy-sync" => Some(r.round_facts.iter().filter(|f| out_of_sync(f)).count() as f64),
        // Seconds from the plant to within 15 m of the spike, on defense.
        "rotation" => mean(&r.round_facts.iter().filter_map(|f| f.rotation_s).collect::<Vec<_>>()),
        // The share of hits on players that weren't headshots.
        "bodies" => {
            let (head, all) = r.round_facts.iter().fold((0, 0), |(h, a), f| {
                (h + f.hits_head, a + f.hits_head + f.hits_body + f.hits_legs)
            });
            (all >= MIN_HITS).then(|| 1.0 - head as f64 / all as f64)
        }
        "moving" => {
            let (m, s) = coach::moving_shots(r);
            (s >= 20).then(|| m as f64 / s as f64)
        }
        // Degrees below the head; on target or above counts as 0.
        "crosshair" => coach::crosshair_offset(r).map(|v| (-v).max(0.0)),
        _ => None,
    }
}

/// A buy against most of the team: buying while 3 or more teammates save, or
/// saving while 3 or more buy in full. Pistol rounds don't count.
fn out_of_sync(f: &crate::facts::RoundFact) -> bool {
    let team = |kinds: &[&str]| -> u32 {
        f.team_buys
            .iter()
            .filter(|(k, _)| kinds.contains(&k.as_str()))
            .map(|(_, n)| n)
            .sum()
    };
    let saving = ["save", "eco"];
    match f.economy.buy_type.as_str() {
        _ if f.pistol => false,
        "full" | "half" | "force" => team(&saving) >= 3,
        "save" | "eco" => team(&["full"]) >= 3,
        _ => false,
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
        "bodies" => format!("{:.0}% headshots", (1.0 - v) * 100.0),
        "rotation" => format!("{:.0} s to the spike", v),
        "buy-sync" => match v {
            v if v < 0.3 => "almost never".into(),
            v if v < 1.5 => "about 1 round a match".into(),
            v => format!("{} rounds a match", v.round() as u32),
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
                    let step = (b * 0.1).max(match h.slug {
                        "moving" | "bodies" => 0.03,
                        "rotation" => 1.0,
                        _ => 0.3,
                    });
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
    use crate::coach::sample::{death, review};

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
    fn the_new_habits_count_from_the_stored_facts() {
        let rounds = [
            (true, Side::Attack),
            (false, Side::Attack),
            (false, Side::Defense),
            (true, Side::Defense),
        ];
        // Round 0: killed from behind, through a wall. Round 1: after the plant, teammates alive.
        // Round 2: on defense, first and furthest forward.
        let mut behind = death(0, "Mid");
        behind.crosshair_offset_to_killer_deg = Some(120.0);
        behind.context.killing_blow_wallbang = true;
        let mut post_plant = death(1, "A Site");
        post_plant.context.side = Some(Side::Attack);
        post_plant.context.post_plant = true;
        let mut forward = death(2, "B Main");
        forward.context.side = Some(Side::Defense);
        forward.first_death_of_round = true;
        let mut r = review(&rounds, vec![behind, post_plant, forward]);
        r.round_facts[2].forward_rank = Some(1);
        // Abilities used in round 1 only; out of sync in round 3: a full buy while three teammates save.
        r.round_facts[1].casts.insert("Q".into(), 2);
        r.round_facts[3].economy.buy_type = "full".into();
        r.round_facts[3].team_buys.insert("save".into(), 3);
        r.round_facts[3].rotation_s = Some(9.0);
        // 6 headshots of 24 hits.
        r.round_facts[0].hits_head = 6;
        r.round_facts[0].hits_body = 16;
        r.round_facts[0].hits_legs = 2;

        let n = |slug: &str| count(slug, &r);
        assert_eq!(n("wrong-way"), Some(1.0));
        assert_eq!(n("wallbang"), Some(1.0));
        assert_eq!(n("post-plant"), Some(1.0));
        assert_eq!(n("push-out"), Some(1.0));
        assert_eq!(n("utility"), Some(2.0), "rounds 0 and 2: died with no ability used");
        assert_eq!(n("buy-sync"), Some(1.0));
        assert_eq!(n("rotation"), Some(9.0));
        assert_eq!(n("bodies"), Some(0.75));
        assert_eq!(describe("bodies", 0.75), "25% headshots");
        assert_eq!(describe("rotation", 9.0), "9 s to the spike");
        assert_eq!(describe("buy-sync", 3.0), "3 rounds a match");
        // Too few hits to tell.
        r.round_facts[0].hits_body = 2;
        assert_eq!(count("bodies", &r), None);
        // Every habit is counted, under a group, in group order.
        for h in &HABITS {
            assert!(GROUPS.contains(&h.group), "{}", h.slug);
            assert!(
                count(h.slug, &review(&rounds, vec![])).is_some()
                    || ["moving", "crosshair", "rotation", "bodies"].contains(&h.slug),
                "{}",
                h.slug
            );
        }
        let order: Vec<usize> = HABITS
            .iter()
            .map(|h| GROUPS.iter().position(|g| *g == h.group).unwrap())
            .collect();
        assert!(order.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn habits_by_slug() {
        assert_eq!(habit("First-In").unwrap().name, "Going in first with no ability");
        assert!(habit("nope").is_none());
        assert_eq!(focus(Some("nope")).slug, DEFAULT_FOCUS);
        assert_eq!(describe("moving", 0.34), "about 1 in 3 shots");
    }
}
