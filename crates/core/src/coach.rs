//! The debrief `/review` shows for one match, in plain English: the short
//! version, fixes ranked by the rounds they cost, the deaths that decided a
//! round, and what to keep doing. Built only from what the stored review
//! holds; every fix names its rounds.
//!
//! Rounds count from 1 here, as in the game (the review counts from 0).

use std::collections::BTreeMap;

use crate::analysis::Side;
use crate::deaths::DeathFact;
use crate::export::MatchReview;
use crate::facts::RoundFact;

/// A teammate this far away or further couldn't have traded.
pub const TRADE_DISTANCE_M: f64 = 15.0;
/// Gear worth this much is a rifle buy.
pub const RIFLE_LOADOUT: u32 = 2900;
/// Faster than this at death is running, not walking.
pub const RUNNING_M_S: f64 = 2.5;

/// How far into the round a death came.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Early,
    Mid,
    Late,
}

impl When {
    pub fn words(self) -> &'static str {
        match self {
            When::Early => "early in the round",
            When::Mid => "mid-round",
            When::Late => "late in the round",
        }
    }
}

/// One of my deaths, with what the debrief says about it.
#[derive(Debug, Clone)]
pub struct Death {
    pub round: u32,
    pub side: Option<Side>,
    pub won: Option<bool>,
    pub killer: String,
    pub place: Option<String>,
    pub when: When,
    pub first: bool,
    /// Teammates were alive but none near enough to trade, and nobody did.
    pub alone: bool,
    pub traded: bool,
    /// The fix and moment this death is part of, as indexes.
    pub fix: Option<usize>,
    pub moment: Option<usize>,
    pub uv: Option<[f64; 2]>,
    /// Match time of the death, for finding its clip.
    pub time_ms: u32,
}

impl Death {
    /// `Raze killed you at A Main, early in the round. Nobody was close enough to trade.`
    pub fn sentence(&self) -> String {
        let mut s = format!(
            "{} killed you at {}, {}",
            self.killer,
            self.place.as_deref().unwrap_or("an unknown spot"),
            self.when.words()
        );
        if self.first {
            s.push_str(", the first death of the round");
        }
        s.push_str(if self.alone {
            ". Nobody was close enough to trade."
        } else if self.traded {
            ". A teammate got the kill back."
        } else {
            "."
        });
        s
    }
}

#[derive(Debug, Clone)]
pub struct Fix {
    /// Which detector found it: the habit slug where it is one, else `enemy` or `rotate`.
    pub kind: &'static str,
    pub title: String,
    /// What it cost, short: `lost 4 of 5`.
    pub cost: String,
    /// Lower case, for the short version: `going into A Main first with no ability out`.
    pub phrase: String,
    pub short: String,
    pub try_short: String,
    pub happened: String,
    pub why: String,
    pub tries: Vec<String>,
    pub check: String,
    /// The `/habits` habit this fix feeds, if any.
    pub habit: Option<&'static str>,
    pub rounds: Vec<u32>,
    pub lost: usize,
}

#[derive(Debug, Clone)]
pub struct Moment {
    pub round: u32,
    /// `1 against 3, 21 seconds left, no plant`.
    pub situation: String,
    /// `hide and keep your Vandal`.
    pub better: String,
    pub text: String,
    pub kind: &'static str,
}

#[derive(Debug, Clone)]
pub struct RoundLine {
    pub round: u32,
    pub side: Option<Side>,
    pub won: Option<bool>,
    /// `pistol`, `full buy`, `half buy`, `force buy`, `eco` or `save`.
    pub buy: String,
    pub kills: u32,
    /// Index into `Debrief::deaths`.
    pub death: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Debrief {
    pub match_id: String,
    pub agent: String,
    pub map: String,
    pub played_at: Option<String>,
    pub won: usize,
    pub lost: usize,
    pub kills: usize,
    pub deaths_n: usize,
    pub summary: String,
    pub fixes: Vec<Fix>,
    pub moments: Vec<Moment>,
    pub keep: Vec<String>,
    pub deaths: Vec<Death>,
    pub rounds: Vec<RoundLine>,
    /// Where I stood when I won a fight.
    pub wins: Vec<[f64; 2]>,
    /// The callout where I won the most fights, and where I died the most.
    pub best_spot: Option<String>,
    pub worst_spot: Option<(String, usize)>,
}

impl Debrief {
    pub fn result(&self) -> &'static str {
        result_word(self.won, self.lost)
    }
}

pub fn result_word(won: usize, lost: usize) -> &'static str {
    match won.cmp(&lost) {
        std::cmp::Ordering::Greater => "won",
        std::cmp::Ordering::Less => "lost",
        std::cmp::Ordering::Equal => "drew",
    }
}

pub fn map_name(r: &MatchReview) -> String {
    r.map_name()
        .trim_matches('`')
        .rsplit('/')
        .next()
        .unwrap_or("?")
        .to_owned()
}

// ------------------------------------------------------------ what counts

fn round_of<'a>(r: &'a MatchReview, d: &DeathFact) -> Option<&'a RoundFact> {
    r.round_facts.iter().find(|f| Some(f.round) == d.round)
}

/// Teammates alive, none near enough to trade, and nobody did.
pub fn is_alone(d: &DeathFact) -> bool {
    !d.self_kill
        && d.teammates_alive > 0
        && !d.traded_within_3s
        && d.nearest_teammate_m().is_none_or(|m| m > TRADE_DISTANCE_M)
}

/// First of my team to die on attack, with no ability used before the round's first fight.
pub fn is_entry(r: &MatchReview, d: &DeathFact) -> bool {
    let Some(f) = round_of(r, d) else { return false };
    d.first_death_of_round
        && f.first_death
        && f.side == Some(Side::Attack)
        && !d.traded_within_3s
        && f.casts_before_contact == 0
        && d.context.casts_last_5s == 0
}

