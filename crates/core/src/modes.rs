//! Game modes: which one a replay is, from its game state's class.
//!
//! The replay doesn't name its queue, so Competitive, Unrated, Premier and
//! custom games all share one label; only the mode itself is recorded.

/// Asset folder of the standard mode, which every queue with ranked rules uses.
pub const STANDARD_FOLDER: &str = "GameModes/Bomb";
/// What the standard mode is called (valorant-api.com says "Standard"), so
/// players know which of their queues it covers.
pub const STANDARD_LABEL: &str = "Comp/Unrated/Premier";

/// Built-in mode table (asset folder under `/Game`, name), checked against
/// valorant-api.com `/v1/gamemodes` on 2026-10-21. Used when the synced
/// catalog doesn't know a folder.
pub(crate) const BUILTIN_MODES: &[(&str, &str)] = &[
    ("GameModes/AROS", "All Random One Site"),
    ("GameModes/AbilityDraftArena", "Gauntlet: Glitched"),
    (STANDARD_FOLDER, STANDARD_LABEL),
    ("GameModes/Deathmatch", "Deathmatch"),
    ("GameModes/Dodgeball", "Knockout"),
    ("GameModes/ExamplePlayerTestBot", "Bot Match"),
    ("GameModes/FortCollins", "Retake"),
    ("GameModes/GunGame", "Escalation"),
    ("GameModes/HURM", "Team Deathmatch"),
    ("GameModes/NPEV2", "Basic Training"),
    ("GameModes/NewPlayerExperience", "Onboarding"),
    ("GameModes/OneForAll", "Replication"),
    ("GameModes/QuickBomb", "Spike Rush"),
    ("GameModes/ShootingRange", "The Range"),
    ("GameModes/Skirmish", "Skirmish"),
    ("GameModes/SkirmishAscension", "Skirmish: Ascension"),
    ("GameModes/SnowballFight", "Snowball Fight"),
    ("GameModes/_Development/Swiftplay_EndOfRoundCredits", "Swiftplay"),
];

/// The asset folder of a game state class, e.g.
/// `/Game/GameModes/Bomb/BombGameState.BombGameState_C` -> `GameModes/Bomb`.
/// The game state sits next to its game mode, in the folder valorant-api.com
/// gives as the mode's `assetPath`. Components such as
/// `Comp_GameState_KillChampion_C` aren't game states.
pub fn game_state_folder(class_path: &str) -> Option<&str> {
    let path = class_path.strip_prefix("/Game/")?;
    if !path.starts_with("GameModes/") || !path.ends_with("GameState_C") {
        return None;
    }
    let (folder, leaf) = path.rsplit_once('/')?;
    (!leaf.starts_with("Comp_")).then_some(folder)
}

/// A name for a folder no table knows: its last part, e.g. `QuickBomb`.
pub fn fallback_name(folder: &str) -> &str {
    folder.rsplit('/').next().unwrap_or(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_state_folders() {
        assert_eq!(
            game_state_folder("/Game/GameModes/Bomb/BombGameState.BombGameState_C"),
            Some("GameModes/Bomb")
        );
        assert_eq!(
            game_state_folder(
                "/Game/GameModes/_Development/Swiftplay_EndOfRoundCredits/Swiftplay_EoRCredits_GameState.Swiftplay_EoRCredits_GameState_C"
            ),
            Some("GameModes/_Development/Swiftplay_EndOfRoundCredits")
        );
        assert_eq!(
            game_state_folder("/Game/GameModes/Components/Comp_GameState_KillChampion.Comp_GameState_KillChampion_C"),
            None
        );
        assert_eq!(
            game_state_folder("/Game/GameModes/Bomb/BombPlayerState.BombPlayerState_C"),
            None
        );
        assert_eq!(fallback_name("GameModes/QuickBomb"), "QuickBomb");
    }
}
