//! Phase 5: aggregate one player's facts across stored matches and compare
//! the last N matches with the N before them. Output is `trends.md`, ranked
//! by how large and how consistent each change is; what a change means is
//! left to the reader.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Serialize;

use crate::analysis::Side;
use crate::clusters::{self, centroid, leader_clusters};
use crate::packet::MatchReview;

pub const TRENDS_SCHEMA_VERSION: u32 = 1;

/// Below these, a number is "not enough data", never a pattern.
pub const MIN_MATCHES: usize = 20;
pub const MIN_EVENTS: f64 = 30.0;
/// Evidence listed per finding.
const MAX_EVIDENCE_MATCHES: usize = 5;
const MAX_EVIDENCE_ROUNDS: usize = 8;
/// A comparable change is ranked when |score| reaches this (about p < 0.05).
pub const MIN_SCORE: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Events out of opportunities (a rate); compared with a two-proportion z-test.
    Rate,
    /// An amount per round; compared with Welch's t on per-match values.
    PerRound,
}

/// One match's contribution to a metric.
#[derive(Debug, Clone, Default)]
pub struct Sample {
    pub num: f64,
    pub den: f64,
    /// Rounds where the counted event happened, and where it could have but did not (rates only).
    pub rounds: Vec<u32>,
    pub misses: Vec<u32>,
}

struct Metric {
    key: &'static str,
    label: &'static str,
    kind: Kind,
    f: fn(&MatchReview) -> Sample,
}

fn rate<T>(
    items: &[T],
    opportunity: impl Fn(&T) -> bool,
    event: impl Fn(&T) -> bool,
    round: impl Fn(&T) -> Option<u32>,
) -> Sample {
    let mut s = Sample::default();
    for it in items.iter().filter(|x| opportunity(x)) {
        s.den += 1.0;
        if event(it) {
            s.num += 1.0;
            s.rounds.extend(round(it));
        } else {
            s.misses.extend(round(it));
        }
    }
    s
}

fn per_round(r: &MatchReview, amount: f64) -> Sample {
    Sample {
        num: amount,
        den: r.round_facts.len() as f64,
        ..Default::default()
    }
}