/// Died in the same spot as my own kill earlier in the round.
pub fn is_repeek(d: &DeathFact) -> bool {
    d.context.same_spot_as_own_kill
}

/// Last alive against 3 or more with no plant, holding a rifle loadout.
pub fn is_lost_round_fight(d: &DeathFact) -> bool {
    d.teammates_alive == 0
        && d.enemies_alive >= 3
        && !d.context.post_plant
        && d.context.inventory_value.is_some_and(|v| v >= RIFLE_LOADOUT)
}

pub fn is_ult_held(d: &DeathFact) -> bool {
    d.context.ult_ready == Some(true)
}

/// A fifth of the magazine or less left.
pub fn is_low_ammo(d: &DeathFact) -> bool {
    match (d.context.magazine_ammo, d.context.magazine_size) {
        (Some(a), Some(s)) if s >= 5 => a * 5 <= s as i64,
        _ => false,
    }
}

/// Shots fired on the move, and all shots.
pub fn moving_shots(r: &MatchReview) -> (u32, u32) {
    r.round_facts
        .iter()
        .fold((0, 0), |(m, s), f| (m + f.moving_shots, s + f.shots))
}

/// The median vertical crosshair offset from the killer's head at death (negative: below).
pub fn crosshair_offset(r: &MatchReview) -> Option<f64> {
    crate::review::median(
        r.death_facts
            .iter()
            .filter_map(|d| d.context.vertical_aim_offset_deg)
            .collect(),
    )
}

/// Defense rounds with a plant where I got to the spike late, and the round was lost.
fn late_rotations(r: &MatchReview) -> Vec<&RoundFact> {
    r.round_facts
        .iter()
        .filter(|f| f.side == Some(Side::Defense) && f.plant_site.is_some() && f.won == Some(false))
        .filter(|f| f.rotation_s.is_some_and(|s| s >= 12.0))
        .collect()
}

// ------------------------------------------------------------- wording

const NUMBER_WORDS: [&str; 11] = [
    "no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
];

pub fn words(n: usize) -> String {
    NUMBER_WORDS.get(n).map_or_else(|| n.to_string(), |w| (*w).to_owned())
}

pub fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// `1 match`, `2 matches`, `3 deaths`.
pub fn plural(n: usize, word: &str) -> String {
    let es = ["s", "x", "ch", "sh"].iter().any(|e| word.ends_with(e));
    format!(
        "{n} {word}{}",
        if n == 1 {
            ""
        } else if es {
            "es"
        } else {
            "s"
        }
    )
}

/// `3, 7 and 12`.
pub fn list_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn rounds_list(rounds: &[u32]) -> String {
    list_and(&rounds.iter().map(u32::to_string).collect::<Vec<_>>())
}

/// `about a third` of something, for a share.
pub fn share_words(p: f64) -> &'static str {
    if p >= 0.6 {
        "most"
    } else if p >= 0.45 {
        "about half"
    } else if p >= 0.3 {
        "about a third"
    } else if p >= 0.2 {
        "about a quarter"
    } else {
        "a few"
    }
}

/// The most common value, ties broken alphabetically.
fn most_common(it: impl Iterator<Item = String>) -> Option<(String, usize)> {
    let mut m: BTreeMap<String, usize> = BTreeMap::new();
    for s in it {
        *m.entry(s).or_default() += 1;
    }
    m.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
}

fn when(d: &DeathFact) -> When {
    match d.context.since_barrier_s {
        _ if d.context.post_plant => When::Late,
        Some(s) if s < 20.0 => When::Early,
        Some(s) if s > 60.0 => When::Late,
        _ => When::Mid,
    }
}

fn buy_words(t: &str) -> String {
    match t {
        "full" => "full buy",
        "half" => "half buy",
        "force" => "force buy",
        "pistol" => "pistol",
        "eco" => "eco",
        "save" => "save",
        _ => "?",
    }
    .to_owned()
}

/// What the debrief calls this agent's abilities.
struct Kit {
    ult: String,
}

fn kit(r: &MatchReview) -> Kit {
    Kit {
        ult: r.abilities.get("X").cloned().unwrap_or_else(|| "your ult".into()),
    }
}

/// A teammate to go in with: a duelist if there was one.
fn partner(r: &MatchReview) -> String {
    let team = r
        .context
        .teams
        .iter()
        .find(|t| t.players.iter().any(|p| p.subject == r.subject));
    team.and_then(|t| {
        t.players
            .iter()
            .filter(|p| p.subject != r.subject)
            .max_by_key(|p| p.role.as_deref() == Some("Duelist"))
            .map(|p| p.agent.clone())
    })
    .unwrap_or_else(|| "teammate".into())
}

// --------------------------------------------------------------- fixes

/// A fix found in the match: the card, and the deaths it is about.
struct Found {
    fix: Fix,
    deaths: Vec<usize>,
}

fn rounds_of(r: &MatchReview, deaths: &[&DeathFact]) -> (Vec<u32>, usize) {
    let mut rounds: Vec<u32> = deaths.iter().filter_map(|d| d.round.map(|x| x + 1)).collect();
    rounds.sort_unstable();
    rounds.dedup();
    let lost = rounds
        .iter()
        .filter(|&&n| r.round_facts.iter().any(|f| f.round + 1 == n && f.won == Some(false)))
        .count();
    (rounds, lost)
}

fn indexes(r: &MatchReview, pred: impl Fn(&DeathFact) -> bool) -> Vec<usize> {
    r.death_facts
        .iter()
        .enumerate()
        .filter(|(_, d)| pred(d))
        .map(|(i, _)| i)
        .collect()
}

