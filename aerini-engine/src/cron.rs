//! 5-field cron expression parser.
//!
//! Supported syntax: `minute hour day-of-month month day-of-week`
//!
//! - Wildcards: `*`
//! - Ranges: `1-5`
//! - Step values: `*/15`, `0-30/5`
//! - Lists: `1,3,5`
//! - Named weekdays: `MON`–`SUN` (case-insensitive)
//! - Named months: `JAN`–`DEC` (case-insensitive)
//! - Macros: `@hourly`, `@daily`, `@weekly`, `@monthly`, `@yearly`
//!
//! # Entry point
//!
//! [`next_cron_delay_secs`] takes an expression string and a reference time,
//! and returns the number of seconds until the next matching minute boundary.
//! The scheduler calls this after each successful run to compute `next_run_at`.
//!
//! Returns `Err(String)` if the expression cannot be parsed. The scheduler
//! propagates this as a `SchedulerError::Other` and marks the job as errored.

use chrono::{DateTime, Datelike, Timelike, Utc};

fn substitute_names(field: &str) -> String {
    field
        .to_uppercase()
        .replace("SUN", "0")
        .replace("MON", "1")
        .replace("TUE", "2")
        .replace("WED", "3")
        .replace("THU", "4")
        .replace("FRI", "5")
        .replace("SAT", "6")
        .replace("JAN", "1")
        .replace("FEB", "2")
        .replace("MAR", "3")
        .replace("APR", "4")
        // MAY is numeric-safe: "MAY" → "5" — no conflict since it doesn't
        // appear as a substring of any other name substitution above.
        .replace("MAY", "5")
        .replace("JUN", "6")
        .replace("JUL", "7")
        .replace("AUG", "8")
        .replace("SEP", "9")
        .replace("OCT", "10")
        .replace("NOV", "11")
        .replace("DEC", "12")
}

/// Normalizes the day-of-week field only. Vixie cron and most crontab-compatible
/// tools accept both `0` and `7` for Sunday; `substitute_names` already maps
/// `SUN` -> `0`, so a bare literal `7` (standalone or inside a comma list) is
/// the one remaining common, unremarkable convention that isn't handled.
///
/// Also validates every bare numeric token in the field is in `0..=6` after
/// substitution, returning a clear error immediately instead of letting an
/// out-of-range value fall through to `next_cron_delay_secs`'s 527,040-iteration
/// exhaustive search, which would otherwise return the same generic
/// "no matching cron time found" error for what is really just a bad value.
///
/// Ranges/steps/wildcards are left untouched — mixing `7` into a range
/// endpoint (e.g. `5-7`) is not a well-defined convention in Vixie cron either,
/// so only bare numeric tokens are in scope here.
fn normalize_dow_field(field: &str) -> Result<String, String> {
    if field == "*" {
        return Ok(field.to_string());
    }
    let parts: Result<Vec<String>, String> = field
        .split(',')
        .map(|part| {
            let part = part.trim();
            let is_bare_number = !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
            if is_bare_number {
                let v: u64 = part
                    .parse()
                    .map_err(|_| format!("Invalid cron value: {}", part))?;
                let normalized = if v == 7 { 0 } else { v };
                if normalized > 6 {
                    return Err(format!(
                        "Invalid day-of-week value '{}': must be 0-7 (0 and 7 both mean Sunday)",
                        part
                    ));
                }
                Ok(normalized.to_string())
            } else {
                Ok(part.to_string())
            }
        })
        .collect();
    Ok(parts?.join(","))
}

fn expand_macro(expr: &str) -> Option<&'static str> {
    match expr.trim() {
        "@hourly"  => Some("0 * * * *"),
        "@daily"   => Some("0 0 * * *"),
        "@weekly"  => Some("0 0 * * 0"),
        "@monthly" => Some("0 0 1 * *"),
        "@yearly"  | "@annually" => Some("0 0 1 1 *"),
        _ => None,
    }
}

