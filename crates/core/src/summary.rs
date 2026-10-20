//! All of one player's matches on one map, summarised: what repeats across
//! matches is a habit, not a bad game. Each finding says in how many of the
//! matches it shows up and lists the evidence per match. Rounds count from 1.

use std::collections::BTreeMap;

use crate::analysis::Side;
use crate::facts::RoundFact;
use crate::packet::MatchReview;
use crate::review::{self, Finding, PlayerLine, Stat, head_hits, median, percent, ratio, tally, trade_rate};
use crate::trends::{SpotAcrossMatches, spots_across_matches};

/// One match: my review and the whole lobby's (mine included, or empty).
pub struct Played {
    pub me: MatchReview,
    pub lobby: Vec<MatchReview>,
}

#[derive(Debug, Clone)]
pub struct MatchLine {
    pub id: String,
    /// `10-18 00:22`.
    pub label: String,
    pub agent: String,
    pub won: usize,
    pub lost: usize,
    pub kills: usize,
    pub deaths: usize,
    pub adr: f64,
    /// My damage-per-round place in the lobby.
    pub adr_rank: Option<usize>,
    pub of: usize,
    pub hs: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Summary {
    pub map: String,
    /// `Sova ×3, Fade ×2`.
    pub agents: String,
    /// Newest first.
    pub matches: Vec<MatchLine>,
    pub stats: Vec<Stat>,
    pub work_on: Vec<Finding>,
    pub went_well: Vec<Finding>,
    pub spots: Vec<SpotAcrossMatches>,
    /// Said when there are too few matches for patterns.
    pub note: Option<String>,
}

/// `10-18 00:22` from when the match was played.
pub fn match_label(r: &MatchReview) -> String {
    r.context
        .played_at
        .as_deref()
        .and_then(|t| t.get(5..16))
        .map(|t| t.replacen('T', " ", 1))
        .unwrap_or_else(|| r.context.match_id.chars().take(8).collect())
}

/// `played` must all be on `map`, oldest first.
pub fn summary(map: &str, played: &[Played]) -> Summary {
    let lines: Vec<(PlayerLine, Vec<PlayerLine>)> = played.iter().map(lobby_lines).collect();
    let mut matches: Vec<MatchLine> = played
        .iter()
        .zip(&lines)
        .map(|(p, (mine, all))| {
            let rf = &p.me.round_facts;
            MatchLine {
                id: p.me.context.match_id.clone(),
                label: match_label(&p.me),
                agent: p.me.agent.clone(),
                won: rf.iter().filter(|f| f.won == Some(true)).count(),
                lost: rf.iter().filter(|f| f.won == Some(false)).count(),
                kills: p.me.kills,
                deaths: p.me.deaths,
                adr: mine.adr,
                adr_rank: (all.len() > 1).then(|| all.iter().filter(|o| o.adr > mine.adr).count() + 1),
                of: all.len(),
                hs: mine.hs,
            }
        })
        .collect();
    let mut work_on = work_on(map, played, &lines);
    let mut went_well = went_well(played, &lines);
    work_on.sort_by(|a, b| b.score.total_cmp(&a.score));
    went_well.sort_by(|a, b| b.score.total_cmp(&a.score));
    let stats = stats(played, &lines, &matches);
    matches.reverse();
    let reviews: Vec<MatchReview> = played.iter().map(|p| p.me.clone()).collect();
    Summary {
        map: map.to_owned(),
        agents: tally(played.iter().map(|p| p.me.agent.clone())),
        matches,
        stats,
        work_on,
        went_well,
        spots: spots_across_matches(&reviews),
        note: (played.len() < 3).then(|| {
            format!(
                "Only {} on {map} so far. Patterns across matches need 3 or more; /review covers a single match in more detail.",
                if played.len() == 1 { "1 match".to_owned() } else { format!("{} matches", played.len()) }
            )
        }),
    }
}

/// My line and every line of the lobby for one match.
fn lobby_lines(p: &Played) -> (PlayerLine, Vec<PlayerLine>) {
    let me = &p.me;
    let allies: Vec<String> = me
        .context
        .teams
        .iter()
        .find(|t| t.players.iter().any(|x| x.subject == me.subject))
        .map(|t| t.players.iter().map(|x| x.subject.clone()).collect())
        .unwrap_or_default();
    let mut all: Vec<PlayerLine> = p
        .lobby
        .iter()
        .map(|r| PlayerLine::from_review(r, &me.subject, &allies))
        .collect();
    let mine = PlayerLine::from_review(me, &me.subject, &allies);
    if !all.iter().any(|l| l.me) {
        all.push(mine.clone());
    }
    (mine, all)
}

fn stats(played: &[Played], lines: &[(PlayerLine, Vec<PlayerLine>)], matches: &[MatchLine]) -> Vec<Stat> {
    let n = played.len();
    let rounds: Vec<&RoundFact> = played.iter().flat_map(|p| &p.me.round_facts).collect();
    let won = rounds.iter().filter(|f| f.won == Some(true)).count();
    let side_rate = |s: Side| {
        let rs: Vec<_> = rounds.iter().filter(|f| f.side == Some(s)).collect();
        ratio(rs.iter().filter(|f| f.won == Some(true)).count(), rs.len())
    };
    let kills: usize = played.iter().map(|p| p.me.kills).sum();
    let deaths: usize = played.iter().map(|p| p.me.deaths).sum();
    let (head, hits) = played
        .iter()
        .map(|p| head_hits(&p.me.round_facts))
        .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    let lobby_hs = median(
        lines
            .iter()
            .flat_map(|(_, all)| all.iter().filter(|l| !l.me).filter_map(|l| l.hs))
            .collect(),
    );
    let ranks: Vec<f64> = matches.iter().filter_map(|m| m.adr_rank.map(|r| r as f64)).collect();
    let avg = |v: Vec<f64>| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
    let lobby_avg = |key: fn(&PlayerLine) -> f64| {
        avg(lines
            .iter()
            .flat_map(|(_, all)| all.iter().filter(|l| !l.me).map(key))
            .collect())
    };
    let won_matches = matches.iter().filter(|m| m.won > m.lost).count();
    let mut out = vec![
        Stat {
            label: "Matches",
            value: format!("{n}: won {won_matches}, lost {}", n - won_matches),
            rank: None,
            of: 0,
        },
        Stat {
            label: "Rounds won",
            value: format!(
                "{} (attack {}, defense {})",
                ratio(won, rounds.len()).map_or("n/a".into(), percent),
                side_rate(Side::Attack).map_or("n/a".into(), percent),
                side_rate(Side::Defense).map_or("n/a".into(), percent)
            ),
            rank: None,
            of: 0,
        },
        Stat {
            label: "Damage per round",
            value: format!(
                "{:.0}{}",
                avg(lines.iter().map(|(m, _)| m.adr).collect()).unwrap_or(0.0),
                avg(ranks)
                    .map(|r| format!(" (average place {r:.1})"))
                    .unwrap_or_default()
            ),
            rank: None,
            of: 0,
        },
        Stat {
            label: "Kills / deaths",
            value: format!("{kills}/{deaths} ({:.2})", kills as f64 / deaths.max(1) as f64),
            rank: None,
            of: 0,
        },
        Stat {
            label: "Headshots",
            value: format!(
                "{}{}",
                ratio(head, hits).map_or("n/a".into(), percent),
                lobby_hs
                    .map(|l| format!(" (lobby median {})", percent(l)))
                    .unwrap_or_default()
            ),
            rank: None,
            of: 0,
        },
        Stat {
            label: "First kills a match",
            value: format!(
                "{:.1}{}",
                lines.iter().map(|(m, _)| m.first_kills).sum::<usize>() as f64 / n.max(1) as f64,
                lobby_avg(|l| l.first_kills as f64)
                    .map(|a| format!(" (lobby {a:.1})"))
                    .unwrap_or_default()
            ),
            rank: None,
            of: 0,
        },
        Stat {
            label: "First deaths a match",
            value: format!(
                "{:.1}{}",
                lines.iter().map(|(m, _)| m.first_deaths).sum::<usize>() as f64 / n.max(1) as f64,
                lobby_avg(|l| l.first_deaths as f64)
                    .map(|a| format!(" (lobby {a:.1})"))
                    .unwrap_or_default()
            ),
            rank: None,
            of: 0,
        },
    ];
    if let Some(kst) = avg(lines.iter().filter_map(|(m, _)| m.kst).collect()) {
        out.push(Stat {
            label: "Kill, survive or trade",
            value: format!(
                "{}{}",
                percent(kst),
                avg(lines
                    .iter()
                    .flat_map(|(_, all)| all.iter().filter(|l| !l.me).filter_map(|l| l.kst))
                    .collect())
                .map(|a| format!(" (lobby {})", percent(a)))
                .unwrap_or_default()
            ),
            rank: None,
            of: 0,
        });
    }
    out
}

/// `10-18 00:22 (Sova): text` for each match where `f` has something to say.
fn per_match(played: &[Played], f: impl Fn(&MatchReview) -> Option<String>) -> Vec<String> {
    played
        .iter()
        .rev()
        .filter_map(|p| f(&p.me).map(|t| format!("{} ({}): {t}", match_label(&p.me), p.me.agent)))
        .collect()
}

fn across(n: usize) -> String {
    format!("across {n} match{}", if n == 1 { "" } else { "es" })
}

fn in_matches(k: usize, n: usize) -> String {
    format!("in {k} of {n} match{}", if n == 1 { "" } else { "es" })
}

fn work_on(map: &str, played: &[Played], lines: &[(PlayerLine, Vec<PlayerLine>)]) -> Vec<Finding> {
    let n = played.len();
    let deaths: Vec<_> = played.iter().flat_map(|p| &p.me.death_facts).collect();
    let rounds: Vec<&RoundFact> = played.iter().flat_map(|p| &p.me.round_facts).collect();
    let total_deaths = deaths.len().max(1) as f64;
    let total_rounds = rounds.len().max(1) as f64;
    let mut out = Vec::new();
    let finding = |title: String, evidence: Vec<String>, score: f64| Finding {
        title,
        evidence,
        rounds: vec![],
        score,
    };

    // Spots that repeat across matches.
    let reviews: Vec<MatchReview> = played.iter().map(|p| p.me.clone()).collect();
    // Spots come biggest first: the top three on their own, the rest together.
    let spots = spots_across_matches(&reviews);
    if spots.len() > 3 {
        let rest = &spots[3..];
        out.push(finding(
            format!(
                "Other repeated death spots: {}",
                rest.iter()
                    .map(|s| format!(
                        "{} ×{} ({} matches)",
                        s.callout.as_deref().unwrap_or("?"),
                        s.deaths,
                        s.matches
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            vec!["Each spans two or more of these matches; the Map tab shows them.".into()],
            rest.iter().map(|s| s.deaths).sum::<usize>() as f64 / total_deaths * 0.5,
        ));
    }
    for s in spots.iter().take(3) {
        let mut by_match: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
        for (id, r) in &s.evidence {
            by_match.entry(id.as_str()).or_default().push(r + 1);
        }
        let evidence = per_match(played, |r| {
            by_match.get(r.context.match_id.as_str()).map(|rs| {
                format!(
                    "round{} {}",
                    if rs.len() == 1 { "" } else { "s" },
                    rs.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
                )
            })
        });
        out.push(finding(
            format!(
                "You die at {} {}: {} deaths",
                s.callout.as_deref().unwrap_or("the same spot"),
                in_matches(s.matches, n),
                s.deaths
            ),
            evidence,
            s.matches as f64 / n as f64 + 0.5 * s.deaths as f64 / total_deaths,
        ));
    }

    // An enemy agent who wins most of your fights, whoever plays it.
    let mut by_enemy: BTreeMap<&str, (usize, usize, Vec<&str>)> = BTreeMap::new();
    for p in played {
        for d in &p.me.duels {
            let e = by_enemy.entry(d.enemy_agent.as_str()).or_default();
            if d.won {
                e.0 += 1
            } else {
                e.1 += 1
            }
            if !e.2.contains(&p.me.context.match_id.as_str()) {
                e.2.push(p.me.context.match_id.as_str());
            }
        }
    }
    if let Some((agent, (w, l, ms))) = by_enemy
        .iter()
        .filter(|(_, (w, l, ms))| *l >= 5 && *l >= 2 * *w && ms.len() >= 2)
        .max_by_key(|(_, (w, l, _))| l - w)
    {
        let evidence = per_match(played, |r| {
            let (w, l) = r
                .duels
                .iter()
                .filter(|d| d.enemy_agent == *agent)
                .fold((0, 0), |(w, l), d| if d.won { (w + 1, l) } else { (w, l + 1) });
            (w + l > 0).then(|| format!("you won {w}, they won {l}"))
        });
        out.push(finding(
            format!("Enemy {agent} players beat you {l}–{w} {}", in_matches(ms.len(), n)),
            evidence,
            (l - w) as f64 / total_deaths * 2.0 + ms.len() as f64 / n as f64 * 0.3,
        ));
    }

    // Ult held at death.
    let ult = |r: &MatchReview| {
        r.death_facts
            .iter()
            .filter(|f| f.context.ult_ready == Some(true))
            .count()
    };
    let ult_total: usize = played.iter().map(|p| ult(&p.me)).sum();
    let ult_k = played.iter().filter(|p| ult(&p.me) >= 2).count();
    if ult_total >= 2 * n && ult_k >= 2 {
        out.push(finding(
            format!(
                "{ult_total} deaths with your ultimate ready ({:.1} a match); 2 or more {}",
                ult_total as f64 / n as f64,
                in_matches(ult_k, n)
            ),
            per_match(played, |r| {
                Some(format!(
                    "{} deaths with it ready, used it {} times",
                    ult(r),
                    r.round_facts.iter().filter(|f| f.ult_used).count()
                ))
            }),
            ult_total as f64 / total_rounds * 2.0,
        ));
    }

    // Deaths without hitting the killer.
    let no_hit = |r: &MatchReview| {
        r.death_facts
            .iter()
            .filter(|f| !f.self_kill && f.context.duel_hits_dealt == 0)
            .count()
    };
    let nh_total: usize = played.iter().map(|p| no_hit(&p.me)).sum();
    let nh_k = played
        .iter()
        .filter(|p| p.me.deaths >= 5 && no_hit(&p.me) * 10 >= p.me.deaths * 4)
        .count();
    if deaths.len() >= 10 && nh_total * 10 >= deaths.len() * 4 && nh_k >= 2 {
        out.push(finding(
            format!(
                "{nh_total} of {} deaths without hitting your killer; 40% or more {}",
                deaths.len(),
                in_matches(nh_k, n)
            ),
            per_match(played, |r| Some(format!("{} of {} deaths", no_hit(r), r.deaths))),
            nh_total as f64 / total_deaths,
        ));
    }

    // Caught facing the wrong way.
    let side = |r: &MatchReview| {
        r.death_facts
            .iter()
            .filter(|f| f.crosshair_offset_to_killer_deg.is_some_and(|a| a > 45.0))
            .count()
    };
    let side_total: usize = played.iter().map(|p| side(&p.me)).sum();
    if side_total >= 4 && side_total as f64 >= total_deaths * 0.15 {
        out.push(finding(
            format!("Killed from the side or behind {side_total} times"),
            per_match(played, |r| (side(r) > 0).then(|| format!("{} times", side(r)))),
            side_total as f64 / total_deaths * 1.5,
        ));
    }

    // Weapons and ranges that lose, across matches.
    for what in ["weapon", "range"] {
        let mut m: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for d in played.iter().flat_map(|p| &p.me.duels) {
            let key = if what == "weapon" {
                Some(d.weapon.clone())
            } else {
                d.range.clone()
            };
            if let Some(k) = key {
                let e = m.entry(k).or_default();
                if d.won { e.0 += 1 } else { e.1 += 1 }
            }
        }
        if let Some((k, (w, l))) = m
            .iter()
            .filter(|(_, (w, l))| w + l >= 8 && *w * 100 <= (w + l) * 35)
            .max_by_key(|(_, (w, l))| l - w)
        {
            let others = m
                .iter()
                .filter(|(o, _)| *o != k)
                .map(|(o, (w, l))| format!("{o} {w}–{l}"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push(finding(
                if what == "weapon" {
                    format!("You lose most fights with the {k}: {w}–{l} {}", across(n))
                } else {
                    format!("You lose most fights at {k}: {w}–{l} {}", across(n))
                },
                vec![
                    "Counts kills between you and an enemy only.".into(),
                    format!(
                        "Other {what}s: {}.",
                        if others.is_empty() { "none".into() } else { others }
                    ),
                ],
                (l - w) as f64 / total_deaths * 1.5,
            ));
        }
    }

    // First deaths against the lobby.
    let my_fd: usize = lines.iter().map(|(m, _)| m.first_deaths).sum();
    let lobby_fd: Vec<f64> = lines
        .iter()
        .flat_map(|(_, all)| all.iter().filter(|l| !l.me).map(|l| l.first_deaths as f64))
        .collect();
    if let Some(lobby) = (!lobby_fd.is_empty()).then(|| lobby_fd.iter().sum::<f64>() / lobby_fd.len() as f64) {
        let mine = my_fd as f64 / n as f64;
        if my_fd >= 3 && mine >= lobby * 1.3 {
            out.push(finding(
                format!("First death of the round {mine:.1} times a match; the lobby averages {lobby:.1}"),
                per_match(played, |r| {
                    Some(format!(
                        "{} first deaths",
                        r.round_facts.iter().filter(|f| f.first_death).count()
                    ))
                }),
                (mine - lobby) / lobby.max(0.5) * 0.5,
            ));
        }
    }

    // Headshots below the lobby, match after match.
    let below: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(i, (m, all))| {
            let (_, hits) = head_hits(&played[*i].me.round_facts);
            let lobby = median(all.iter().filter(|l| !l.me).filter_map(|l| l.hs).collect());
            hits >= 20 && m.hs.zip(lobby).is_some_and(|(h, l)| h + 0.05 <= l)
        })
        .map(|(i, _)| i)
        .collect();
    if below.len() >= 2 && below.len() * 10 >= n * 6 {
        out.push(finding(
            format!("Headshots below the lobby median {}", in_matches(below.len(), n)),
            lines
                .iter()
                .enumerate()
                .rev()
                .map(|(i, (m, all))| {
                    let lobby = median(all.iter().filter(|l| !l.me).filter_map(|l| l.hs).collect());
                    format!(
                        "{} ({}): you {}, lobby median {}",
                        match_label(&played[i].me),
                        played[i].me.agent,
                        m.hs.map_or("n/a".into(), percent),
                        lobby.map_or("n/a".into(), percent)
                    )
                })
                .collect(),
            below.len() as f64 / n as f64 * 0.8,
        ));
    }

    // Shooting on the move.
    let shots: u32 = rounds.iter().map(|f| f.shots).sum();
    let moving: u32 = rounds.iter().map(|f| f.moving_shots).sum();
    if shots >= 150 && moving * 100 >= shots * 30 {
        out.push(finding(
            format!(
                "{} of your shots were fired while moving",
                percent(moving as f64 / shots as f64)
            ),
            per_match(played, |r| {
                let s: u32 = r.round_facts.iter().map(|f| f.shots).sum();
                let m: u32 = r.round_facts.iter().map(|f| f.moving_shots).sum();
                ratio(m as usize, s as usize).map(|x| format!("{} of {s} shots", percent(x)))
            }),
            moving as f64 / shots as f64,
        ));
    }

    // Fighting on with a rifle when the round is lost.
    let save = |r: &MatchReview| {
        r.death_facts
            .iter()
            .filter(|f| {
                f.context.inventory_value.is_some_and(|v| v >= 2900) && f.enemies_alive >= f.teammates_alive + 3
            })
            .count()
    };
    let save_total: usize = played.iter().map(|p| save(&p.me)).sum();
    if save_total >= 3 {
        out.push(finding(
            format!("Died {save_total} times with a rifle loadout while down two or more"),
            per_match(played, |r| (save(r) > 0).then(|| format!("{} times", save(r)))),
            save_total as f64 / total_rounds * 2.0,
        ));
    }

    // Isolated deaths.
    let alone = |r: &MatchReview| {
        r.death_facts
            .iter()
            .filter(|f| f.teammates_alive > 0)
            .filter(|f| f.nearest_teammate_m().is_none_or(|m| m > 15.0) && !f.traded_within_3s)
            .count()
    };
    let alone_total: usize = played.iter().map(|p| alone(&p.me)).sum();
    if alone_total >= 5 && alone_total as f64 >= total_deaths * 0.25 {
        out.push(finding(
            format!("{alone_total} deaths with no teammate within 15 m to trade"),
            per_match(played, |r| (alone(r) > 0).then(|| format!("{} deaths", alone(r)))),
            alone_total as f64 / total_deaths,
        ));
    }

    // Late into fights.
    let gaps: Vec<f64> = played
        .iter()
        .filter_map(|p| {
            let rf = &p.me.round_facts;
            Some(
                median(rf.iter().filter_map(|f| f.my_first_contact_s).collect())?
                    - median(rf.iter().filter_map(|f| f.first_contact_s).collect())?,
            )
        })
        .collect();
    if let Some(gap) = median(gaps.clone())
        && gap >= 10.0
        && gaps.len() >= 2
    {
        out.push(finding(
            format!(
                "Your first fight comes {gap:.0} s after the round's first fight ({})",
                in_matches(gaps.iter().filter(|g| **g >= 10.0).count(), n)
            ),
            vec!["Expected for lurkers and anchors; for an entry or initiator it can mean arriving after the fight is decided.".into()],
            (gap / 30.0).min(0.6),
        ));
    }

    // A side that loses on this map.
    let side_wl = |s: Side| {
        let rs: Vec<_> = rounds.iter().filter(|f| f.side == Some(s)).collect();
        (rs.iter().filter(|f| f.won == Some(true)).count(), rs.len())
    };
    let (aw, an) = side_wl(Side::Attack);
    let (dw, dn) = side_wl(Side::Defense);
    if an >= 15 && dn >= 15 {
        let (a, d) = (aw as f64 / an as f64, dw as f64 / dn as f64);
        if (a - d).abs() >= 0.2 {
            let (weak, wr, wn, strong, sr) = if a < d {
                ("attack", aw, an, "defense", d)
            } else {
                ("defense", dw, dn, "attack", a)
            };
            out.push(finding(
                format!(
                    "{} on {map} wins {} of rounds ({wr} of {wn}); {strong} wins {}",
                    review_capitalise(weak),
                    percent(wr as f64 / wn as f64),
                    percent(sr)
                ),
                per_match(played, |r| {
                    let s = if weak == "attack" { Side::Attack } else { Side::Defense };
                    let rs: Vec<_> = r.round_facts.iter().filter(|f| f.side == Some(s)).collect();
                    Some(format!(
                        "{weak} {}–{}",
                        rs.iter().filter(|f| f.won == Some(true)).count(),
                        rs.iter().filter(|f| f.won == Some(false)).count()
                    ))
                }),
                (a - d).abs(),
            ));
        }
    }

    // Sites: retakes on defense and post-plants on attack.
    for (side, kind) in [(Side::Defense, "Retakes"), (Side::Attack, "Post-plants")] {
        let mut by_site: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for f in rounds.iter().filter(|f| f.side == Some(side)) {
            if let Some(site) = f.plant_site.as_deref() {
                let e = by_site.entry(site).or_default();
                match f.won {
                    Some(true) => e.0 += 1,
                    Some(false) => e.1 += 1,
                    None => {}
                }
            }
        }
        let bar = if side == Side::Defense { 25 } else { 40 };
        for (site, (w, l)) in by_site
            .iter()
            .filter(|(_, (w, l))| w + l >= 4 && w * 100 <= (w + l) * bar)
        {
            out.push(finding(
                format!("{kind} on {site}: {w}–{l}"),
                vec![format!(
                    "Rounds where the spike went down on {site} while you were on {}.",
                    if side == Side::Defense { "defense" } else { "attack" }
                )],
                (l - w) as f64 / total_rounds * 4.0,
            ));
        }
    }

    // Close clutches lost.
    let close = |r: &MatchReview| {
        r.round_facts
            .iter()
            .filter(|f| f.clutch_vs.is_some_and(|v| v <= 2) && f.won == Some(false))
            .count()
    };
    let close_total: usize = played.iter().map(|p| close(&p.me)).sum();
    if close_total >= 3 {
        out.push(finding(
            format!("Lost {close_total} 1v1 or 1v2 clutches"),
            per_match(played, |r| (close(r) > 0).then(|| format!("{} lost", close(r)))),
            close_total as f64 / total_rounds * 3.0,
        ));
    }
    out
}

fn went_well(played: &[Played], lines: &[(PlayerLine, Vec<PlayerLine>)]) -> Vec<Finding> {
    let n = played.len();
    let rounds: Vec<&RoundFact> = played.iter().flat_map(|p| &p.me.round_facts).collect();
    let mut out = Vec::new();
    let simple = |title: String, score: f64| Finding {
        title,
        evidence: vec![],
        rounds: vec![],
        score,
    };
    let wl = |rs: &[&&RoundFact]| {
        (
            rs.iter().filter(|f| f.won == Some(true)).count(),
            rs.iter().filter(|f| f.won == Some(false)).count(),
        )
    };
    let pistols: Vec<_> = rounds.iter().filter(|f| f.pistol).collect();
    let (w, l) = wl(&pistols);
    if w + l >= 3 && w > l {
        out.push(simple(format!("Pistol rounds {w}–{l}"), 0.4));
    }
    let post: Vec<_> = rounds
        .iter()
        .filter(|f| f.side == Some(Side::Attack) && f.plant_site.is_some())
        .collect();
    let (w, l) = wl(&post);
    if w + l >= 5 && w * 10 >= (w + l) * 7 {
        out.push(simple(format!("Post-plant {w}–{l}"), 0.5));
    }
    let retake: Vec<_> = rounds
        .iter()
        .filter(|f| f.side == Some(Side::Defense) && f.plant_site.is_some())
        .collect();
    let (w, l) = wl(&retake);
    if w + l >= 5 && w * 2 >= w + l {
        out.push(simple(format!("Retakes {w}–{l}"), 0.6));
    }
    let (mt, mn, tt, tn) = played.iter().fold((0, 0, 0, 0), |a, p| {
        (
            a.0 + p.me.my_trades.0,
            a.1 + p.me.my_trades.1,
            a.2 + p.me.team_trades.0,
            a.3 + p.me.team_trades.1,
        )
    });
    if mn >= 10 && tn > 0 && mt as f64 / mn as f64 > tt as f64 / tn as f64 + 0.05 {
        out.push(simple(
            format!(
                "Traded {} of the time, against {} for your teammates",
                trade_rate((mt, mn)),
                trade_rate((tt, tn))
            ),
            0.5,
        ));
    }
    // (match index, damage place) where there is a lobby to place in.
    let ranks: Vec<(usize, usize)> = lines
        .iter()
        .enumerate()
        .filter(|(_, (_, all))| all.len() >= 6)
        .map(|(i, (m, all))| (i, all.iter().filter(|o| o.adr > m.adr).count() + 1))
        .collect();
    if !ranks.is_empty() {
        let avg = ranks.iter().map(|(_, r)| r).sum::<usize>() as f64 / ranks.len() as f64;
        if avg <= 3.5 {
            out.push(simple(
                format!("Damage per round averages {avg:.1} place in the lobby"),
                0.8,
            ));
        }
    }
    let above = lines
        .iter()
        .filter(|(m, all)| {
            let lobby = median(all.iter().filter(|l| !l.me).filter_map(|l| l.hs).collect());
            m.hs.zip(lobby).is_some_and(|(h, l)| h >= l + 0.05)
        })
        .count();
    if above >= 2 && above * 10 >= n * 6 {
        out.push(simple(
            format!("Headshots above the lobby median {}", in_matches(above, n)),
            0.6,
        ));
    }
    let mut by_weapon: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for d in played.iter().flat_map(|p| &p.me.duels) {
        let e = by_weapon.entry(d.weapon.as_str()).or_default();
        if d.won { e.0 += 1 } else { e.1 += 1 }
    }
    if let Some((k, (w, l))) = by_weapon
        .iter()
        .filter(|(_, (w, l))| w + l >= 8 && w * 100 >= (w + l) * 65)
        .max_by_key(|(_, (w, _))| *w)
    {
        out.push(simple(format!("{k} fights {w}–{l}"), 0.4));
    }
    let won_clutch = rounds
        .iter()
        .filter(|f| f.clutch_vs.is_some() && f.won == Some(true))
        .count();
    if won_clutch >= 2 {
        out.push(simple(format!("Won {won_clutch} clutches"), 0.7));
    }
    let top: Vec<String> = ranks
        .iter()
        .filter(|(_, r)| *r == 1)
        .map(|(i, _)| match_label(&played[*i].me))
        .collect();
    if !top.is_empty() && n >= 2 {
        out.push(simple(format!("Top damage in the lobby: {}", top.join(", ")), 0.3));
    }
    out
}

fn review_capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// `death_line` with the match in front, for lists across matches.
pub fn death_line(r: &MatchReview, f: &crate::deaths::DeathFact) -> String {
    format!("{} · {}", match_label(r), review::death_line(f))
}