fn find_fixes(r: &MatchReview) -> Vec<Found> {
    let d = &r.death_facts;
    let kit = kit(r);
    let partner = partner(r);
    let mut out = Vec::new();
    let pick = |ix: &[usize]| ix.iter().map(|&i| &d[i]).collect::<Vec<_>>();

    // Going in first with no ability.
    let ix = indexes(r, |x| is_entry(r, x));
    if ix.len() >= 2 {
        let ds = pick(&ix);
        let (rounds, lost) = rounds_of(r, &ds);
        let spot = most_common(ds.iter().filter_map(|x| x.nearest_callout.clone()))
            .filter(|(_, n)| *n * 2 >= ds.len())
            .map(|(s, _)| s);
        let at = spot.as_deref().map(|s| format!(" at {s}")).unwrap_or_default();
        let into = spot.as_deref().map(|s| format!(" into {s}")).unwrap_or_default();
        let n = rounds.len();
        out.push(Found {
            fix: Fix {
                kind: "first-in",
                title: match &spot {
                    Some(s) => format!("First into {s} alone, with no ability first"),
                    None => "Going in first alone, with no ability first".into(),
                },
                cost: format!("lost {lost} of {n}"),
                phrase: format!("going in first{into} with no ability out"),
                short: format!(
                    "In {} attack rounds you were the first of your team to meet the enemy{at}, and you died before anyone could get the kill back. You hadn't used an ability first.",
                    words(n)
                ),
                try_short: format!(
                    "Use an ability{} before anyone walks in, then go in second, a few steps behind your {partner}.",
                    spot.as_deref().map(|s| format!(" on {s}")).unwrap_or_default()
                ),
                happened: format!(
                    "Rounds {}. You were the first of your team to meet the enemy{at}. Your nearest teammate was too far back to trade, so the enemy got a free kill.",
                    rounds_list(&rounds)
                ),
                why: format!(
                    "Dying first with nobody near turns 5 against 5 into 4 against 5, and your team learns nothing from it. You lost {lost} of these {n} rounds."
                ),
                tries: vec![
                    "Use an ability before the push, so you know who's there before you walk in.".into(),
                    format!("Go in second, a few steps behind your {partner}, so one of you can trade the other."),
                    "If you have to go first, peek for a moment to see who's there, then step back instead of taking the fight.".into(),
                ],
                check: "Next match: go in first with no ability at most once.".into(),
                habit: Some("first-in"),
                rounds,
                lost,
            },
            deaths: ix,
        });
    }

    // Peeking again after a kill.
    let ix = indexes(r, is_repeek);
    if ix.len() >= 2 {
        let ds = pick(&ix);
        let (rounds, lost) = rounds_of(r, &ds);
        let n = ix.len();
        out.push(Found {
            fix: Fix {
                kind: "repeek",
                title: "Peeking again right after a kill".into(),
                cost: format!("lost {lost} of {}", rounds.len()),
                phrase: "peeking the same angle again after a kill".into(),
                short: format!(
                    "{} times you got a kill, stayed where you were, and died in the same spot soon after.",
                    capitalise(&words(n))
                ),
                try_short: "After a kill, move: step back or change angle before you look again.".into(),
                happened: format!(
                    "Rounds {}. You got a kill, then died within a few steps of it: the enemy knew where you were and was ready for you.",
                    rounds_list(&rounds)
                ),
                why: format!(
                    "After a kill the enemy knows exactly where you are, so the next peek walks into a fight they're ready for. You lost {lost} of those {} rounds.",
                    rounds.len()
                ),
                tries: vec![
                    "After every kill, step back or move to a different angle before you peek again.".into(),
                    "If you want that angle back, use an ability first so they have to deal with it.".into(),
                ],
                check: "Next match: no deaths in the same spot as your own kill.".into(),
                habit: Some("repeek"),
                rounds,
                lost,
            },
            deaths: ix,
        });
    }

    // Fighting on in rounds that were lost.
    let ix = indexes(r, is_lost_round_fight);
    if ix.len() >= 2 {
        let ds = pick(&ix);
        let (rounds, lost) = rounds_of(r, &ds);
        let n = ix.len();
        out.push(Found {
            fix: Fix {
                kind: "save",
                title: "Fighting on in rounds that were lost".into(),
                cost: format!("{} rifles lost", words(n)),
                phrase: "fighting on in rounds that were already lost".into(),
                short: format!(
                    "{} times you were last alive against 3 or more, with no spike down, and you took the fight instead of saving your rifle.",
                    capitalise(&words(n))
                ),
                try_short: "At 1 against 3 or worse with no plant, hide and keep your gun.".into(),
                happened: format!(
                    "Rounds {}. You were the last one alive, outnumbered with the spike not planted, holding a rifle.",
                    rounds_list(&rounds)
                ),
                why: "You lost those fights, and your team couldn't afford as good a buy the round after. Rounds like that are almost never won; the rifle would have been worth more next round.".into(),
                tries: vec![
                    "When it's 1 against 3 or worse and there's no plant, hide and save.".into(),
                    "Saving your rifle means a full buy next round, even after the loss.".into(),
                ],
                check: "Next match: save every rifle in a lost round.".into(),
                habit: Some("save"),
                rounds,
                lost,
            },
            deaths: ix,
        });
    }

    // One enemy who won most of the fights.
    let mut by_enemy: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for duel in &r.duels {
        let e = by_enemy.entry(duel.enemy_agent.as_str()).or_default();
        if duel.won { e.0 += 1 } else { e.1 += 1 }
    }
    if let Some((agent, (w, l))) = by_enemy
        .iter()
        .filter(|(_, (w, l))| *l >= 4 && *l >= 2 * *w)
        .max_by_key(|(_, (w, l))| l - w)
    {
        let agent = (*agent).to_owned();
        let ix = indexes(r, |x| x.killer_agent == agent);
        let ds = pick(&ix);
        let (rounds, lost) = rounds_of(r, &ds);
        let range = crate::review::median(ds.iter().filter_map(|x| x.dist_to_killer_m).collect()).map_or("", |m| {
            if m < 10.0 {
                "up close"
            } else if m < 25.0 {
                "at mid range"
            } else {
                "from long range"
            }
        });
        let place = most_common(ds.iter().filter_map(|x| x.nearest_callout.clone()))
            .filter(|(_, n)| *n >= 2)
            .map(|(s, _)| format!(" around {s}"))
            .unwrap_or_default();
        out.push(Found {
            fix: Fix {
                kind: "enemy",
                title: format!("Taking {agent}'s fights on their terms"),
                cost: format!("lost {lost} of {}", rounds.len()),
                phrase: format!("taking fights the enemy {agent} was ready for"),
                short: format!(
                    "{agent} killed you {} times, {range}{place}. You won {} of your fights with them. They were set up and waiting, and you walked into them.",
                    words(*l),
                    words(*w)
                ),
                try_short: format!(
                    "Find {agent} with an ability before you swing, and fight them from an angle they don't expect."
                ),
                happened: format!(
                    "{agent} killed you {} times, mostly {range}{place}. You got {} of them.",
                    words(*l),
                    words(*w)
                ),
                why: format!(
                    "You kept peeking the spot they were holding, so they had the first shot. You lost {lost} of those rounds."
                ),
                tries: vec![
                    format!("Don't peek {agent}'s angle without information. Find them first."),
                    "Take fights from close, off-angle spots, where they can't line up the first shot.".into(),
                ],
                check: "Next match: watch for the player who keeps beating you, and stop giving them the same fight."
                    .into(),
                habit: None,
                rounds,
                lost,
            },
            deaths: ix,
        });
    }

    // Shooting on the move.
    let (moving, shots) = moving_shots(r);
    if shots >= 60 && moving * 100 >= shots * 30 {
        let p = moving as f64 / shots as f64;
        out.push(Found {
            fix: Fix {
                kind: "moving",
                title: "Shooting while still moving".into(),
                cost: format!("{} of shots", share_words(p)),
                phrase: "shooting while still moving".into(),
                short: format!(
                    "{} of your shots were fired while you were moving, and those rarely hit.",
                    capitalise(share_words(p))
                ),
                try_short: "Stop, then shoot. Tap the opposite movement key to stop faster.".into(),
                happened: format!(
                    "{moving} of your {shots} shots were fired while you were moving faster than a walk."
                ),
                why: "Guns in VALORANT are inaccurate while you move. The player who stops first usually wins.".into(),
                tries: vec![
                    "Counter-strafe: tap the opposite movement key, then shoot.".into(),
                    "5 minutes of strafe, stop, shoot on bots in the Range before you queue.".into(),
                ],
                check: "Next match: stop before every shot, even in a rushed fight.".into(),
                habit: Some("moving"),
                rounds: vec![],
                lost: 0,
            },
            deaths: vec![],
        });
    }

    // Dying where nobody could trade.
    let ix = indexes(r, is_alone);
    if ix.len() >= 3 {
        let ds = pick(&ix);
        let (rounds, lost) = rounds_of(r, &ds);
        let place = most_common(ds.iter().filter_map(|x| x.nearest_callout.clone()))
            .filter(|(_, n)| *n * 2 >= ds.len() && *n >= 3)
            .map(|(s, _)| s);
        let in_place = place.as_deref().map(|s| format!(" in {s}")).unwrap_or_default();
        let n = ix.len();
        out.push(Found {
            fix: Fix {
                kind: "untraded",
                title: format!("Dying where nobody could trade you{}", place.as_deref().map(|s| format!(", in {s}")).unwrap_or_default()),
                cost: format!("lost {lost} of {}", rounds.len()),
                phrase: format!("dying{in_place} too far from your team to be traded"),
                short: format!(
                    "{} times you died with teammates alive, but none was close enough to get the kill back.",
                    capitalise(&words(n))
                ),
                try_short: "Play a few steps from a teammate, so if you die, they can kill your killer.".into(),
                happened: format!(
                    "Rounds {}. You died{in_place} while your teammates were alive but more than {TRADE_DISTANCE_M:.0} m away, and nobody got the kill back.",
                    rounds_list(&rounds)
                ),
                why: format!(
                    "A death nobody can answer is a free kill for the enemy. You lost {lost} of those {} rounds.",
                    rounds.len()
                ),
                tries: vec![
                    "Stay close enough that a teammate could swing the moment you die.".into(),
                    "Say where you're going before you go there, so someone can follow.".into(),
                ],
                check: "Next match: no more than 2 deaths where nobody could trade you.".into(),
                habit: Some("untraded"),
                rounds,
                lost,
            },
            deaths: ix,
        });
    }

    // Rotating late.
    let late = late_rotations(r);
    if late.len() >= 2 {
        let to = most_common(late.iter().filter_map(|f| f.plant_site.clone())).map(|(s, _)| s);
        let from = most_common(late.iter().filter_map(|f| {
            f.region_s
                .iter()
                .filter(|(k, _)| Some(*k) != to.as_ref() && k.len() == 1)
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(k, _)| k.clone())
        }))
        .map(|(s, _)| s);
        let to_w = to.clone().unwrap_or_else(|| "the site".into());
        let from_w = from.clone().unwrap_or_else(|| "your site".into());
        let mut rounds: Vec<u32> = late.iter().map(|f| f.round + 1).collect();
        rounds.sort_unstable();
        let n = rounds.len();
        let ix: Vec<usize> = indexes(r, |x| x.round.is_some_and(|rd| rounds.contains(&(rd + 1))));
        out.push(Found {
            fix: Fix {
                kind: "rotate",
                title: format!("Rotating from {from_w} to {to_w} too late"),
                cost: format!("lost {n} of {n}"),
                phrase: format!("rotating to {to_w} after the spike was already down"),
                short: format!(
                    "On defense you got to {to_w} late {} times, after the spike was already down, and the round was lost.",
                    words(n)
                ),
                try_short: format!("Leave {from_w} when a teammate calls {to_w}, not when you hear the plant."),
                happened: format!(
                    "Rounds {}. The spike went down on {to_w} and you took a while to get there, so you had to retake.",
                    rounds_list(&rounds)
                ),
                why: format!("Retakes are much harder than holding the site. You lost all {n} of these rounds."),
                tries: vec![
                    format!("When a teammate calls {to_w}, start moving straight away."),
                    "Use your information early, so you know where they're going before they get there.".into(),
                ],
                check: format!("Next match: get to {to_w} before the plant when it's called."),
                habit: None,
                rounds,
                lost: n,
            },
            deaths: ix,
        });
    }

    // Dying with the ult ready.
    let ix = indexes(r, is_ult_held);
    if ix.len() >= 2 {
        let ds = pick(&ix);
        let (rounds, lost) = rounds_of(r, &ds);
        let ult = &kit.ult;
        out.push(Found {
            fix: Fix {
                kind: "ult",
                title: format!("Dying with {ult} ready"),
                cost: format!("lost {lost} of {}", rounds.len()),
                phrase: format!("dying with {ult} ready"),
                short: format!(
                    "{} times you died with {ult} ready and unused.",
                    capitalise(&words(ix.len()))
                ),
                try_short: format!("When {ult} is ready, plan where you'll use it at the start of the round."),
                happened: format!(
                    "Rounds {}. You had {ult} ready and died without using it.",
                    rounds_list(&rounds)
                ),
                why: "An unused ult is a whole round's worth of help thrown away.".into(),
                tries: vec![
                    format!("Decide where {ult} goes before the round starts."),
                    "Use it to start a retake or to clear a site, not to save it for later.".into(),
                ],
                check: "Next match: no deaths with your ult ready.".into(),
                habit: Some("ult"),
                rounds,
                lost,
            },
            deaths: ix,
        });
    }

    // Crosshair below the head.
    let known: Vec<f64> = d.iter().filter_map(|x| x.context.vertical_aim_offset_deg).collect();
    let low = known.iter().filter(|v| **v < -2.0).count();
    if known.len() >= 5 && low * 2 >= known.len() {
        out.push(Found {
            fix: Fix {
                kind: "crosshair",
                title: "Crosshair too low when enemies appeared".into(),
                cost: "slower fights".into(),
                phrase: "having to flick up to the enemy's head".into(),
                short: format!(
                    "In {low} of {} deaths your crosshair was below the enemy's head, so you had to flick up first.",
                    known.len()
                ),
                try_short: "Walk with your crosshair at head height, on the edge of the corner you're clearing.".into(),
                happened: format!(
                    "In {low} of the {} deaths where it's known, your crosshair was more than 2° below the head of the player who killed you.",
                    known.len()
                ),
                why: "Every flick up gives the enemy a moment for free. With your crosshair already there, you only have to click.".into(),
                tries: vec![
                    "Keep your crosshair at head height as you walk, on the edge you're about to clear.".into(),
                    "Walk a map in a custom game practising just this for 5 minutes.".into(),
                ],
                check: "Next match: think about where your crosshair sits as you move.".into(),
                habit: Some("crosshair"),
                rounds: vec![],
                lost: 0,
            },
            deaths: vec![],
        });
    }

    // Reloading in the open.
    let ix = indexes(r, is_low_ammo);
    if ix.len() >= 3 {
        let ds = pick(&ix);
        let (rounds, lost) = rounds_of(r, &ds);
        out.push(Found {
            fix: Fix {
                kind: "reload",
                title: "Fighting with an empty magazine".into(),
                cost: format!("lost {lost} of {}", rounds.len()),
                phrase: "taking fights with almost no bullets left".into(),
                short: format!(
                    "{} times you died reloading or with almost nothing left in your magazine.",
                    capitalise(&words(ix.len()))
                ),
                try_short: "Reload behind cover, or switch to your pistol to finish a fight.".into(),
                happened: format!(
                    "Rounds {}. You had a fifth of your magazine or less when you died.",
                    rounds_list(&rounds)
                ),
                why: "While you reload you can't fight back, and enemies listen for it.".into(),
                tries: vec![
                    "Reload after a fight only once you're behind cover.".into(),
                    "With a few bullets left, switch to your pistol instead of reloading in the open.".into(),
                ],
                check: "Next match: no deaths with an empty magazine.".into(),
                habit: Some("reload"),
                rounds,
                lost,
            },
            deaths: ix,
        });
    }

    out.sort_by(|a, b| {
        b.fix
            .lost
            .cmp(&a.fix.lost)
            .then(b.fix.rounds.len().cmp(&a.fix.rounds.len()))
    });
    out
}

