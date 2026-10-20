//! Which replay builds the pinned vrfkit release decodes.

/// Builds listed as supported by vrfkit v0.3.0 (README, "Supported VALORANT builds").
pub const VRFKIT_VERSION: &str = "v0.3.0";
const SUPPORTED: (u32, u32) = (11_06, 13_08);
/// Supported, but vrfkit verified it on very few replays.
const LEAST_VERIFIED: &[u32] = &[13_08];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildSupport {
    Supported,
    /// Supported with a caveat; the string says what.
    Caveat(String),
    Unsupported(String),
}

/// `replay_build` looks like `++Ares-Core+release-13.08`.
pub fn check_build(replay_build: &str) -> BuildSupport {
    let Some(code) = parse_build(replay_build) else {
        return BuildSupport::Unsupported(format!("unrecognised replay_build `{replay_build}`"));
    };
    let pretty = format!("{}.{:02}", code / 100, code % 100);
    if code < SUPPORTED.0 || code > SUPPORTED.1 {
        BuildSupport::Unsupported(format!(
            "build {pretty} is outside vrfkit {VRFKIT_VERSION}'s supported range 11.06-13.08; update vrfkit"
        ))
    } else if LEAST_VERIFIED.contains(&code) {
        BuildSupport::Caveat(format!(
            "build {pretty} is the least-verified build in vrfkit {VRFKIT_VERSION}; treat odd numbers with suspicion"
        ))
    } else {
        BuildSupport::Supported
    }
}

fn parse_build(s: &str) -> Option<u32> {
    let ver = s.rsplit("release-").next()?;
    let (major, minor) = ver.split_once('.')?;
    let minor: String = minor.chars().take_while(|c| c.is_ascii_digit()).collect();
    Some(major.parse::<u32>().ok()? * 100 + minor.parse::<u32>().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds() {
        assert!(matches!(
            check_build("++Ares-Core+release-13.08"),
            BuildSupport::Caveat(_)
        ));
        assert_eq!(check_build("++Ares-Core+release-13.05"), BuildSupport::Supported);
        assert!(matches!(
            check_build("++Ares-Core+release-13.10"),
            BuildSupport::Unsupported(_)
        ));
        assert!(matches!(check_build("garbage"), BuildSupport::Unsupported(_)));
    }
}