pub fn next_cron_delay_secs(expr: &str, now: &DateTime<Utc>) -> Result<u64, String> {
    let normalized = if let Some(expanded) = expand_macro(expr) {
        expanded.to_string()
    } else {
        expr.trim().to_string()
    };

    let raw_fields: Vec<&str> = normalized.split_whitespace().collect();
    if raw_fields.len() != 5 {
        return Err(format!("Expected 5 cron fields, got {}", raw_fields.len()));
    }

    // Substitute named weekdays/months in fields 3 (month) and 4 (weekday).
    // Also substitute in all fields so expressions like MON in a list work.
    let subs: Vec<String> = raw_fields.iter().map(|f| substitute_names(f)).collect();
    let fields: Vec<&str> = subs.iter().map(|s| s.as_str()).collect();
    let dow_field = normalize_dow_field(fields[4])?;

    let mut candidate = now
        .with_second(0).expect("cron: second normalization to 0 overflowed — impossible before year 262143")
        .checked_add_signed(chrono::Duration::minutes(1)).expect("cron: adding 1 minute overflowed — impossible before year 9999");

    for _ in 0..(366 * 24 * 60) {
        if cron_field_matches(fields[0], candidate.minute() as u64, 0)?
            && cron_field_matches(fields[1], candidate.hour() as u64, 0)?
            && cron_field_matches(fields[2], candidate.day() as u64, 1)?
            && cron_field_matches(fields[3], candidate.month() as u64, 1)?
            && cron_field_matches(&dow_field,
               candidate.weekday().num_days_from_sunday() as u64, 0)?
        {
            let delay = candidate.signed_duration_since(*now);
            return Ok(delay.num_seconds().max(0) as u64);
        }
        candidate = candidate
            .checked_add_signed(chrono::Duration::minutes(1)).expect("cron: adding 1 minute in search loop overflowed — impossible before year 9999");
    }
    Err("No matching cron time found within 366 days".to_string())
}