// ------------------------------------------------------------- moments

fn find_moments(r: &MatchReview, fixes: &[Found]) -> Vec<(Moment, usize)> {
    let kit = kit(r);
    let mut out: Vec<(Moment, usize)> = Vec::new();
    let rotating: Vec<u32> = fixes
        .iter()
        .filter(|f| f.fix.kind == "rotate")
        .flat_map(|f| f.fix.rounds.clone())
        .collect();
    for (i, d) in r.death_facts.iter().enumerate() {
        let Some(f) = round_of(r, d) else { continue };
        if f.won != Some(false) || out.iter().any(|(m, _)| m.round == f.round + 1) {
            continue;
        }
        let c = &d.context;
        let place = d.nearest_callout.clone().unwrap_or_else(|| "the open".into());
        let gun = c.weapon_in_hand.clone().unwrap_or_else(|| "rifle".into());
        let killer = &d.killer_agent;
        let m = if is_lost_round_fight(d) {
            let secs = c
                .clock_left_s
                .map(|s| format!(", {s:.0} seconds left"))
                .unwrap_or_default();
            Some(Moment {
                round: f.round + 1,
                situation: format!("1 against {}{secs}, no plant", d.enemies_alive),
                better: format!("hide and keep your {gun}"),
                text: format!(
                    "You were the last one alive against {} with the spike not planted. You took a fight at {place} and {killer} killed you. Better: fall back and keep your {gun} and shields for next round.",
                    d.enemies_alive
                ),
                kind: "save",
            })
        } else if rotating.contains(&(f.round + 1)) {
            let site = f.plant_site.clone().unwrap_or_else(|| "the site".into());
            Some(Moment {
                round: f.round + 1,
                situation: format!("Late rotation to {site}"),
                better: "leave when it's called".into(),
                text: format!(
                    "The spike went down on {site} before you got there, and you died in the retake. Better: start moving the moment {site} is called."
                ),
                kind: "rotate",
            })
        } else if is_ult_held(d) && c.post_plant && f.side == Some(Side::Defense) {
            let site = f.plant_site.clone().unwrap_or_else(|| "the site".into());
            Some(Moment {
                round: f.round + 1,
                situation: format!("{} ready, retaking {site}", kit.ult),
                better: format!("open with {}", kit.ult),
                text: format!(
                    "You had {} ready and died retaking {site} without using it. Better: start the retake with your ult, so the enemy has to move or take damage.",
                    kit.ult
                ),
                kind: "ult",
            })
        } else if is_low_ammo(d) {
            Some(Moment {
                round: f.round + 1,
                situation: format!("Low on ammo with {killer} close by"),
                better: "reload behind cover".into(),
                text: format!(
                    "You had almost nothing left in your magazine at {place} when {killer} swung. Better: finish the fight with your pistol, or reload only behind cover."
                ),
                kind: "reload",
            })
        } else if is_repeek(d) {
            Some(Moment {
                round: f.round + 1,
                situation: format!("Peeked {place} again after a kill"),
                better: "move after a kill".into(),
                text: format!(
                    "You got a kill at {place}, then stayed and {killer} was waiting for you. Better: step back or change angle after a kill."
                ),
                kind: "repeek",
            })
        } else if d.speed_m_s.is_some_and(|s| s > RUNNING_M_S) && c.duel_hits_dealt == 0 && !d.self_kill {
            Some(Moment {
                round: f.round + 1,
                situation: format!("Running into {killer}"),
                better: "stop, then shoot".into(),
                text: format!(
                    "You were still running when {killer} found you at {place}, and none of your shots hit. Better: stop before you shoot, even when the fight is rushed."
                ),
                kind: "moving",
            })
        } else if is_alone(d) && d.first_death_of_round {
            let mate = d
                .nearest_teammates
                .first()
                .map(|t| t.agent.clone())
                .unwrap_or_else(|| "a teammate".into());
            Some(Moment {
                round: f.round + 1,
                situation: format!("Pushed {place} on your own"),
                better: format!("wait for {mate}"),
                text: format!(
                    "You went into {place} by yourself while {mate} was still far behind. You died and nobody could get the kill back. Better: wait a second for {mate}, so whoever dies first gets traded."
                ),
                kind: "alone",
            })
        } else {
            None
        };
        if let Some(m) = m {
            out.push((m, i));
        }
    }
    out.sort_by_key(|(m, _)| m.round);
    out.truncate(5);
    out
}

