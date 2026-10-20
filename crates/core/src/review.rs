//! A match overview for one player, built in valreview rather than left to an
//! AI: everyone's stats from the stored reviews of the whole lobby, and
//! findings ranked by how far a number sits from the lobby or from a fixed
//! threshold. Every finding names the rounds behind it; none of it is advice
//! the replay cannot show.
//!
//! Rounds are shown counting from 1, as the game does (the export counts from 0).

use std::collections::BTreeMap;

use crate::analysis::Side;
use crate::facts::RoundFact;
use crate::export::MatchReview;

/// One player's line in the scoreboard, from their own stored review.
#[derive(Debug, Clone)]
pub struct PlayerLine {
    pub subject: String,
    pub agent: String,
    pub me: bool,
    pub ally: bool,
    pub kills: usize,
    pub deaths: usize,
    /// Damage per round.
    pub adr: f64,
    /// Headshot hits of all hits on players.
    pub hs: Option<f64>,
    pub first_kills: usize,
    pub first_deaths: usize,
    /// Rounds with a kill, survived, or death traded (no assists in the replay).
    pub kst: Option<f64>,
}

impl PlayerLine {
    pub fn from_review(r: &MatchReview, me: &str, allies: &[String]) -> Self {
        let rf = &r.round_facts;
        let rounds = rf.len().max(1) as f64;
        let (head, hits) = head_hits(rf);
        let traded: Vec<u32> = r
            .death_facts
            .iter()
            .filter(|d| d.traded_within_3s)
            .filter_map(|d| d.round)
            .collect();
        let kst = rf
            .iter()
            .filter(|f| f.kills > 0 || !f.died || traded.contains(&f.round))
            .count();
        PlayerLine {
            subject: r.subject.clone(),
            agent: r.agent.clone(),
            me: r.subject == me,
            ally: r.subject == me || allies.contains(&r.subject),
            kills: r.kills,
            deaths: r.deaths,
            adr: rf.iter().map(|f| f.damage_dealt).sum::<f64>() / rounds,
            hs: ratio(head, hits),
            first_kills: rf.iter().filter(|f| f.first_kill).count(),
            first_deaths: rf.iter().filter(|f| f.first_death).count(),
            kst: (!rf.is_empty()).then(|| kst as f64 / rounds),
        }
    }
}

/// A number of mine and where it ranks in the lobby (1 = best).
#[derive(Debug, Clone)]
pub struct Stat {
    pub label: &'static str,
    pub value: String,
    pub rank: Option<usize>,
    pub of: usize,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub title: String,
    /// Supporting lines: the numbers and, where they help, one per round.
    pub evidence: Vec<String>,
    /// Rounds (from 1) the finding is about.
    pub rounds: Vec<u32>,
    /// Higher sorts first. Comparable only within one list.
    pub score: f64,
}

#[derive(Debug, Clone)]
pub struct Overview {
    pub agent: String,
    pub map: String,
    pub won: usize,
    pub lost: usize,
    pub kills: usize,
    pub deaths: usize,
    pub players: Vec<PlayerLine>,
    pub stats: Vec<Stat>,
    pub work_on: Vec<Finding>,
    pub went_well: Vec<Finding>,
    /// Lobby reviews were available (else ranks and lobby comparisons are missing).
    pub has_lobby: bool,
}

