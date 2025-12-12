// Minimal RFC3339 (GitHub DateTime) parser to unix seconds.
// Supports: "YYYY-MM-DDTHH:MM:SSZ" and fractional seconds like ".sssZ".

pub fn parse_github_datetime_to_unix(s: &str) -> Option<i64> {
    // Trim fractional seconds if present.
    let s = s.trim();
    let (main, tz) = s.rsplit_once('Z')?;
    if !tz.is_empty() {
        return None;
    }
    let main = main.split_once('.').map(|(a, _)| a).unwrap_or(main);

    // YYYY-MM-DDTHH:MM:SS
    if main.len() != 19 {
        return None;
    }
    let year: i32 = main.get(0..4)?.parse().ok()?;
    if main.get(4..5)? != "-" {
        return None;
    }
    let month: u32 = main.get(5..7)?.parse().ok()?;
    if main.get(7..8)? != "-" {
        return None;
    }
    let day: u32 = main.get(8..10)?.parse().ok()?;
    if main.get(10..11)? != "T" {
        return None;
    }
    let hour: u32 = main.get(11..13)?.parse().ok()?;
    if main.get(13..14)? != ":" {
        return None;
    }
    let minute: u32 = main.get(14..16)?.parse().ok()?;
    if main.get(16..17)? != ":" {
        return None;
    }
    let second: u32 = main.get(17..19)?.parse().ok()?;

    let days = days_from_civil(year, month as i32, day as i32)?;
    let secs = (days as i64) * 86_400 + (hour as i64) * 3600 + (minute as i64) * 60 + second as i64;
    Some(secs)
}

pub fn unix_to_ymd(ts: i64) -> Option<(i32, u32, u32)> {
    // Convert unix seconds to UTC date (YYYY-MM-DD).
    if ts < 0 {
        return None;
    }
    let days = ts / 86_400;
    civil_from_days(days)
}

// Inverse of days_from_civil: (days since 1970-01-01) -> (year, month, day).
// Algorithm adapted from Howard Hinnant's civil calendar routines.
fn civil_from_days(days: i64) -> Option<(i32, u32, u32)> {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = (yoe as i32) + (era as i32) * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = mp + if mp < 10 { 3 } else { -9 }; // [1, 12]
    let year = y + (m <= 2) as i32;
    Some((year, m as u32, d as u32))
}

// Days since 1970-01-01. Algorithm adapted from Howard Hinnant's civil calendar routines.
fn days_from_civil(year: i32, month: i32, day: i32) -> Option<i64> {
    if month < 1 || month > 12 {
        return None;
    }
    if day < 1 || day > 31 {
        return None;
    }

    let mut y = year;
    let m = month;
    y -= (m <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = (era as i64) * 146_097 + (doe as i64) - 719_468; // 719468 = days to 1970-01-01
    Some(days)
}