pub fn cron_field_matches(field: &str, value: u64, min: u64) -> Result<bool, String> {
    if field == "*" { return Ok(true); }

    // */n — every n steps from min
    if let Some(step_str) = field.strip_prefix("*/") {
        let step: u64 = step_str.parse()
            .map_err(|_| format!("Invalid step: {}", field))?;
        return Ok(step > 0 && (value - min).is_multiple_of(step));
    }

    // Comma list — recurse on each part (parts may themselves be ranges or step-ranges)
    if field.contains(',') {
        for part in field.split(',') {
            if cron_field_matches(part.trim(), value, min)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }

    // a-b/n — step range
    if field.contains('-') && field.contains('/') {
        let (range_part, step_str) = field.split_once('/')
            .ok_or_else(|| format!("Invalid step-range: {}", field))?;
        let step: u64 = step_str.parse()
            .map_err(|_| format!("Invalid step in step-range: {}", field))?;
        if step == 0 {
            return Err(format!("Step must be > 0 in: {}", field));
        }
        let (lo_str, hi_str) = range_part.split_once('-')
            .ok_or_else(|| format!("Invalid range in step-range: {}", field))?;
        let lo: u64 = lo_str.parse()
            .map_err(|_| format!("Invalid range start in: {}", field))?;
        let hi: u64 = hi_str.parse()
            .map_err(|_| format!("Invalid range end in: {}", field))?;
        if lo > hi {
            return Err(format!(
                "Invalid step-range '{}': start ({}) must not exceed end ({})",
                field, lo, hi
            ));
        }
        return Ok(value >= lo && value <= hi && (value - lo).is_multiple_of(step));
    }

    // a-b — plain range
    if field.contains('-') {
        let (lo_str, hi_str) = field.split_once('-')
            .ok_or_else(|| format!("Invalid range: {}", field))?;
        let lo: u64 = lo_str.parse()
            .map_err(|_| format!("Invalid range start: {}", field))?;
        let hi: u64 = hi_str.parse()
            .map_err(|_| format!("Invalid range end: {}", field))?;
        if lo > hi {
            return Err(format!(
                "Invalid cron range '{}': start ({}) must not exceed end ({})",
                field, lo, hi
            ));
        }
        return Ok(value >= lo && value <= hi);
    }

    // Bare number
    let v: u64 = field.parse()
        .map_err(|_| format!("Invalid cron value: {}", field))?;
    Ok(v == value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wildcard() {
        assert!(cron_field_matches("*", 0, 0).unwrap());
        assert!(cron_field_matches("*", 59, 0).unwrap());
    }

    #[test]
    fn test_bare_number() {
        assert!(cron_field_matches("5", 5, 0).unwrap());
        assert!(!cron_field_matches("5", 6, 0).unwrap());
    }

    #[test]
    fn test_range() {
        assert!(cron_field_matches("1-5", 3, 0).unwrap());
        assert!(cron_field_matches("1-5", 1, 0).unwrap());
        assert!(cron_field_matches("1-5", 5, 0).unwrap());
        assert!(!cron_field_matches("1-5", 0, 0).unwrap());
        assert!(!cron_field_matches("1-5", 6, 0).unwrap());
    }

    #[test]
    fn test_step() {
        assert!(cron_field_matches("*/2", 0, 0).unwrap());
        assert!(cron_field_matches("*/2", 2, 0).unwrap());
        assert!(!cron_field_matches("*/2", 1, 0).unwrap());
    }

    #[test]
    fn test_step_range() {
        // 1-5/2: matches 1, 3, 5
        assert!(cron_field_matches("1-5/2", 1, 0).unwrap());
        assert!(cron_field_matches("1-5/2", 3, 0).unwrap());
        assert!(cron_field_matches("1-5/2", 5, 0).unwrap());
        assert!(!cron_field_matches("1-5/2", 2, 0).unwrap());
        assert!(!cron_field_matches("1-5/2", 0, 0).unwrap());
        assert!(!cron_field_matches("1-5/2", 6, 0).unwrap());
    }

    #[test]
    fn test_list() {
        assert!(cron_field_matches("1,3,5", 3, 0).unwrap());
        assert!(!cron_field_matches("1,3,5", 2, 0).unwrap());
    }

    #[test]
    fn test_named_weekdays() {
        // substitute_names converts MON→1
        let subst = substitute_names("MON");
        assert_eq!(subst, "1");
        let subst = substitute_names("mon");
        assert_eq!(subst, "1");
        let subst = substitute_names("SAT");
        assert_eq!(subst, "6");
    }

    #[test]
    fn test_named_months() {
        let subst = substitute_names("JAN");
        assert_eq!(subst, "1");
        let subst = substitute_names("DEC");
        assert_eq!(subst, "12");
    }

    #[test]
    fn test_macro_expansion() {
        assert_eq!(expand_macro("@daily"),   Some("0 0 * * *"));
        assert_eq!(expand_macro("@hourly"),  Some("0 * * * *"));
        assert_eq!(expand_macro("@weekly"),  Some("0 0 * * 0"));
        assert_eq!(expand_macro("@monthly"), Some("0 0 1 * *"));
        assert_eq!(expand_macro("@yearly"),  Some("0 0 1 1 *"));
        assert_eq!(expand_macro("@annually"),Some("0 0 1 1 *"));
        assert_eq!(expand_macro("0 * * * *"), None);
    }

    #[test]
    fn test_invalid_range_reversed() {
        assert!(cron_field_matches("5-1", 3, 0).is_err());
    }

    #[test]
    fn test_step_range_zero_step() {
        assert!(cron_field_matches("1-5/0", 3, 0).is_err());
    }

    // ── day-of-week '7' == Sunday ────────────────────────────────────────

    #[test]
    fn test_normalize_dow_field_bare_seven_becomes_zero() {
        assert_eq!(normalize_dow_field("7").unwrap(), "0");
    }

    #[test]
    fn test_normalize_dow_field_seven_in_list() {
        // "1,7" (Monday + Sunday-as-7) must normalize to "1,0", not error and
        // not silently fail to match Sunday.
        assert_eq!(normalize_dow_field("1,7").unwrap(), "1,0");
    }

    #[test]
    fn test_normalize_dow_field_leaves_wildcard_and_ranges_alone() {
        assert_eq!(normalize_dow_field("*").unwrap(), "*");
        assert_eq!(normalize_dow_field("1-5").unwrap(), "1-5");
        assert_eq!(normalize_dow_field("*/2").unwrap(), "*/2");
    }

    #[test]
    fn test_normalize_dow_field_out_of_range_bare_value_errors_upfront() {
        // 8 is not a valid day-of-week under any convention (0-7). Must return
        // a clear error immediately, not silently fall through.
        let err = normalize_dow_field("8").unwrap_err();
        assert!(err.contains("8"), "error must mention the bad value: {}", err);
        assert!(
            err.contains("0-7"),
            "error must state the valid range: {}",
            err
        );
    }

    #[test]
    fn test_next_cron_delay_secs_bare_seven_matches_sunday() {
        // Reference time is Saturday 2026-07-18T23:30:00Z; the next midnight
        // boundary, 2026-07-19T00:00:00Z, is a Sunday — the target both
        // expressions below must match, 30 minutes later.
        let ref_time = chrono::DateTime::parse_from_rfc3339("2026-07-18T23:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let via_seven = next_cron_delay_secs("0 0 * * 7", &ref_time).unwrap();
        let via_zero = next_cron_delay_secs("0 0 * * 0", &ref_time).unwrap();
        assert_eq!(
            via_seven, via_zero,
            "day-of-week '7' must resolve identically to '0' (both mean Sunday)"
        );
    }

    #[test]
    fn test_next_cron_delay_secs_invalid_dow_value_returns_clear_error() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-07-18T23:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let err = next_cron_delay_secs("0 0 * * 8", &now).unwrap_err();
        assert!(
            err.contains("0-7"),
            "must fail fast with a clear range error, not the generic \
             'no matching cron time found within 366 days' message: {}",
            err
        );
    }
}