fn metrics() -> Vec<Metric> {
    macro_rules! m {
        ($key:literal, $label:literal, $kind:ident, $f:expr) => {
            Metric {
                key: $key,
                label: $label,
                kind: Kind::$kind,
                f: $f,
            }
        };
    }
    vec![
        m!(
            "trade_rate",
            "My deaths traded within 3 s, when a teammate was alive",
            Rate,
            |r| {
                let d = &r.death_facts;
                rate(
                    d,
                    |f| !f.self_kill && f.teammates_alive > 0,
                    |f| f.traded_within_3s,
                    |f| f.round,
                )
            }
        ),
        m!("first_death", "Rounds where I was my team's first death", Rate, |r| {
            rate(&r.round_facts, |_| true, |f| f.first_death, |f| Some(f.round))
        }),
        m!("first_kill", "Rounds where I got the round's first kill", Rate, |r| {
            rate(&r.round_facts, |_| true, |f| f.first_kill, |f| Some(f.round))
        }),
        m!("died", "Rounds where I died", Rate, |r| rate(
            &r.round_facts,
            |_| true,
            |f| f.died,
            |f| Some(f.round)
        )),
        m!("kills_per_round", "Kills per round", PerRound, |r| {
            per_round(r, r.round_facts.iter().map(|f| f64::from(f.kills)).sum())
        }),
        m!("adr", "Damage dealt per round", PerRound, |r| per_round(
            r,
            r.round_facts.iter().map(|f| f.damage_dealt).sum()
        )),
        m!(
            "isolated_death",
            "Deaths with the nearest living teammate beyond 15 m",
            Rate,
            |r| {
                rate(
                    &r.death_facts,
                    |f| f.nearest_teammate_m().is_some(),
                    |f| f.nearest_teammate_m().is_some_and(|d| d > 15.0),
                    |f| f.round,
                )
            }
        ),
        m!(
            "facing_away",
            "Deaths with my crosshair more than 45° off the killer",
            Rate,
            |r| {
                rate(
                    &r.death_facts,
                    |f| f.crosshair_offset_to_killer_deg.is_some(),
                    |f| f.crosshair_offset_to_killer_deg.is_some_and(|a| a > 45.0),
                    |f| f.round,
                )
            }
        ),
        m!("never_hit_killer", "Deaths where I never hit my killer", Rate, |r| {
            rate(
                &r.death_facts,
                |f| !f.self_kill,
                |f| f.context.duel_hits_dealt == 0,
                |f| f.round,
            )
        }),
        m!("hit_first", "Fatal duels where I hit the killer first", Rate, |r| {
            rate(
                &r.death_facts,
                |f| f.context.hit_killer_first.is_some(),
                |f| f.context.hit_killer_first == Some(true),
                |f| f.round,
            )
        }),
        m!("headshot_share", "Headshots among my hits on players", Rate, |r| {
            let head: u32 = r.round_facts.iter().map(|f| f.hits_head).sum();
            let all: u32 = r
                .round_facts
                .iter()
                .map(|f| f.hits_head + f.hits_body + f.hits_legs)
                .sum();
            Sample {
                num: f64::from(head),
                den: f64::from(all),
                ..Default::default()
            }
        }),
        m!("accuracy", "Hits per shot (no shotguns)", Rate, |r| {
            let hits: u32 = r.round_facts.iter().map(|f| f.gun_hits_no_shotgun).sum();
            let shots: u32 = r.round_facts.iter().map(|f| f.shots_no_shotgun).sum();
            Sample {
                num: f64::from(hits),
                den: f64::from(shots),
                ..Default::default()
            }
        }),
        m!("moving_shots", "Shots fired faster than 1.5 m/s", Rate, |r| {
            let moving: u32 = r.round_facts.iter().map(|f| f.moving_shots).sum();
            let shots: u32 = r.round_facts.iter().map(|f| f.shots).sum();
            Sample {
                num: f64::from(moving),
                den: f64::from(shots),
                ..Default::default()
            }
        }),
        m!("outnumbered_death", "Deaths while my team was outnumbered", Rate, |r| {
            rate(&r.death_facts, |_| true, |f| f.context.outnumbered, |f| f.round)
        }),
        m!("ult_ready_death", "Deaths holding a ready ult", Rate, |r| {
            rate(
                &r.death_facts,
                |f| f.context.ult_ready.is_some(),
                |f| f.context.ult_ready == Some(true),
                |f| f.round,
            )
        }),
        m!(
            "same_spot_death",
            "Deaths within 5 m of my own earlier kill that round",
            Rate,
            |r| {
                rate(
                    &r.death_facts,
                    |_| true,
                    |f| f.context.same_spot_as_own_kill,
                    |f| f.round,
                )
            }
        ),
        m!(
            "repeat_spot_death",
            "Deaths in a repeated death spot of that match",
            Rate,
            |r| {
                rate(
                    &r.death_facts,
                    |f| f.pos_cm.is_some(),
                    |f| f.cluster_id.is_some(),
                    |f| f.round,
                )
            }
        ),
        m!(
            "most_forward",
            "Rounds I was the most forward at my team's first contact",
            Rate,
            |r| {
                rate(
                    &r.round_facts,
                    |f| f.forward_rank.is_some(),
                    |f| f.forward_rank == Some(1),
                    |f| Some(f.round),
                )
            }
        ),
        m!(
            "late_clock_death",
            "Deaths with 15 s or less on the round clock",
            Rate,
            |r| {
                rate(
                    &r.death_facts,
                    |f| f.context.clock_left_s.is_some(),
                    |f| f.context.clock_left_s.is_some_and(|s| s <= 15.0),
                    |f| f.round,
                )
            }
        ),
        m!("casts_per_round", "Ability casts per round (no ult)", PerRound, |r| {
            let n: u32 = r
                .round_facts
                .iter()
                .map(|f| {
                    f.casts
                        .iter()
                        .filter(|(k, _)| k.as_str() != "X")
                        .map(|(_, v)| v)
                        .sum::<u32>()
                })
                .sum();
            per_round(r, f64::from(n))
        }),
        m!(
            "casts_before_contact",
            "Casts before the round's first hit (share of casts)",
            Rate,
            |r| {
                let before: u32 = r.round_facts.iter().map(|f| f.casts_before_contact).sum();
                let all: u32 = r
                    .round_facts
                    .iter()
                    .map(|f| {
                        f.casts
                            .iter()
                            .filter(|(k, _)| k.as_str() != "X")
                            .map(|(_, v)| v)
                            .sum::<u32>()
                    })
                    .sum();
                Sample {
                    num: f64::from(before),
                    den: f64::from(all),
                    ..Default::default()
                }
            }
        ),
        m!(
            "misaligned_buy",
            "Rounds I bought differently from most of my team",
            Rate,
            |r| {
                let class = |t: &str| match t {
                    "full" | "half" => "buy",
                    "force" => "force",
                    _ => "save",
                };
                rate(
                    &r.round_facts,
                    |f| !f.pistol && !f.team_buys.is_empty(),
                    |f| {
                        let majority = f.team_buys.iter().max_by_key(|(_, n)| **n).map(|(k, _)| k.as_str());
                        majority.is_some_and(|m| class(m) != class(&f.economy.buy_type))
                    },
                    |f| Some(f.round),
                )
            }
        ),
        m!("full_buy_win", "Full-buy rounds won", Rate, |r| {
            rate(
                &r.round_facts,
                |f| f.economy.buy_type == "full" && f.won.is_some(),
                |f| f.won == Some(true),
                |f| Some(f.round),
            )
        }),
        m!("force_win", "Force-buy rounds won", Rate, |r| {
            rate(
                &r.round_facts,
                |f| f.economy.buy_type == "force" && f.won.is_some(),
                |f| f.won == Some(true),
                |f| Some(f.round),
            )
        }),
        m!("pistol_win", "Pistol rounds won", Rate, |r| {
            rate(
                &r.round_facts,
                |f| f.pistol && f.won.is_some(),
                |f| f.won == Some(true),
                |f| Some(f.round),
            )
        }),
        m!("attack_win", "Attack rounds won", Rate, |r| {
            rate(
                &r.round_facts,
                |f| f.side == Some(Side::Attack) && f.won.is_some(),
                |f| f.won == Some(true),
                |f| Some(f.round),
            )
        }),
        m!("defense_win", "Defense rounds won", Rate, |r| {
            rate(
                &r.round_facts,
                |f| f.side == Some(Side::Defense) && f.won.is_some(),
                |f| f.won == Some(true),
                |f| Some(f.round),
            )
        }),
        m!(
            "advantage_conversion",
            "Rounds won after my team got the first kill",
            Rate,
            |r| {
                rate(
                    &r.round_facts,
                    |f| f.team_first_kill == Some(true) && f.won.is_some(),
                    |f| f.won == Some(true),
                    |f| Some(f.round),
                )
            }
        ),
        m!(
            "disadvantage_wins",
            "Rounds won after the enemy got the first kill",
            Rate,
            |r| {
                rate(
                    &r.round_facts,
                    |f| f.team_first_kill == Some(false) && f.won.is_some(),
                    |f| f.won == Some(true),
                    |f| Some(f.round),
                )
            }
        ),
        m!("clutch_win", "Rounds won when I was last alive", Rate, |r| {
            rate(
                &r.round_facts,
                |f| f.clutch_vs.is_some() && f.won.is_some(),
                |f| f.won == Some(true),
                |f| Some(f.round),
            )
        }),
        m!("post_plant_win", "Attack rounds with a plant that we won", Rate, |r| {
            rate(
                &r.round_facts,
                |f| f.side == Some(Side::Attack) && f.plant_site.is_some() && f.won.is_some(),
                |f| f.won == Some(true),
                |f| Some(f.round),
            )
        }),
        m!(
            "retake_win",
            "Defense rounds with an enemy plant that we won",
            Rate,
            |r| {
                rate(
                    &r.round_facts,
                    |f| f.side == Some(Side::Defense) && f.plant_site.is_some() && f.won.is_some(),
                    |f| f.won == Some(true),
                    |f| Some(f.round),
                )
            }
        ),
        m!(
            "rotation_reached",
            "Defense rounds with a plant where I reached the spike",
            Rate,
            |r| {
                rate(
                    &r.round_facts,
                    |f| f.side == Some(Side::Defense) && f.plant_site.is_some(),
                    |f| f.rotation_s.is_some(),
                    |f| Some(f.round),
                )
            }
        ),
        m!(
            "expensive_death_down_two",
            "Deaths with 2,900+ of gear while down two or more players",
            Rate,
            |r| {
                rate(
                    &r.death_facts,
                    |_| true,
                    |f| {
                        f.context.inventory_value.is_some_and(|v| v >= 2900) && f.enemies_alive >= f.teammates_alive + 3
                    },
                    |f| f.round,
                )
            }
        ),
    ]
}

