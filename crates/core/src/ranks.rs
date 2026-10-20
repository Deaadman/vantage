//! Ranks, set by the player in `/settings`: replays carry no rank or RR (the
//! header has only loadouts, and RR is worked out by the server after the
//! match). The colours are the game's own tier colours (`color` in
//! valorant-api.com/v1/competitivetiers).

pub const TIERS: [&str; 9] = [
    "Iron",
    "Bronze",
    "Silver",
    "Gold",
    "Platinum",
    "Diamond",
    "Ascendant",
    "Immortal",
    "Radiant",
];

const COLOURS: [(u8, u8, u8); 9] = [
    (0x86, 0x89, 0x86),
    (0xa5, 0x85, 0x5d),
    (0xbb, 0xc2, 0xc2),
    (0xec, 0xcf, 0x56),
    (0x59, 0xa9, 0xb6),
    (0xb4, 0x89, 0xc4),
    (0x6a, 0xe2, 0xaf),
    (0xbb, 0x3d, 0x65),
    (0xff, 0xff, 0xaa),
];

/// Every rank, lowest first: `Iron 1` … `Immortal 3`, `Radiant`.
pub fn all() -> Vec<String> {
    let mut out: Vec<String> = TIERS[..8]
        .iter()
        .flat_map(|t| (1..=3).map(move |d| format!("{t} {d}")))
        .collect();
    out.push("Radiant".into());
    out
}

/// The tier of a rank, `Gold` for `Gold 2`.
pub fn tier(rank: &str) -> &str {
    rank.split_whitespace().next().unwrap_or(rank)
}

/// The tier's in-game colour, or `None` for something that isn't a rank.
pub fn colour(rank: &str) -> Option<(u8, u8, u8)> {
    let t = tier(rank);
    TIERS.iter().position(|x| x.eq_ignore_ascii_case(t)).map(|i| COLOURS[i])
}

/// A rank written any way (`gold 2`, `GOLD2`) as the game writes it, if it is one.
pub fn parse(s: &str) -> Option<String> {
    let squashed: String = s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
    all().into_iter().find(|r| r.replace(' ', "").to_lowercase() == squashed)
}

/// The rank above, or the same rank at the top.
pub fn next(rank: &str) -> String {
    let all = all();
    match all.iter().position(|r| r == rank) {
        Some(i) if i + 1 < all.len() => all[i + 1].clone(),
        _ => rank.to_owned(),
    }
}

/// Ranks grouped by what wins games there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    Low,
    Mid,
    High,
    Top,
}

pub fn band(rank: &str) -> Band {
    match tier(rank) {
        "Iron" | "Bronze" | "Silver" => Band::Low,
        "Gold" | "Platinum" => Band::Mid,
        "Diamond" | "Ascendant" => Band::High,
        _ => Band::Top,
    }
}

pub struct Advice {
    pub label: &'static str,
    /// What wins games, most important first: (headline, why).
    pub wins: [(&'static str, &'static str); 4],
    pub verdict: &'static str,
}

pub fn advice(band: Band) -> Advice {
    match band {
        Band::Low => Advice {
            label: "Iron to Silver",
            wins: [
                (
                    "Keep your crosshair at head height",
                    "Most fights here are won by whoever's crosshair is already where the enemy's head appears. Look at the edge of the corner you're about to clear, at head height.",
                ),
                (
                    "Stop moving before you shoot",
                    "Running while you shoot is the most common way fights are lost at this rank. Let go of your movement keys, then click.",
                ),
                (
                    "Buy with your team",
                    "One full buy next to four players saving loses rounds. Ask what the team is doing and match it.",
                ),
                (
                    "Stay close to a teammate",
                    "If you're near a teammate when you die, they can kill your killer. Alone, you just lose a player.",
                ),
            ],
            verdict: "At this rank, clean aim habits win more games than anything clever. Fix how you take fights first; strategy comes later.",
        },
        Band::Mid => Advice {
            label: "Gold and Platinum",
            wins: [
                (
                    "Trade your teammates",
                    "Players here can aim. Rounds are decided by who gets the kill back, so stay a few steps from whoever goes in first.",
                ),
                (
                    "Use an ability before you peek",
                    "Gold and Platinum players punish peeks without utility. One ability first lets you take the fight on your terms.",
                ),
                (
                    "Move after a kill",
                    "Players at this rank expect you to swing again from the same spot. Reposition before you peek again.",
                ),
                (
                    "Save when the round is lost",
                    "Keeping your rifle for the next round wins more games than a long-shot 1v3.",
                ),
            ],
            verdict: "Your aim is good enough for the rank above. What holds players back in Gold and Platinum is when they fight, not how they shoot.",
        },
        Band::High => Advice {
            label: "Diamond and Ascendant",
            wins: [
                (
                    "Pick when you fight",
                    "Enemies hold good angles. Win the round by fighting when your team is ready, not the moment you see someone.",
                ),
                (
                    "Combine utility with your team",
                    "A flash or recon timed with a teammate's push is worth more than both used alone.",
                ),
                (
                    "Change your patterns",
                    "Players here notice habits within a half. If you take the same fight twice, they'll be waiting the third time.",
                ),
                (
                    "Win the post-plant",
                    "Play the spike from a spot that sees it, not from on top of it, and make them come to you.",
                ),
            ],
            verdict: "At this rank, everyone's mechanics are solid. Games are won by timing and by not being predictable.",
        },
        Band::Top => Advice {
            label: "Immortal and Radiant",
            wins: [
                (
                    "Play with your team's plan",
                    "Solo plays get read and punished. Your value is in making your team's plan work.",
                ),
                (
                    "Mix up your positions",
                    "Players will learn your spot after one round. Never hold the same angle three rounds in a row.",
                ),
                (
                    "Know every timing",
                    "Rotations and pushes here are decided by seconds. Know how long each route takes.",
                ),
                (
                    "Save more than you think",
                    "Economy decides close games. A saved rifle is often a round won later.",
                ),
            ],
            verdict: "At the top, the small habits are what separate you. Every repeat in your play is something the enemy can use.",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks() {
        let all = all();
        assert_eq!(all.len(), 25);
        assert_eq!(all[0], "Iron 1");
        assert_eq!(all[24], "Radiant");
        assert_eq!(parse("gold2").as_deref(), Some("Gold 2"));
        assert_eq!(parse(" Immortal 3 ").as_deref(), Some("Immortal 3"));
        assert_eq!(parse("Gold 4"), None);
        assert_eq!(next("Gold 3"), "Platinum 1");
        assert_eq!(next("Radiant"), "Radiant");
        assert_eq!(colour("Gold 2"), Some((0xec, 0xcf, 0x56)));
        assert_eq!(colour("Unranked"), None);
        assert_eq!(band("Silver 3"), Band::Low);
        assert_eq!(band("Ascendant 1"), Band::High);
        assert_eq!(band("Radiant"), Band::Top);
    }
}