// ----------------------------------------------------------- keep doing

fn keep_doing(r: &MatchReview, lobby: &[MatchReview]) -> Vec<String> {
    let rf = &r.round_facts;
    let mut out: Vec<(f64, String)> = Vec::new();
    let wl = |it: Vec<&RoundFact>| {
        (
            it.iter().filter(|f| f.won == Some(true)).count(),
            it.iter().filter(|f| f.won == Some(false)).count(),
        )
    };
    let (w, l) = wl(rf
        .iter()
        .filter(|f| f.side == Some(Side::Attack) && f.planted)
        .collect());
    if w + l >= 3 && w > l {
        out.push((
            0.6,
            format!("You won {w} of the {} rounds where you planted the spike.", w + l),
        ));
    }
    let (w, l) = wl(rf
        .iter()
        .filter(|f| f.side == Some(Side::Attack) && f.plant_site.is_some())
        .collect());
    if w + l >= 3 && w > l {
        out.push((0.5, format!("You won {w} of your {} post-plants.", w + l)));
    }
    let (w, l) = wl(rf
        .iter()
        .filter(|f| f.side == Some(Side::Defense) && f.plant_site.is_some())
        .collect());
    if w + l >= 3 && w >= l {
        out.push((0.5, format!("You won {w} of {} retakes.", w + l)));
    }
    for f in rf.iter().filter(|f| f.won == Some(true)) {
        if let Some(n) = f.clutch_vs {
            out.push((0.8, format!("You won a 1 against {n} in round {}.", f.round + 1)));
        }
    }
    let ((mt, mn), (tt, tn)) = (r.my_trades, r.team_trades);
    if mn >= 5 && tn > 0 && mt as f64 / mn as f64 > tt as f64 / tn as f64 + 0.05 {
        out.push((
            0.5,
            "When you died, a teammate got the kill back more often than for the rest of your team.".into(),
        ));
    }
    if let Some((spot, n)) = most_common(r.duels.iter().filter(|x| x.won).filter_map(|x| x.callout.clone()))
        && n >= 3
    {
        let lost = r
            .duels
            .iter()
            .filter(|x| !x.won && x.callout.as_deref() == Some(&spot))
            .count();
        if n > lost {
            out.push((0.7, format!("You won {n} fights at {spot}. That spot works for you.")));
        }
    }
    let first_kills = rf.iter().filter(|f| f.first_kill).count();
    if first_kills >= 3 {
        let (won, _) = wl(rf.iter().filter(|f| f.first_kill).collect());
        out.push((
            0.6,
            format!(
                "You opened {} rounds with the first kill, and your team won {won} of them.",
                words(first_kills)
            ),
        ));
    }
    if let Some(rank) = damage_rank(r, lobby)
        && rank <= 3
    {
        out.push((0.9, "You did more damage than most of the lobby.".into()));
    }
    let (w, l) = wl(rf.iter().filter(|f| f.pistol).collect());
    if w > l {
        out.push((0.4, format!("You won {w} of {} pistol rounds.", w + l)));
    }
    if r.death_facts.len() >= 8 && !r.death_facts.iter().any(is_alone) {
        out.push((0.7, "No deaths where nobody could trade you.".into()));
    }
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    out.into_iter().map(|(_, s)| s).take(3).collect()
}