#[derive(Debug, Clone, Default)]
pub struct PerMatch {
    pub match_id: String,
    pub value: f64,
    pub rounds: Vec<u32>,
    pub misses: Vec<u32>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Window {
    pub matches: usize,
    pub num: f64,
    pub den: f64,
    /// num / den (rate) or per round.
    pub value: Option<f64>,
    /// Per-match values (matches with den > 0), oldest first.
    #[serde(skip)]
    pub per_match: Vec<PerMatch>,
}

impl Window {
    fn new(reviews: &[&MatchReview], f: fn(&MatchReview) -> Sample) -> Self {
        let mut w = Window {
            matches: reviews.len(),
            ..Default::default()
        };
        for r in reviews {
            let s = f(r);
            w.num += s.num;
            w.den += s.den;
            if s.den > 0.0 {
                w.per_match.push(PerMatch {
                    match_id: r.context.match_id.clone(),
                    value: s.num / s.den,
                    rounds: s.rounds,
                    misses: s.misses,
                });
            }
        }
        w.value = (w.den > 0.0).then(|| w.num / w.den);
        w
    }

    pub fn enough(&self) -> bool {
        self.matches >= MIN_MATCHES && self.den >= MIN_EVENTS
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub all: Window,
    pub last: Window,
    pub previous: Window,
    /// z (rates) or Welch t (per round), last minus previous.
    pub score: Option<f64>,
    /// Share of the last window's matches on the same side of the previous value as the change.
    pub consistency: Option<f64>,
    /// Both windows meet the sample-size floor.
    pub comparable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpotAcrossMatches {
    pub map: String,
    pub deaths: usize,
    pub matches: usize,
    pub centroid_cm: [f64; 3],
    pub callout: Option<String>,
    /// (match id, round)
    pub evidence: Vec<(String, u32)>,
    /// Deaths on this map in total, for the sample-size label.
    pub map_deaths: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrendReport {
    pub subject: String,
    pub agents: BTreeMap<String, usize>,
    pub matches: usize,
    pub window: usize,
    pub first_played: Option<String>,
    pub last_played: Option<String>,
    pub findings: Vec<Finding>,
    pub spots: Vec<SpotAcrossMatches>,
}

/// `reviews` must be one player's, oldest first.
pub fn trends(subject: &str, reviews: &[MatchReview], window: usize) -> TrendReport {
    let window = window.max(1);
    let all: Vec<&MatchReview> = reviews.iter().collect();
    let split = all.len().saturating_sub(window);
    let last = &all[split..];
    let previous = &all[split.saturating_sub(window)..split];

    let mut findings: Vec<Finding> = metrics()
        .into_iter()
        .map(|m| {
            let a = Window::new(&all, m.f);
            let l = Window::new(last, m.f);
            let p = Window::new(previous, m.f);
            let score = match m.kind {
                Kind::Rate => two_proportion_z(&l, &p),
                Kind::PerRound => welch_t(&l, &p),
            };
            let consistency = match (l.value, p.value) {
                (Some(lv), Some(pv)) if lv != pv && !l.per_match.is_empty() => {
                    let dir = (lv - pv).signum();
                    let same = l.per_match.iter().filter(|m| (m.value - pv).signum() == dir).count();
                    Some(same as f64 / l.per_match.len() as f64)
                }
                _ => None,
            };
            Finding {
                key: m.key,
                label: m.label,
                kind: m.kind,
                comparable: l.enough() && p.enough(),
                all: a,
                last: l,
                previous: p,
                score,
                consistency,
            }
        })
        .collect();
    // Biggest, most consistent changes first; then everything else in catalogue order.
    findings.sort_by(|x, y| {
        let rank = |f: &Finding| {
            if f.comparable {
                f.score.map(f64::abs).unwrap_or(0.0) * f.consistency.unwrap_or(0.0)
            } else {
                -1.0
            }
        };
        rank(y).total_cmp(&rank(x))
    });

    let mut agents = BTreeMap::new();
    for r in reviews {
        *agents.entry(r.agent.clone()).or_insert(0) += 1;
    }
    TrendReport {
        subject: subject.to_owned(),
        agents,
        matches: reviews.len(),
        window,
        first_played: reviews.first().and_then(|r| r.context.played_at.clone()),
        last_played: reviews.last().and_then(|r| r.context.played_at.clone()),
        findings,
        spots: spots_across_matches(reviews),
    }
}

fn two_proportion_z(a: &Window, b: &Window) -> Option<f64> {
    if a.den <= 0.0 || b.den <= 0.0 {
        return None;
    }
    let (p1, p2) = (a.num / a.den, b.num / b.den);
    let p = (a.num + b.num) / (a.den + b.den);
    let se = (p * (1.0 - p) * (1.0 / a.den + 1.0 / b.den)).sqrt();
    (se > 0.0).then(|| (p1 - p2) / se)
}

fn welch_t(a: &Window, b: &Window) -> Option<f64> {
    let stats = |w: &Window| {
        let v: Vec<f64> = w.per_match.iter().map(|x| x.value).collect();
        let n = v.len() as f64;
        if n < 2.0 {
            return None;
        }
        let mean = v.iter().sum::<f64>() / n;
        let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
        Some((mean, var, n))
    };
    let ((m1, v1, n1), (m2, v2, n2)) = (stats(a)?, stats(b)?);
    let se = (v1 / n1 + v2 / n2).sqrt();
    (se > 0.0).then(|| (m1 - m2) / se)
}

/// Death spots that repeat across matches on the same map.
fn spots_across_matches(reviews: &[MatchReview]) -> Vec<SpotAcrossMatches> {
    let mut by_map: BTreeMap<String, Vec<(&MatchReview, &crate::deaths::DeathFact)>> = BTreeMap::new();
    for r in reviews {
        for d in &r.death_facts {
            by_map.entry(r.map_name()).or_default().push((r, d));
        }
    }
    let mut out = Vec::new();
    for (map, deaths) in by_map {
        let points: Vec<Option<[f64; 3]>> = deaths.iter().map(|(_, d)| d.pos_cm).collect();
        for members in leader_clusters(&points) {
            let mut matches: Vec<&str> = members.iter().map(|&i| deaths[i].0.context.match_id.as_str()).collect();
            matches.sort_unstable();
            matches.dedup();
            if matches.len() < 2 {
                continue;
            }
            let mut callouts: BTreeMap<&str, usize> = BTreeMap::new();
            for &i in &members {
                if let Some(c) = deaths[i].1.nearest_callout.as_deref() {
                    *callouts.entry(c).or_insert(0) += 1;
                }
            }
            out.push(SpotAcrossMatches {
                map: map.clone(),
                deaths: members.len(),
                matches: matches.len(),
                centroid_cm: centroid(&points, &members),
                callout: callouts.into_iter().max_by_key(|(_, n)| *n).map(|(c, _)| c.to_owned()),
                evidence: members
                    .iter()
                    .filter_map(|&i| deaths[i].1.round.map(|r| (deaths[i].0.context.match_id.clone(), r)))
                    .collect(),
                map_deaths: deaths.len(),
            });
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.deaths));
    out
}

// ---------------------------------------------------------------- rendering

fn fmt_value(kind: Kind, v: Option<f64>) -> String {
    match (kind, v) {
        (_, None) => "n/a".into(),
        (Kind::Rate, Some(v)) => format!("{:.1}%", v * 100.0),
        (Kind::PerRound, Some(v)) => format!("{v:.2}"),
    }
}

fn fmt_window(kind: Kind, w: &Window) -> String {
    match kind {
        Kind::Rate => format!(
            "{} ({:.0}/{:.0}, {} matches)",
            fmt_value(kind, w.value),
            w.num,
            w.den,
            w.matches
        ),
        Kind::PerRound => format!(
            "{} ({:.0} rounds, {} matches)",
            fmt_value(kind, w.value),
            w.den,
            w.matches
        ),
    }
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

impl TrendReport {
    /// Comparable changes with |score| >= MIN_SCORE, biggest and most consistent first.
    pub fn ranked(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| f.comparable && f.score.is_some_and(|z| z.abs() >= MIN_SCORE))
    }

    pub fn markdown(&self) -> String {
        let mut o = String::new();
        let agents = self
            .agents
            .iter()
            .map(|(a, n)| format!("{a} ×{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(o, "# Trends across {} matches", self.matches);
        let _ = writeln!(o);
        let _ = writeln!(
            o,
            "trends_schema: {TRENDS_SCHEMA_VERSION} · review_schema: {}",
            crate::packet::REVIEW_SCHEMA_VERSION
        );
        let _ = writeln!(o);
        let _ = writeln!(
            o,
            "Player `{}` · agents {agents} · matches played {} to {} · window: last {} vs the {} before",
            self.subject,
            self.first_played.as_deref().unwrap_or("?"),
            self.last_played.as_deref().unwrap_or("?"),
            self.window,
            self.window
        );
        let _ = writeln!(o);
        let _ = writeln!(o, "## How to read this");
        let _ = writeln!(o);
        let _ = writeln!(
            o,
            "Facts from per-match packets, aggregated. Each metric is a count of events out of\n\
             opportunities (rates) or an amount per round. A change is listed only when both\n\
             windows have at least {MIN_MATCHES} matches and {MIN_EVENTS:.0} events; anything smaller is\n\
             \"not enough data\", not a pattern. `score` is a two-proportion z (rates) or\n\
             Welch's t over per-match values (per round); `consistency` is the share of the\n\
             recent matches on the same side of the previous value as the change. Evidence\n\
             is match ids (first 8 characters) and round numbers; `valreview packet --match`\n\
             shows any of them. The caveats in each packet apply here too. What a change\n\
             means is for you to judge."
        );
        let _ = writeln!(o);

        let ranked: Vec<&Finding> = self.ranked().collect();
        let _ = writeln!(o, "## Ranked changes, last {} vs previous {}", self.window, self.window);
        let _ = writeln!(o);
        if ranked.is_empty() {
            if self.findings.iter().any(|f| f.comparable) {
                let _ = writeln!(o, "No metric changed with |score| >= {MIN_SCORE}.");
            } else {
                let _ = writeln!(
                    o,
                    "Not enough data: comparing windows needs {} matches in each ({} stored).",
                    MIN_MATCHES, self.matches
                );
            }
        }
        for (i, f) in ranked.iter().enumerate() {
            let change = match (f.last.value, f.previous.value) {
                (Some(l), Some(p)) => match f.kind {
                    Kind::Rate => format!("{:+.1} points", (l - p) * 100.0),
                    Kind::PerRound => format!("{:+.2}", l - p),
                },
                _ => "n/a".into(),
            };
            let _ = writeln!(
                o,
                "{}. **{}**: {} vs {}; change {change}, score {:+.2}, consistency {}",
                i + 1,
                f.label,
                fmt_window(f.kind, &f.last),
                fmt_window(f.kind, &f.previous),
                f.score.unwrap_or(0.0),
                f.consistency
                    .map(|c| format!("{:.0}%", c * 100.0))
                    .unwrap_or("n/a".into())
            );
            let _ = writeln!(o, "   Evidence: {}", evidence(f));
        }
        let _ = writeln!(o);

        let _ = writeln!(o, "## All metrics");
        let _ = writeln!(o);
        let _ = writeln!(
            o,
            "| metric | all matches | last {} | previous {} | status |",
            self.window, self.window
        );
        let _ = writeln!(o, "|---|---|---|---|---|");
        let mut table: Vec<&Finding> = self.findings.iter().collect();
        table.sort_by_key(|f| metrics().iter().position(|m| m.key == f.key));
        for f in table {
            let status = if f.comparable {
                "compared"
            } else if f.all.enough() {
                "enough overall, not per window"
            } else {
                "not enough data"
            };
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} | {status} |",
                f.label,
                fmt_window(f.kind, &f.all),
                fmt_window(f.kind, &f.last),
                fmt_window(f.kind, &f.previous)
            );
        }
        let _ = writeln!(o);

        let _ = writeln!(o, "## Death spots repeated across matches");
        let _ = writeln!(o);
        let _ = writeln!(
            o,
            "Leader clustering per map ({}+ deaths within {} m and {} m vertically), kept when the spot spans 2+ matches.",
            clusters::MIN_DEATHS,
            clusters::RADIUS_CM / 100.0,
            clusters::FLOOR_TOLERANCE_CM / 100.0
        );
        let _ = writeln!(o);
        if self.spots.is_empty() {
            let _ = writeln!(o, "None yet.");
        }
        for s in &self.spots {
            let [x, y, z] = s.centroid_cm;
            let label = if s.map_deaths as f64 >= MIN_EVENTS {
                ""
            } else {
                " (not enough data on this map)"
            };
            let ev = s
                .evidence
                .iter()
                .map(|(m, r)| format!("{} r{r}", short(m)))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(
                o,
                "- {}: {} of {} deaths on this map, in {} matches, near {} ({x}, {y}, z {z}){label}. Evidence: {ev}",
                s.map,
                s.deaths,
                s.map_deaths,
                s.matches,
                s.callout.as_deref().unwrap_or("?")
            );
        }
        o
    }
}

/// Recent matches furthest in the direction of the change, with the rounds
/// behind it: where the event happened for a rise, where it could have but
/// did not for a fall.
fn evidence(f: &Finding) -> String {
    let Some(prev) = f.previous.value else {
        return "n/a".into();
    };
    let dir = f.last.value.map(|l| (l - prev).signum()).unwrap_or(0.0);
    let mut v: Vec<&PerMatch> = f
        .last
        .per_match
        .iter()
        .filter(|m| (m.value - prev).signum() == dir)
        .collect();
    v.sort_by(|a, b| ((b.value - prev) * dir).total_cmp(&((a.value - prev) * dir)));
    let parts: Vec<String> = v
        .iter()
        .take(MAX_EVIDENCE_MATCHES)
        .map(|m| {
            let (label, rounds) = if dir >= 0.0 {
                ("rounds", &m.rounds)
            } else {
                ("rounds without it", &m.misses)
            };
            let rs = if rounds.is_empty() {
                String::new()
            } else {
                let shown: Vec<String> = rounds.iter().take(MAX_EVIDENCE_ROUNDS).map(u32::to_string).collect();
                let more = if rounds.len() > MAX_EVIDENCE_ROUNDS { ",…" } else { "" };
                format!(" {label} {}{more}", shown.join(","))
            };
            format!("{} ({}{rs})", short(&m.match_id), fmt_value(f.kind, Some(m.value)))
        })
        .collect();
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(num: f64, den: f64, matches: usize) -> Window {
        Window {
            matches,
            num,
            den,
            value: Some(num / den),
            per_match: Vec::new(),
        }
    }

    #[test]
    fn z_test() {
        // 40/100 vs 20/100: pooled p = 0.3, se = sqrt(0.21 * 0.02) = 0.0648, z = 3.09.
        let z = two_proportion_z(&window(40.0, 100.0, 20), &window(20.0, 100.0, 20)).unwrap();
        assert!((z - 3.086).abs() < 0.01, "{z}");
        assert!(two_proportion_z(&window(0.0, 0.0, 20), &window(1.0, 2.0, 20)).is_none());
    }

    #[test]
    fn floors() {
        assert!(window(10.0, 30.0, 20).enough());
        assert!(!window(10.0, 29.0, 20).enough());
        assert!(!window(10.0, 300.0, 19).enough());
    }
}
