//! `/plan`: a game plan for one map from your own matches there: how your
//! rounds go at each site on each side, what to do and what to avoid. With
//! fewer than `FULL_AT` matches it is the map's general tips instead.

use std::collections::BTreeMap;

use crate::analysis::Side;
use crate::coach::{self, plural};
use crate::export::MatchReview;
use crate::facts::RoundFact;

/// Matches on a map before the plan is your own.
pub const FULL_AT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Good,
    Bad,
    Plain,
}

#[derive(Debug, Clone)]
pub struct SiteLine {
    pub site: String,
    pub won: usize,
    pub of: usize,
    pub note: String,
    pub tone: Tone,
}

#[derive(Debug, Clone, Default)]
pub struct SidePlan {
    pub sites: Vec<SiteLine>,
    pub do_this: Vec<String>,
    pub avoid: Vec<String>,
    /// Attack: the post-plant. Defense: a round to copy.
    pub after: String,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub map: String,
    pub matches: usize,
    pub agents: Vec<String>,
    pub attack: SidePlan,
    pub defense: SidePlan,
    pub deaths: Vec<[f64; 2]>,
    pub wins: Vec<[f64; 2]>,
    pub go_here: Vec<String>,
    pub stay_out: Vec<String>,
    pub one_line: String,
}

/// The site a round was played at: the plant, else the site I spent longest at.
fn site_of(f: &RoundFact) -> Option<String> {
    f.plant_site.clone().or_else(|| {
        f.region_s
            .iter()
            .filter(|(k, s)| k.len() == 1 && **s >= 5.0)
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(k, _)| k.clone())
    })
}

fn side_word(s: Side) -> &'static str {
    match s {
        Side::Attack => "on attack",
        Side::Defense => "on defense",
    }
}