/// Builds the overview for `me`; `lobby` holds every player's review of the
/// same match, `me` included, or is empty when only one review is known.
pub fn overview(me: &MatchReview, lobby: &[MatchReview]) -> Overview {
    let allies: Vec<String> = me
        .context
        .teams
        .iter()
        .find(|t| t.players.iter().any(|p| p.subject == me.subject))
        .map(|t| t.players.iter().map(|p| p.subject.clone()).collect())
        .unwrap_or_default();
    let mut players: Vec<PlayerLine> = lobby
        .iter()
        .map(|r| PlayerLine::from_review(r, &me.subject, &allies))
        .collect();
    if !players.iter().any(|p| p.me) {
        players.push(PlayerLine::from_review(me, &me.subject, &allies));
    }
    players.sort_by(|a, b| b.ally.cmp(&a.ally).then(b.adr.total_cmp(&a.adr)));
    let rf = &me.round_facts;
    let has_lobby = players.len() > 1;
    Overview {
        agent: me.agent.clone(),
        map: me.map_name().trim_matches('`').to_owned(),
        won: rf.iter().filter(|f| f.won == Some(true)).count(),
        lost: rf.iter().filter(|f| f.won == Some(false)).count(),
        kills: me.kills,
        deaths: me.deaths,
        stats: stats(me, &players),
        work_on: sorted(work_on(me, &players)),
        went_well: sorted(went_well(me, &players)),
        players,
        has_lobby,
    }
}

fn sorted(mut v: Vec<Finding>) -> Vec<Finding> {
    v.sort_by(|a, b| b.score.total_cmp(&a.score));
    v
}

fn stats(me: &MatchReview, players: &[PlayerLine]) -> Vec<Stat> {
    let mine = players.iter().find(|p| p.me).expect("me is always in players");
    let of = players.len();
    let rank = |key: &dyn Fn(&PlayerLine) -> Option<f64>, lower_better: bool| -> Option<usize> {
        let v = key(mine)?;
        if of < 2 {
            return None;
        }
        let better = players
            .iter()
            .filter_map(key)
            .filter(|&x| if lower_better { x < v } else { x > v })
            .count();
        Some(better + 1)
    };
    let kd = |p: &PlayerLine| Some(p.kills as f64 / p.deaths.max(1) as f64);
    let (gun_hits, shots) = me
        .round_facts
        .iter()
        .fold((0, 0), |(h, s), f| (h + f.gun_hits_no_shotgun, s + f.shots_no_shotgun));
    vec![
        Stat {
            label: "Damage per round",
            value: format!("{:.0}", mine.adr),
            rank: rank(&|p| Some(p.adr), false),
            of,
        },
        Stat {
            label: "Kills / deaths",
            value: format!("{}/{} ({:.2})", mine.kills, mine.deaths, kd(mine).unwrap_or(0.0)),
            rank: rank(&kd, false),
            of,
        },
        Stat {
            label: "Headshots",
            value: mine.hs.map_or("n/a".into(), percent),
            rank: rank(&|p| p.hs, false),
            of,
        },
        Stat {
            label: "Kill, survive or trade",
            value: mine.kst.map_or("n/a".into(), percent),
            rank: rank(&|p| p.kst, false),
            of,
        },
        Stat {
            label: "First kills",
            value: mine.first_kills.to_string(),
            rank: rank(&|p| Some(p.first_kills as f64), false),
            of,
        },
        Stat {
            label: "First deaths",
            value: mine.first_deaths.to_string(),
            rank: rank(&|p| Some(p.first_deaths as f64), true),
            of,
        },
        Stat {
            label: "Accuracy (no shotguns)",
            value: ratio(gun_hits as usize, shots as usize).map_or("n/a".into(), percent),
            rank: None,
            of,
        },
    ]
}

// ------------------------------------------------------------------ work on

