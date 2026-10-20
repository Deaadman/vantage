//! Which replay builds the pinned vrfkit release decodes, and how far the
//! live game has moved past them.

/// Builds listed as supported by vrfkit v0.3.0 (README, "Supported VALORANT builds").
pub const VRFKIT_VERSION: &str = "v0.3.0";
const SUPPORTED: (u32, u32) = (11_06, 13_08);
/// The same range, for messages.
pub const SUPPORTED_RANGE: &str = "11.06 to 13.08";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildSupport {
    Supported,
    Unsupported(String),
}

impl BuildSupport {
    pub fn is_unsupported(&self) -> bool {
        matches!(self, BuildSupport::Unsupported(_))
    }
}

/// `replay_build` looks like `++Ares-Core+release-13.08`.
pub fn check_build(replay_build: &str) -> BuildSupport {
    let Some(code) = parse_build(replay_build) else {
        return BuildSupport::Unsupported(format!("unrecognised replay_build `{replay_build}`"));
    };
    let this = pretty(code);
    if code > SUPPORTED.1 {
        BuildSupport::Unsupported(format!(
            "build {this} is newer than vrfkit {VRFKIT_VERSION} supports (up to {}); update vrfkit once a release supports {this}",
            pretty(SUPPORTED.1)
        ))
    } else if code < SUPPORTED.0 {
        BuildSupport::Unsupported(format!(
            "build {this} is older than vrfkit {VRFKIT_VERSION} supports (from {})",
            pretty(SUPPORTED.0)
        ))
    } else {
        BuildSupport::Supported
    }
}

/// One line when the live game (valorant-api.com `/v1/version`, e.g.
/// `13.10.00.5712345`) is past what vrfkit decodes: new replays will fail.
pub fn check_game_version(game_version: &str) -> Option<String> {
    let mut parts = game_version.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    let code = major * 100 + minor;
    (code > SUPPORTED.1).then(|| {
        format!(
            "the game is on {} but vrfkit {VRFKIT_VERSION} decodes up to {}: replays from this patch will fail until you update vrfkit and valreview",
            pretty(code),
            pretty(SUPPORTED.1)
        )
    })
}

/// `++Ares-Core+release-13.08` -> 1308.
pub fn parse_build(s: &str) -> Option<u32> {
    let ver = s.rsplit("release-").next()?;
    let (major, minor) = ver.split_once('.')?;
    let minor: String = minor.chars().take_while(|c| c.is_ascii_digit()).collect();
    Some(major.parse::<u32>().ok()? * 100 + minor.parse::<u32>().ok()?)
}

fn pretty(code: u32) -> String {
    format!("{}.{:02}", code / 100, code % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds() {
        assert_eq!(check_build("++Ares-Core+release-13.08"), BuildSupport::Supported);
        assert_eq!(check_build("++Ares-Core+release-13.05"), BuildSupport::Supported);
        assert!(check_build("++Ares-Core+release-13.10").is_unsupported());
        assert!(check_build("++Ares-Core+release-11.05").is_unsupported());
        assert!(check_build("garbage").is_unsupported());
    }

    #[test]
    fn game_version() {
        assert_eq!(check_game_version("13.08.00.5698904"), None);
        assert!(check_game_version("13.10.00.5712345").unwrap().contains("13.10"));
        assert_eq!(check_game_version("nonsense"), None);
    }
}