/// My damage per round's place in the lobby (1 = most), when the lobby is known.
pub fn damage_rank(r: &MatchReview, lobby: &[MatchReview]) -> Option<usize> {
    if lobby.len() < 6 {
        return None;
    }
    let adr =
        |x: &MatchReview| x.round_facts.iter().map(|f| f.damage_dealt).sum::<f64>() / x.round_facts.len().max(1) as f64;
    let mine = adr(r);
    Some(lobby.iter().filter(|x| x.subject != r.subject && adr(x) > mine).count() + 1)
}

// --------------------------------------------------------------- debrief

pub fn debrief(r: &MatchReview, lobby: &[MatchReview]) -> Debrief {
    let rf = &r.round_facts;
    let won = rf.iter().filter(|f| f.won == Some(true)).count();
    let lost = rf.iter().filter(|f| f.won == Some(false)).count();
    let found = find_fixes(r);
    let moments = find_moments(r, &found);
    let mut deaths: Vec<Death> = r
        .death_facts
        .iter()
        .map(|d| {
            let f = round_of(r, d);
            Death {
                round: d.round.map_or(0, |x| x + 1),
                side: f.and_then(|f| f.side),
                won: f.and_then(|f| f.won),
                killer: d.killer_agent.clone(),
                place: d.nearest_callout.clone(),
                when: when(d),
                first: d.first_death_of_round,
                alone: is_alone(d),
                traded: d.traded_within_3s,
                fix: None,
                moment: None,
                uv: d.minimap_uv,
                time_ms: d.time_ms,
            }
        })
        .collect();
    for (fi, f) in found.iter().enumerate() {
        for &i in &f.deaths {
            deaths[i].fix.get_or_insert(fi);
        }
    }
    for (mi, (_, i)) in moments.iter().enumerate() {
        deaths[*i].moment = Some(mi);
    }
    let rounds = rf
        .iter()
        .map(|f| RoundLine {
            round: f.round + 1,
            side: f.side,
            won: f.won,
            buy: buy_words(&f.economy.buy_type),
            kills: f.kills,
            death: deaths.iter().position(|d| d.round == f.round + 1),
        })
        .collect();
    let fixes: Vec<Fix> = found.into_iter().map(|f| f.fix).collect();
    let keep = keep_doing(r, lobby);
    let worst_spot =
        most_common(r.death_facts.iter().filter_map(|d| d.nearest_callout.clone())).filter(|(_, n)| *n >= 2);
    let best_spot = most_common(r.duels.iter().filter(|x| x.won).filter_map(|x| x.callout.clone()))
        .filter(|(_, n)| *n >= 2)
        .map(|(s, _)| s);
    Debrief {
        match_id: r.context.match_id.clone(),
        agent: r.agent.clone(),
        map: map_name(r),
        played_at: r.context.played_at.clone(),
        won,
        lost,
        kills: r.kills,
        deaths_n: r.deaths,
        summary: summary(won, lost, &fixes, damage_rank(r, lobby)),
        fixes,
        moments: moments.into_iter().map(|(m, _)| m).collect(),
        keep,
        deaths,
        rounds,
        wins: r.duels.iter().filter(|x| x.won).filter_map(|x| x.uv).collect(),
        best_spot,
        worst_spot,
    }
}

