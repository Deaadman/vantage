//! Small UTC time helpers; avoids a date crate for three conversions.

/// Seconds between 0001-01-01 and 1970-01-01.
const UE_EPOCH_TO_UNIX_S: i64 = 62_135_596_800;

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn now_rfc3339() -> String {
    rfc3339_utc(now_unix())
}

/// UE `FDateTime` ticks (100 ns since 0001-01-01), as in `manifest.timestamp_ticks`, to unix seconds.
pub fn ue_ticks_to_unix(ticks: i64) -> i64 {
    ticks / 10_000_000 - UE_EPOCH_TO_UNIX_S
}

/// Unix seconds to `YYYY-MM-DDTHH:MM:SSZ` (days-to-civil, Howard Hinnant).
pub fn rfc3339_utc(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_792_454_400), "2026-10-20T00:00:00Z");
        assert_eq!(rfc3339_utc(951_825_599), "2000-02-29T11:59:59Z");
        // The oracle replay's manifest.timestamp_ticks.
        assert_eq!(
            rfc3339_utc(ue_ticks_to_unix(639_278_797_629_240_000)),
            "2026-10-18T00:22:42Z"
        );
    }
}