fn work_on(me: &MatchReview, players: &[PlayerLine]) -> Vec<Finding> {
    let d = &me.death_facts;
    let rf = &me.round_facts;
    let deaths = d.len();
    let mut out = Vec::new();

    // Deaths that repeat in one place: big spots on their own, small ones together.
    let (big, small): (Vec<_>, Vec<_>) = me.clusters.iter().partition(|c| c.deaths >= 4);
    if !small.is_empty() {
        let parts: Vec<String> = small
            .iter()
            .map(|c| {
                format!(
                    "{} ×{}",
                    c.nearest_callout.clone().unwrap_or_else(|| format!("spot {}", c.id)),
                    c.deaths
                )
            })
            .collect();
        let here: Vec<_> = d
            .iter()
            .filter(|f| f.cluster_id.is_some_and(|id| small.iter().any(|c| c.id == id)))
            .collect();
        out.push(Finding {
            title: format!("Other repeated death spots: {}", parts.join(", ")),
            evidence: here.iter().map(|f| death_line(f)).collect(),
            rounds: rounds_of(here.iter().filter_map(|f| f.round)),
            score: here.len() as f64 / deaths.max(1) as f64,
        });
    }
    for c in big {
        let place = c.nearest_callout.clone().unwrap_or_else(|| format!("spot {}", c.id));
        let rounds: Vec<u32> = c.rounds.iter().map(|r| r + 1).collect();
        let mut evidence = vec![format!(
            "{} of your {deaths} deaths were within 20 m of each other on the same floor.",
            c.deaths
        )];
        let here: Vec<_> = d.iter().filter(|f| f.cluster_id == Some(c.id)).collect();
        let killers = tally(here.iter().map(|f| f.killer_agent.clone()));
        evidence.push(format!("Killed by {}.", killers));
        for f in &here {
            evidence.push(death_line(f));
        }
        out.push(Finding {
            title: format!("You keep dying at {place}: {} deaths", c.deaths),
            evidence,
            rounds,
            score: 2.0 * c.deaths as f64 / deaths.max(1) as f64 + 0.3,
        });
    }

    // One enemy who won most of your fights.
    let mut by_enemy: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for duel in &me.duels {
        let e = by_enemy.entry(duel.enemy_agent.as_str()).or_default();
        if duel.won { e.0 += 1 } else { e.1 += 1 }
    }
    if let Some((agent, (w, l))) = by_enemy
        .iter()
        .filter(|(_, (w, l))| *l >= 4 && *l >= 2 * *w)
        .max_by_key(|(_, (w, l))| l - w)
    {
        let theirs: Vec<_> = d.iter().filter(|f| f.killer_agent == *agent).collect();
        let mut evidence = vec![
            format!("Kills between you: you won {w}, they won {l}."),
            format!(
                "They killed you with {}.",
                tally(theirs.iter().filter_map(|f| f.context.killer_weapon.clone()))
            ),
            format!(
                "Where: {}.",
                tally(theirs.iter().filter_map(|f| f.nearest_callout.clone()))
            ),
        ];
        let ranges: Vec<f64> = theirs.iter().filter_map(|f| f.dist_to_killer_m).collect();
        if let Some(m) = median(ranges) {
            evidence.push(format!("Median distance when they killed you: {m:.0} m."));
        }
        out.push(Finding {
            title: format!("The enemy {agent} beat you {l}–{w}"),
            evidence,
            rounds: rounds_of(theirs.iter().filter_map(|f| f.round)),
            score: (l - w) as f64 / deaths.max(1) as f64 * 2.0 + 0.2,
        });
    }

    // Ult held at death.
    let ult_deaths: Vec<_> = d.iter().filter(|f| f.context.ult_ready == Some(true)).collect();
    let used = rf.iter().filter(|f| f.ult_used).count();
    if ult_deaths.len() >= 3 {
        out.push(Finding {
            title: format!(
                "{} deaths with your ultimate ready; you used it {used} time{}",
                ult_deaths.len(),
                if used == 1 { "" } else { "s" }
            ),
            evidence: vec![
                "Ready means holding the most ult points you held all match.".into(),
                format!(
                    "Rounds: {}.",
                    list(&rounds_of(ult_deaths.iter().filter_map(|f| f.round)))
                ),
            ],
            rounds: rounds_of(ult_deaths.iter().filter_map(|f| f.round)),
            score: ult_deaths.len() as f64 / rf.len().max(1) as f64 * 2.0,
        });
    }

    // Deaths without hitting the killer.
    let no_hit: Vec<_> = d
        .iter()
        .filter(|f| !f.self_kill && f.context.duel_hits_dealt == 0)
        .collect();
    if deaths >= 5 && no_hit.len() * 10 >= deaths * 4 {
        let side = no_hit
            .iter()
            .filter(|f| f.crosshair_offset_to_killer_deg.is_some_and(|a| a > 45.0))
            .count();
        let moving = no_hit.iter().filter(|f| f.speed_m_s.is_some_and(|s| s > 2.5)).count();
        let mut evidence = vec![format!(
            "{} of {deaths} deaths without a single hit on the player who killed you.",
            no_hit.len()
        )];
        evidence.push(format!(
            "In those: looking more than 45° away from the killer {side}, moving faster than 2.5 m/s {moving}."
        ));
        let first = d
            .iter()
            .filter(|f| f.context.hit_killer_first.is_some())
            .collect::<Vec<_>>();
        if !first.is_empty() {
            evidence.push(format!(
                "In fights where you both landed hits, you hit first {} of {}.",
                first
                    .iter()
                    .filter(|f| f.context.hit_killer_first == Some(true))
                    .count(),
                first.len()
            ));
        }
        out.push(Finding {
            title: format!("{} of {deaths} deaths without hitting your killer", no_hit.len()),
            evidence,
            rounds: rounds_of(no_hit.iter().filter_map(|f| f.round)),
            score: no_hit.len() as f64 / deaths as f64,
        });
    }

    // Caught facing the wrong way.
    let side: Vec<_> = d
        .iter()
        .filter(|f| f.crosshair_offset_to_killer_deg.is_some_and(|a| a > 45.0))
        .collect();
    if side.len() >= 3 {
        out.push(Finding {
            title: format!("Killed from the side or behind {} times", side.len()),
            evidence: side.iter().map(|f| death_line(f)).collect(),
            rounds: rounds_of(side.iter().filter_map(|f| f.round)),
            score: side.len() as f64 / deaths.max(1) as f64 * 1.5,
        });
    }

    // Weapons and ranges that lose.
    for (what, key) in [("weapon", 0), ("range", 1)] {
        let mut m: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for duel in &me.duels {
            let k = if key == 0 {
                Some(duel.weapon.clone())
            } else {
                duel.range.clone()
            };
            if let Some(k) = k {
                let e = m.entry(k).or_default();
                if duel.won { e.0 += 1 } else { e.1 += 1 }
            }
        }
        if let Some((k, (w, l))) = m
            .iter()
            .filter(|(_, (w, l))| w + l >= 6 && *w * 100 <= (w + l) * 35)
            .max_by_key(|(_, (w, l))| l - w)
        {
            let others = m
                .iter()
                .filter(|(o, _)| *o != k)
                .map(|(o, (w, l))| format!("{o} {w}–{l}"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push(Finding {
                title: if what == "weapon" {
                    format!("You lost most fights with the {k}: {w}–{l}")
                } else {
                    format!("You lost most fights at {k}: {w}–{l}")
                },
                evidence: vec![
                    "Counts kills between you and an enemy only (a fight nobody died in is not counted).".into(),
                    format!(
                        "Other {what}s: {}.",
                        if others.is_empty() { "none".into() } else { others }
                    ),
                ],
                rounds: vec![],
                score: (l - w) as f64 / deaths.max(1) as f64 * 1.5,
            });
        }
    }

    // First to die.
    let fd: Vec<&RoundFact> = rf.iter().filter(|f| f.first_death).collect();
    let fd_rank = players.iter().filter(|p| p.first_deaths > fd.len()).count() + 1;
    if fd.len() >= 3 && (players.len() < 2 || fd_rank <= 3) {
        let lost = fd.iter().filter(|f| f.won == Some(false)).count();
        out.push(Finding {
            title: format!("First death of the round {} times", fd.len()),
            evidence: vec![
                format!("Your team lost {lost} of those {} rounds.", fd.len()),
                if players.len() > 1 {
                    format!("{} in the lobby for first deaths.", ordinal(fd_rank))
                } else {
                    String::new()
                },
            ]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect(),
            rounds: rounds_of(fd.iter().map(|f| f.round)),
            score: fd.len() as f64 / rf.len().max(1) as f64 * 3.0,
        });
    }

    // Headshots below the lobby.
    let (head, hits) = head_hits(rf);
    let hs = ratio(head, hits);
    let lobby_hs = median(players.iter().filter(|p| !p.me).filter_map(|p| p.hs).collect());
    if let (Some(hs), Some(lobby)) = (hs, lobby_hs)
        && hits >= 30
        && hs + 0.08 <= lobby
    {
        out.push(Finding {
            title: format!("Headshots {} against a lobby median of {}", percent(hs), percent(lobby)),
            evidence: vec![
                format!("{head} of your {hits} hits on players were headshots."),
                format!(
                    "Vertical aim at death: more than 2° off the killer's head height in {} of {} deaths.",
                    d.iter()
                        .filter(|f| f.context.vertical_aim_offset_deg.is_some_and(|v| v.abs() > 2.0))
                        .count(),
                    d.iter().filter(|f| f.context.vertical_aim_offset_deg.is_some()).count()
                ),
            ],
            rounds: vec![],
            score: (lobby - hs) * 4.0,
        });
    }

    // Shooting on the move.
    let all_shots: u32 = rf.iter().map(|f| f.shots).sum();
    let moving: u32 = rf.iter().map(|f| f.moving_shots).sum();
    if all_shots >= 60 && moving * 100 >= all_shots * 30 {
        out.push(Finding {
            title: format!(
                "{} of your shots were fired while moving",
                percent(moving as f64 / all_shots as f64)
            ),
            evidence: vec![
                format!("{moving} of {all_shots} shots above 1.5 m/s horizontal speed."),
                "Moving shots are far less accurate in Valorant.".into(),
            ],
            rounds: vec![],
            score: moving as f64 / all_shots as f64,
        });
    }

    // Clutches you could have won.
    let close: Vec<&RoundFact> = rf
        .iter()
        .filter(|f| f.clutch_vs.is_some_and(|n| n <= 2) && f.won == Some(false))
        .collect();
    if close.len() >= 2 {
        out.push(Finding {
            title: format!("Lost {} 1v1 or 1v2 clutches", close.len()),
            evidence: close
                .iter()
                .map(|f| format!("Round {}: 1v{}.", f.round + 1, f.clutch_vs.unwrap_or(0)))
                .collect(),
            rounds: rounds_of(close.iter().map(|f| f.round)),
            score: close.len() as f64 / rf.len().max(1) as f64 * 3.0,
        });
    }

    // Fighting on when outnumbered with an expensive loadout.
    let save: Vec<_> = d
        .iter()
        .filter(|f| f.context.inventory_value.is_some_and(|v| v >= 2900) && f.enemies_alive >= f.teammates_alive + 3)
        .collect();
    if save.len() >= 2 {
        out.push(Finding {
            title: format!("Died {} times with a rifle loadout while down two or more", save.len()),
            evidence: vec![
                "Saving the gun in a lost round keeps next round's buy.".into(),
                format!("Rounds: {}.", list(&rounds_of(save.iter().filter_map(|f| f.round)))),
            ],
            rounds: rounds_of(save.iter().filter_map(|f| f.round)),
            score: save.len() as f64 / rf.len().max(1) as f64 * 2.0,
        });
    }

    // Isolated deaths.
    let alone: Vec<_> = d
        .iter()
        .filter(|f| f.teammates_alive > 0)
        .filter(|f| f.nearest_teammate_m().is_none_or(|m| m > 15.0) && !f.traded_within_3s)
        .collect();
    if alone.len() >= 4 {
        out.push(Finding {
            title: format!("{} deaths with no teammate within 15 m to trade", alone.len()),
            evidence: vec![
                format!(
                    "Teammates were alive but none was near enough; none of these deaths was traded within 3 s. Your trade rate overall: {}.",
                    trade_rate(me.my_trades)
                ),
                format!("Rounds: {}.", list(&rounds_of(alone.iter().filter_map(|f| f.round)))),
            ],
            rounds: rounds_of(alone.iter().filter_map(|f| f.round)),
            score: alone.len() as f64 / deaths.max(1) as f64,
        });
    }

    // Late into fights.
    let round_contact = median(rf.iter().filter_map(|f| f.first_contact_s).collect());
    let my_contact = median(rf.iter().filter_map(|f| f.my_first_contact_s).collect());
    if let (Some(r), Some(m)) = (round_contact, my_contact)
        && m - r >= 10.0
    {
        out.push(Finding {
            title: format!("Your first fight came {:.0} s after the round's first fight", m - r),
            evidence: vec![
                format!("Median first contact: the round's {r:.1} s after the barrier, yours {m:.1} s."),
                "Expected for lurkers and anchors; for an entry or initiator it can mean arriving after the fight is decided.".into(),
            ],
            rounds: vec![],
            score: ((m - r) / 30.0).min(0.6),
        });
    }

    // Low damage for the lobby.
    let mine = players.iter().find(|p| p.me);
    if let Some(mine) = mine
        && players.len() >= 6
    {
        let below = players.iter().filter(|p| p.adr > mine.adr).count();
        if below + 1 >= players.len() - 1 {
            out.push(Finding {
                title: format!(
                    "{} damage per round: {} of {} in the lobby",
                    mine.adr.round(),
                    ordinal(below + 1),
                    players.len()
                ),
                evidence: vec![format!(
                    "Lobby median {:.0}.",
                    median(players.iter().map(|p| p.adr).collect()).unwrap_or(0.0)
                )],
                rounds: vec![],
                score: 0.5,
            });
        }
    }
    out
}

// ---------------------------------------------------------------- went well

fn went_well(me: &MatchReview, players: &[PlayerLine]) -> Vec<Finding> {
    let rf = &me.round_facts;
    let mut out = Vec::new();
    let wl = |rs: &[&RoundFact]| {
        (
            rs.iter().filter(|f| f.won == Some(true)).count(),
            rs.iter().filter(|f| f.won == Some(false)).count(),
        )
    };
    let simple = |title: String, score: f64| Finding {
        title,
        evidence: vec![],
        rounds: vec![],
        score,
    };

    let pistols: Vec<&RoundFact> = rf.iter().filter(|f| f.pistol).collect();
    let (w, l) = wl(&pistols);
    if w > l {
        out.push(simple(format!("Pistol rounds {w}–{l}"), 0.4));
    }
    let post: Vec<&RoundFact> = rf
        .iter()
        .filter(|f| f.side == Some(Side::Attack) && f.plant_site.is_some())
        .collect();
    let (w, l) = wl(&post);
    if w + l >= 3 && w > l {
        out.push(simple(format!("Post-plant {w}–{l}"), 0.5));
    }
    let retake: Vec<&RoundFact> = rf
        .iter()
        .filter(|f| f.side == Some(Side::Defense) && f.plant_site.is_some())
        .collect();
    let (w, l) = wl(&retake);
    if w + l >= 3 && w >= l {
        out.push(simple(format!("Retakes {w}–{l}"), 0.5));
    }
    let ((mt, mn), (tt, tn)) = (me.my_trades, me.team_trades);
    if mn >= 5 && tn > 0 && mt as f64 / mn as f64 > tt as f64 / tn as f64 + 0.05 {
        out.push(simple(
            format!(
                "Traded {} of the time, against {} for your teammates",
                trade_rate((mt, mn)),
                trade_rate((tt, tn))
            ),
            0.5,
        ));
    }
    let won_clutch: Vec<String> = rf
        .iter()
        .filter(|f| f.clutch_vs.is_some() && f.won == Some(true))
        .map(|f| format!("1v{} in round {}", f.clutch_vs.unwrap_or(0), f.round + 1))
        .collect();
    if !won_clutch.is_empty() {
        out.push(simple(format!("Won clutches: {}", won_clutch.join(", ")), 0.8));
    }
    if let Some(mine) = players.iter().find(|p| p.me)
        && players.len() >= 6
    {
        let rank = players.iter().filter(|p| p.adr > mine.adr).count() + 1;
        if rank <= 3 {
            out.push(simple(
                format!("{:.0} damage per round, {} in the lobby", mine.adr, ordinal(rank)),
                1.0 - rank as f64 * 0.1,
            ));
        }
        if let (Some(hs), Some(lobby)) = (
            mine.hs,
            median(players.iter().filter(|p| !p.me).filter_map(|p| p.hs).collect()),
        ) && hs >= lobby + 0.05
        {
            out.push(simple(
                format!("Headshots {} against a lobby median of {}", percent(hs), percent(lobby)),
                0.6,
            ));
        }
        let fk = players.iter().filter(|p| p.first_kills > mine.first_kills).count() + 1;
        if mine.first_kills >= 3 && fk <= 2 {
            out.push(simple(
                format!("{} first kills, {} in the lobby", mine.first_kills, ordinal(fk)),
                0.7,
            ));
        }
    }
    let mut by_weapon: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for duel in &me.duels {
        let e = by_weapon.entry(duel.weapon.as_str()).or_default();
        if duel.won { e.0 += 1 } else { e.1 += 1 }
    }
    if let Some((k, (w, l))) = by_weapon
        .iter()
        .filter(|(_, (w, l))| w + l >= 5 && *w >= 2 * *l.max(&1))
        .max_by_key(|(_, (w, _))| *w)
    {
        out.push(simple(format!("{k} fights {w}–{l}"), 0.4));
    }
    out
}

// ------------------------------------------------------------------ helpers

pub(crate) fn head_hits(rf: &[RoundFact]) -> (usize, usize) {
    let head: u32 = rf.iter().map(|f| f.hits_head).sum();
    let all: u32 = rf.iter().map(|f| f.hits_head + f.hits_body + f.hits_legs).sum();
    (head as usize, all as usize)
}

pub(crate) fn ratio(n: usize, d: usize) -> Option<f64> {
    (d > 0).then(|| n as f64 / d as f64)
}

pub fn percent(x: f64) -> String {
    format!("{:.0}%", x * 100.0)
}

pub(crate) fn trade_rate((n, d): (usize, usize)) -> String {
    ratio(n, d).map_or("n/a".into(), percent)
}

pub(crate) fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// Rounds from the replay's 0-based numbers, shown from 1, sorted and unique.
fn rounds_of(it: impl Iterator<Item = u32>) -> Vec<u32> {
    let mut v: Vec<u32> = it.map(|r| r + 1).collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn list(v: &[u32]) -> String {
    v.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
}

/// `Vandal ×3, Phantom ×1`, most common first.
pub fn tally(it: impl Iterator<Item = String>) -> String {
    let mut m: BTreeMap<String, usize> = BTreeMap::new();
    for s in it {
        *m.entry(s).or_insert(0) += 1;
    }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    if v.is_empty() {
        return "unknown".into();
    }
    v.iter()
        .map(|(k, n)| format!("{k} ×{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// One death in a sentence: round, killer, weapon, where, and what stands out.
pub fn death_line(f: &crate::deaths::DeathFact) -> String {
    let mut s = format!(
        "Round {}: {} with {}",
        f.round.map_or("?".into(), |r| (r + 1).to_string()),
        f.killer_agent,
        f.context.killer_weapon.as_deref().unwrap_or("?")
    );
    if let Some(c) = &f.nearest_callout {
        s.push_str(&format!(" at {c}"));
    }
    if let Some(m) = f.dist_to_killer_m {
        s.push_str(&format!(", {m:.0} m"));
    }
    if let Some(a) = f.crosshair_offset_to_killer_deg
        && a > 45.0
    {
        s.push_str(&format!(", looking {a:.0}° away"));
    }
    if f.context.duel_hits_dealt == 0 {
        s.push_str(", no hits on them");
    }
    if f.traded_within_3s {
        s.push_str(", traded");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinals() {
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(2), "2nd");
        assert_eq!(ordinal(3), "3rd");
        assert_eq!(ordinal(4), "4th");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(22), "22nd");
    }

    #[test]
    fn rounds_show_from_one() {
        assert_eq!(rounds_of([3, 0, 3].into_iter()), vec![1, 4]);
    }
}