fn summary(won: usize, lost: usize, fixes: &[Fix], damage_rank: Option<usize>) -> String {
    let opening = if won > lost {
        match won - lost {
            1..=2 => "A close win.",
            3..=5 => "A good win.",
            _ => "A clean win.",
        }
    } else if lost > won {
        match lost - won {
            1..=2 => "Close, but lost.",
            3..=5 => "A loss.",
            _ => "A tough loss.",
        }
    } else {
        "A draw."
    };
    let mut s = opening.to_owned();
    if damage_rank.is_some_and(|r| r <= 3) {
        s.push_str(" Your aim held up: you did more damage than most of the lobby.");
    }
    match fixes.first() {
        Some(f) if f.lost >= 2 => s.push_str(&format!(
            " The rounds slipped away in one place: {}. If you fix one thing next match, fix that.",
            f.phrase
        )),
        Some(f) => s.push_str(&format!(" The thing to watch: {}.", f.phrase)),
        None => s.push_str(" Nothing kept going wrong. Keep doing what worked."),
    }
    s
}

/// Made-up reviews for tests, here and in the CLI.
#[doc(hidden)]
pub mod sample {
    use super::*;

    /// A made-up review: `rounds` (won?, side) and deaths built by `death`.
    pub fn review(rounds: &[(bool, Side)], deaths: Vec<DeathFact>) -> MatchReview {
        let mut r: MatchReview = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "subject": "me", "pawn": 1, "agent": "Sova", "agent_id": null,
            "abilities": {"X": "Hunter's Fury"},
            "context": {"match_id": "m1", "map_url": null, "map_name": "Lotus", "replay_build": "13.08",
                "build_note": null, "played_at": "2026-10-19T21:04:11Z", "duration_ms": 0, "rounds": 0, "teams": []},
            "map_path": null, "map_has_callouts": true, "catalog_synced": true, "warnings": [],
            "kills": 10, "deaths": 0, "teammates": [], "enemies": [], "match_kills": 0, "match_kills_traded": 0,
            "my_trades": [0, 0], "team_trades": [0, 0], "death_facts": [], "round_facts": [], "duels": [],
            "clusters": [],
            "summary": {"deaths": 0, "first_deaths": 0, "first_death_rounds": [], "no_teammate_alive": 0,
                "with_teammate_alive": 0, "with_teammate_alive_traded": 0, "teammate_within_10m": 0,
                "teammate_within_10m_traded": 0, "nearest_teammate_beyond_15m": 0, "facing_killer_within_45deg": 0,
                "facing_known": 0, "side_or_behind_rounds": [], "killers": [], "median_since_round_start_s": null}
        }))
        .expect("a valid review");
        r.round_facts = rounds
            .iter()
            .enumerate()
            .map(|(i, (won, side))| RoundFact {
                round: i as u32,
                won: Some(*won),
                side: Some(*side),
                ..RoundFact::default()
            })
            .collect();
        r.deaths = deaths.len();
        for d in &deaths {
            if let Some(f) = r.round_facts.iter_mut().find(|f| Some(f.round) == d.round) {
                f.died = true;
                f.first_death = d.first_death_of_round;
            }
        }
        r.death_facts = deaths;
        r
    }

    pub fn death(round: u32, place: &str) -> DeathFact {
        serde_json::from_value(serde_json::json!({
            "schema_version": 3, "match_id": "m1", "round": round, "time_ms": 1000 * round,
            "since_round_start_s": 50.0, "victim_pawn": 1, "killer_pawn": 7, "killer_agent": "Reyna",
            "self_kill": false, "first_death_of_round": false, "teammates_alive": 2, "enemies_alive": 3,
            "nearest_teammates": [{"pawn": 2, "agent": "Omen", "dist_m": 6.0}], "dist_to_killer_m": 12.0,
            "crosshair_offset_to_killer_deg": 10.0, "speed_m_s": 1.0, "traded_within_3s": false,
            "pos_cm": null, "minimap_uv": [0.5, 0.5], "nearest_callout": place, "cluster_id": null,
            "side": null, "since_barrier_s": 30.0, "before_round_end_s": null, "clock_left_s": 40.0,
            "post_plant": false, "carrying_spike": false, "outnumbered": false, "killer_weapon": "Vandal",
            "killing_blow_region": null, "killing_blow_wallbang": false, "weapon_in_hand": "Vandal",
            "knife_out": false, "magazine_ammo": 20, "magazine_size": 25, "duel_damage_dealt": 0.0,
            "duel_damage_taken": 150.0, "duel_hits_dealt": 1, "duel_hits_taken": 3, "hit_killer_first": null,
            "duel_ms": null, "vertical_aim_offset_deg": 0.0, "ult_points": null, "ult_ready": false,
            "inventory_value": 3900, "same_spot_as_own_kill": false, "casts_last_5s": 1,
            "utility_unused_value": false
        }))
        .expect("a valid death")
    }
}

