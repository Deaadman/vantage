//! Which replay builds a vrfkit release decodes, and how far the live game
//! has moved past them. Each release's range is in its pin (`vrfkit.json`),
//! taken from vrfkit's README, "Supported VALORANT builds".

/// What a vrfkit release decodes: its tag, and the oldest and newest builds
/// as codes (13.08 is 1308).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decodes {
    pub tag: String,
    pub oldest: u32,
    pub newest: u32,
}

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
pub fn check_build(replay_build: &str, d: &Decodes) -> BuildSupport {
    let Some(code) = parse_build(replay_build) else {
        return BuildSupport::Unsupported(format!("unrecognised replay_build `{replay_build}`"));
    };
    let this = pretty(code);
    if code > d.newest {
        BuildSupport::Unsupported(format!(
            "build {this} is newer than vrfkit {} supports (up to {}); it's read once a vrfkit update supports {this}",
            d.tag,
            pretty(d.newest)
        ))
    } else if code < d.oldest {
        BuildSupport::Unsupported(format!(
            "build {this} is older than vrfkit {} supports (from {})",
            d.tag,
            pretty(d.oldest)
        ))
    } else {
        BuildSupport::Supported
    }
}

/// One line when the live game (valorant-api.com `/v1/version`, e.g.
/// `13.10.00.5712345`) is past what vrfkit decodes: new replays will fail.
pub fn check_game_version(game_version: &str, d: &Decodes) -> Option<String> {
    let code = parse_version(game_version)?;
    (code > d.newest).then(|| {
        format!(
            "the game is on {} but vrfkit {} decodes up to {}: replays from this patch are read once a vrfkit update supports it",
            pretty(code),
            d.tag,
            pretty(d.newest)
        )
    })
}

/// `13.10` or `13.10.00.5712345` -> 1310.
pub fn parse_version(v: &str) -> Option<u32> {
    let mut parts = v.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    Some(major * 100 + minor)
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

    fn d() -> Decodes {
        Decodes {
            tag: "v0.3.0".into(),
            oldest: 11_06,
            newest: 13_08,
        }
    }

    #[test]
    fn builds() {
        assert_eq!(check_build("++Ares-Core+release-13.08", &d()), BuildSupport::Supported);
        assert_eq!(check_build("++Ares-Core+release-13.05", &d()), BuildSupport::Supported);
        assert!(check_build("++Ares-Core+release-13.10", &d()).is_unsupported());
        assert!(check_build("++Ares-Core+release-11.05", &d()).is_unsupported());
        assert!(check_build("garbage", &d()).is_unsupported());
        let newer = Decodes { newest: 13_10, ..d() };
        assert_eq!(
            check_build("++Ares-Core+release-13.10", &newer),
            BuildSupport::Supported
        );
    }

    #[test]
    fn game_version() {
        assert_eq!(check_game_version("13.08.00.5698904", &d()), None);
        assert!(check_game_version("13.10.00.5712345", &d()).unwrap().contains("13.10"));
        assert_eq!(check_game_version("nonsense", &d()), None);
        assert_eq!(parse_version("13.10"), Some(13_10));
    }
}