fn top(m: BTreeMap<String, usize>) -> Vec<(String, usize)> {
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

fn side_plan(reviews: &[MatchReview], side: Side) -> SidePlan {
    // (won, of) by site, and where I died / won fights in that site's rounds.
    let mut sites: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut deaths_at: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut death_spots: BTreeMap<String, usize> = BTreeMap::new();
    let mut death_matches: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
    let mut alone_spots: BTreeMap<String, usize> = BTreeMap::new();
    let mut win_spots: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut early = BTreeMap::<String, usize>::new();
    for r in reviews {
        for f in r.round_facts.iter().filter(|f| f.side == Some(side)) {
            let Some(site) = site_of(f) else { continue };
            let e = sites.entry(site.clone()).or_default();
            e.1 += 1;
            if f.won == Some(true) {
                e.0 += 1;
            }
            for d in r.death_facts.iter().filter(|d| d.round == Some(f.round)) {
                if let Some(c) = &d.nearest_callout {
                    *deaths_at.entry(site.clone()).or_default().entry(c.clone()).or_default() += 1;
                }
            }
        }
        for d in &r.death_facts {
            let Some(f) = r.round_facts.iter().find(|f| Some(f.round) == d.round) else {
                continue;
            };
            if f.side != Some(side) {
                continue;
            }
            if let Some(c) = &d.nearest_callout {
                *death_spots.entry(c.clone()).or_default() += 1;
                death_matches
                    .entry(c.clone())
                    .or_default()
                    .insert(r.context.match_id.clone());
                if coach::is_alone(d) {
                    *alone_spots.entry(c.clone()).or_default() += 1;
                }
                if d.context.since_barrier_s.is_some_and(|s| s < 25.0) {
                    *early.entry(c.clone()).or_default() += 1;
                }
            }
        }
        for duel in &r.duels {
            let Some(f) = r.round_facts.iter().find(|f| Some(f.round) == duel.round) else {
                continue;
            };
            if f.side != Some(side) {
                continue;
            }
            if let Some(c) = &duel.callout {
                let e = win_spots.entry(c.clone()).or_default();
                if duel.won { e.0 += 1 } else { e.1 += 1 }
            }
        }
    }

    let rate = |(w, n): (usize, usize)| if n == 0 { 0.0 } else { w as f64 / n as f64 };
    let best = sites
        .iter()
        .filter(|(_, v)| v.1 >= 3)
        .max_by(|a, b| rate(*a.1).total_cmp(&rate(*b.1)))
        .map(|(s, v)| (s.clone(), *v));
    let mut lines = Vec::new();
    for (site, (won, of)) in &sites {
        let p = rate((*won, *of));
        let trap = deaths_at
            .get(site)
            .and_then(|m| top(m.clone()).into_iter().next())
            .filter(|(_, n)| *n * 3 >= *of && *n >= 2);
        let (note, tone) = if best.as_ref().is_some_and(|b| &b.0 == site) && p >= 0.5 {
            ("your best site".to_owned(), Tone::Good)
        } else if p < 0.4 && *of >= 3 {
            match trap {
                Some((c, _)) => (format!("you die at {c} in a lot of these rounds"), Tone::Bad),
                None => ("you lose most rounds here".to_owned(), Tone::Bad),
            }
        } else {
            (String::new(), Tone::Plain)
        };
        lines.push(SiteLine {
            site: site.clone(),
            won: *won,
            of: *of,
            note,
            tone,
        });
    }

    let mut do_this = Vec::new();
    let mut avoid = Vec::new();
    if let Some((site, (w, n))) = &best
        && rate((*w, *n)) >= 0.5
    {
        do_this.push(match side {
            Side::Attack => format!("Push {site} when your team has the choice. You win {w} of {n} rounds there."),
            Side::Defense => format!("Keep holding {site}. You win {w} of {n} rounds there."),
        });
    }
    let mut wins: Vec<_> = win_spots.iter().filter(|(_, (w, l))| *w >= 3 && w > l).collect();
    wins.sort_by_key(|w| std::cmp::Reverse((w.1).0));
    if let Some((c, (w, l))) = wins.first() {
        do_this.push(format!("Keep taking fights at {c}. You won {w} of {} there.", w + l));
    }
    let deaths = top(death_spots);
    if let Some((c, n)) = deaths.first().filter(|(_, n)| *n >= 3) {
        let in_matches = death_matches.get(c).map_or(0, |m| m.len());
        let opening = if early.get(c).is_some_and(|e| e * 2 >= *n) {
            format!("Walking into {c} early with no information.")
        } else {
            format!("Taking fights at {c}.")
        };
        avoid.push(format!(
            "{opening} You've died there {} in {} of your {} matches here.",
            plural(*n, "time"),
            in_matches,
            reviews.len()
        ));
    }
    if let Some((c, _)) = top(alone_spots)
        .into_iter()
        .find(|(c, n)| *n >= 2 && deaths.first().is_none_or(|(d, _)| d != c))
    {
        avoid.push(format!("Going to {c} on your own. Nobody can trade you there."));
    }
    if side == Side::Defense {
        let late: usize = reviews
            .iter()
            .flat_map(|r| &r.round_facts)
            .filter(|f| {
                f.side == Some(Side::Defense)
                    && f.plant_site.is_some()
                    && f.won == Some(false)
                    && f.rotation_s.is_some_and(|s| s >= 12.0)
            })
            .count();
        if late >= 2 {
            avoid.push(format!(
                "Rotating late. {} you reached the spike after it was down and lost the round. Leave as soon as the site is called.",
                coach::capitalise(&plural(late, "time"))
            ));
        }
    }

    let after = match side {
        Side::Attack => {
            let post: Vec<&RoundFact> = reviews
                .iter()
                .flat_map(|r| &r.round_facts)
                .filter(|f| f.side == Some(Side::Attack) && f.plant_site.is_some())
                .collect();
            let w = post.iter().filter(|f| f.won == Some(true)).count();
            match post.len() {
                0 => "You haven't played a post-plant here yet. When you do, play the spike from a spot that sees it, not from on top of it.".to_owned(),
                n if w * 2 >= n => format!("You win {w} of your {n} post-plants here. Keep playing them from a spot that sees the spike, not from on top of it."),
                n => format!("You win only {w} of your {n} post-plants here. Play them from a spot that sees the spike, not from on top of it, and make them come to you."),
            }
        }
        Side::Defense => {
            let best_round = reviews
                .iter()
                .flat_map(|r| r.round_facts.iter().map(move |f| (r, f)))
                .filter(|(_, f)| f.side == Some(Side::Defense) && f.won == Some(true) && !f.died)
                .max_by_key(|(_, f)| f.kills);
            match best_round {
                Some((r, f)) if f.kills >= 2 => {
                    let date = r.context.played_at.as_deref().and_then(|t| t.get(..10)).unwrap_or("?");
                    let at = site_of(f).map(|s| format!(" holding {s}")).unwrap_or_default();
                    format!(
                        "Your best defense round here: round {} on {date}, {}{at}, and you survived. Do that again.",
                        f.round + 1,
                        plural(f.kills as usize, "kill")
                    )
                }
                _ => "No defense round here stands out yet. Hold with a teammate close enough to trade, and play for time.".to_owned(),
            }
        }
    };
    SidePlan {
        sites: lines,
        do_this,
        avoid,
        after,
    }
}

/// The plan for `map` from my reviews there, oldest first.
pub fn plan(map: &str, reviews: &[MatchReview]) -> Plan {
    let mut agents: Vec<String> = Vec::new();
    for r in reviews {
        if !agents.contains(&r.agent) {
            agents.push(r.agent.clone());
        }
    }
    let attack = side_plan(reviews, Side::Attack);
    let defense = side_plan(reviews, Side::Defense);
    let side_of = |r: &MatchReview, round: Option<u32>| {
        r.round_facts
            .iter()
            .find(|f| Some(f.round) == round)
            .and_then(|f| f.side)
    };
    let mut wins: BTreeMap<(String, &'static str), usize> = BTreeMap::new();
    let mut deaths: BTreeMap<(String, &'static str), usize> = BTreeMap::new();
    for r in reviews {
        for d in &r.duels {
            if let (true, Some(c), Some(s)) = (d.won, &d.callout, side_of(r, d.round)) {
                *wins.entry((c.clone(), side_word(s))).or_default() += 1;
            }
        }
        for d in &r.death_facts {
            if let (Some(c), Some(s)) = (&d.nearest_callout, side_of(r, d.round)) {
                *deaths.entry((c.clone(), side_word(s))).or_default() += 1;
            }
        }
    }
    let ranked = |m: BTreeMap<(String, &'static str), usize>, n: usize| {
        let mut v: Vec<_> = m.into_iter().filter(|(_, k)| *k >= 2).collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        v.into_iter()
            .take(n)
            .map(|((c, s), _)| format!("{c}, {s}"))
            .collect::<Vec<_>>()
    };
    let best = |p: &SidePlan| {
        p.sites
            .iter()
            .filter(|s| s.tone == Tone::Good)
            .map(|s| s.site.clone())
            .next()
    };
    let one_line = match (best(&attack), best(&defense)) {
        (Some(a), Some(d)) => format!("Hit {a} on attack, and hold {d} on defense."),
        (Some(a), None) => format!("Hit {a} on attack. On defense, stay where a teammate can trade you."),
        (None, Some(d)) => format!("On defense, hold {d}. On attack, go in with your team and an ability first."),
        (None, None) => "Go in with your team and an ability first, and stay where you can be traded.".to_owned(),
    };
    Plan {
        map: map.to_owned(),
        matches: reviews.len(),
        agents,
        go_here: ranked(wins, 3),
        stay_out: ranked(deaths, 2),
        attack,
        defense,
        deaths: reviews
            .iter()
            .flat_map(|r| r.death_facts.iter().filter_map(|d| d.minimap_uv))
            .collect(),
        wins: reviews
            .iter()
            .flat_map(|r| r.duels.iter().filter(|d| d.won).filter_map(|d| d.uv))
            .collect(),
        one_line,
    }
}

/// What matters on a map for anyone, before there's a plan of your own.
pub fn tips(map: &str) -> &'static [&'static str] {
    match map {
        "Ascent" => &[
            "Mid decides most rounds. Take Market or Mid with a teammate, never alone.",
            "B Main is narrow. Use a smoke or a flash before you walk in.",
            "On defense, the site doors can be closed to slow a push. Use them when you're alone.",
        ],
        "Pearl" => &[
            "Mid is long and open. Don't wander it on your own.",
            "B Long is a long sightline. Peek it briefly for information instead of walking down it.",
            "Retake A through Link with your team, not on your own.",
        ],
        "Haven" => &[
            "Three sites means long rotations. On defense, call early and rotate early.",
            "A Long and C Long are long sightlines. Use utility to cross them.",
            "Garage is how attackers split B and C. Keep an ear on it.",
        ],
        "Sunset" => &[
            "Mid decides where you can attack. Take it with a teammate.",
            "B Main is short. A flash before you swing wins it.",
            "Retake A as a group. Players coming in one at a time get picked off.",
        ],
        "Split" => &[
            "Ropes make fights vertical. Check above you, not just in front.",
            "Mid is how attackers split the sites. Take it with utility.",
            "B Main is a tight choke. Don't stand in it without a smoke or a flash.",
        ],
        "Lotus" => &[
            "The rotating doors are loud. Use the sound to know where the enemy is going.",
            "A Rubble and C Mound are the main fights. Take them with utility.",
            "Three sites means long rotations. On defense, leave early.",
        ],
        "Bind" => &[
            "There's no mid, so the teleporter is how rotations get faked. Listen for it.",
            "Hookah and Showers are tight. Clear them with an ability, not your body.",
            "Retakes are short from spawn, so on defense you can hold back and come in together.",
        ],
        "Icebox" => &[
            "Fights happen at every height. Clear the high spots before the ground.",
            "B Yellow and A Belt are long sightlines. Cross them behind a smoke.",
            "Planting for the default spot on B lets your team see it from far away.",
        ],
        "Breeze" => &[
            "The map is wide and open. Long-range fights decide it, so don't run across open ground.",
            "Mid controls rotations. Take it with a teammate.",
            "Smokes matter more here than anywhere. Use them to cut the long angles.",
        ],
        "Fracture" => &[
            "Attackers come from both sides. On defense, watch your back.",
            "Rope and Dish are quick paths between sites. Listen for them.",
            "Take a site from both sides at once when your team can.",
        ],
        "Abyss" => &[
            "There are no walls at the edges. Don't get knocked or walk off.",
            "Mid is how attackers split the sites. Take it with a teammate.",
            "Long angles cross each site. Use a smoke before you peek them.",
        ],
        _ => &[
            "Go in with a teammate, so whoever dies first gets traded.",
            "Use an ability before you walk onto a site.",
            "On defense, call where they're going early, and rotate early.",
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coach::sample::{death, review};

    #[test]
    fn sites_and_spots() {
        let mut reviews = Vec::new();
        for _ in 0..3 {
            let rounds: Vec<(bool, Side)> = (0..12).map(|i| (i % 2 == 0, Side::Attack)).collect();
            let mut r = review(&rounds, (1..6).map(|i| death(i * 2 + 1, "A Rubble")).collect());
            for f in &mut r.round_facts {
                let site = if f.round % 2 == 0 { "C" } else { "A" };
                f.region_s.insert(site.into(), 20.0);
            }
            reviews.push(r);
        }
        let p = plan("Lotus", &reviews);
        assert_eq!(p.matches, 3);
        let c = p.attack.sites.iter().find(|s| s.site == "C").unwrap();
        assert_eq!((c.won, c.of, c.tone), (18, 18, Tone::Good));
        let a = p.attack.sites.iter().find(|s| s.site == "A").unwrap();
        assert_eq!((a.won, a.of, a.tone), (0, 18, Tone::Bad));
        assert!(a.note.contains("A Rubble"), "{}", a.note);
        assert!(p.attack.do_this[0].starts_with("Push C"));
        assert!(p.attack.avoid[0].contains("A Rubble"), "{:?}", p.attack.avoid);
        assert!(p.one_line.starts_with("Hit C"));
        assert_eq!(tips("Lotus").len(), 3);
    }
}