#[cfg(test)]
mod tests {
    use super::sample::{death, review};
    use super::*;

    #[test]
    fn entries_and_isolated_deaths_become_fixes_ranked_by_rounds_lost() {
        let rounds: Vec<(bool, Side)> = (0..13)
            .map(|i| (i % 3 == 0, if i < 12 { Side::Attack } else { Side::Defense }))
            .collect();
        let mut deaths = Vec::new();
        for round in [1, 2, 4] {
            let mut d = death(round, "A Rubble");
            d.first_death_of_round = true;
            d.context.casts_last_5s = 0;
            d.nearest_teammates.clear();
            deaths.push(d);
        }
        for round in [5, 7] {
            let mut d = death(round, "B Main");
            d.context.same_spot_as_own_kill = true;
            deaths.push(d);
        }
        let r = review(&rounds, deaths);
        let b = debrief(&r, &[]);
        assert_eq!(b.fixes[0].kind, "first-in");
        assert_eq!(b.fixes[0].title, "First into A Rubble alone, with no ability first");
        assert_eq!(b.fixes[0].rounds, [2, 3, 5]);
        assert_eq!(b.fixes[0].cost, "lost 3 of 3");
        assert!(b.fixes.iter().any(|f| f.kind == "untraded"));
        assert!(b.fixes.iter().any(|f| f.kind == "repeek"));
        // Each death belongs to the first fix that names it.
        assert_eq!(b.deaths[0].fix, Some(0));
        assert!(b.deaths[0].alone && b.deaths[0].first);
        assert_eq!(
            b.deaths[0].sentence(),
            "Reyna killed you at A Rubble, mid-round, the first death of the round. Nobody was close enough to trade."
        );
        assert!(b.summary.contains("going in first into A Rubble"), "{}", b.summary);
        assert_eq!(b.rounds.len(), 13);
        assert_eq!(b.rounds[1].death, Some(0));
        assert!(!b.moments.is_empty());
    }

    #[test]
    fn wording() {
        assert_eq!(list_and(&["1".into(), "2".into(), "3".into()]), "1, 2 and 3");
        assert_eq!(words(3), "three");
        assert_eq!(plural(2, "match"), "2 matches");
        assert_eq!(plural(1, "match"), "1 match");
        assert_eq!(plural(3, "death"), "3 deaths");
        assert_eq!(words(14), "14");
        assert_eq!(share_words(0.34), "about a third");
    }
}
