//! 5-field cron expression parser.
//!
//! Supported syntax: `minute hour day-of-month month day-of-week`
//!
//! - Wildcards: `*`
//! - Ranges: `1-5`
//! - Step values: `*/15`, `0-30/5`
//! - Lists: `1,3,5`
//! - Named weekdays: `MON`–`SUN` (day-of-week field only, case-insensitive)
//! - Named months: `JAN`–`DEC` (month field only, case-insensitive)
//! - Day-of-week `7` is Sunday, same as `0`
//! - Macros: `@hourly`, `@daily`, `@weekly`, `@monthly`, `@yearly`, `@annually`
//!
//! Every value is range-checked when the expression is parsed, so a bad value
//! (for example minute `75`) fails immediately with a message naming the field.
//! Day matching follows Vixie cron. The month must always match. When both
//! day-of-month and day-of-week are restricted (neither field starts with `*`),
//! a day matches if *either* does, so `0 9 1 * MON` runs on the 1st and on every
//! Monday. If either field starts with `*` (including `*/2`), both must match.
//!
//! # Entry point
//!
//! [`next_cron_delay_secs`] takes an expression string and a reference time,
//! and returns the number of whole seconds until the next matching minute
//! boundary. The scheduler calls this after each successful run to compute
//! `next_run_at`.
//!
//! The delay is measured from `now` truncated to the second, so waking after
//! exactly that many seconds is never earlier than the target minute.
//!
//! Returns `Err(String)` if the expression cannot be parsed or never matches.
//! The scheduler propagates this as a `SchedulerError::Other` and marks the
//! job as errored.

use chrono::{DateTime, Datelike, Duration, Timelike, Utc};

/// Covers a Feb 29 schedule across the 2096 → 2104 leap-year gap.
const MAX_SEARCH_DAYS: i64 = 366 * 8;

const WEEKDAY_NAMES: [(&str, &str); 7] = [
    ("SUN", "0"), ("MON", "1"), ("TUE", "2"), ("WED", "3"),
    ("THU", "4"), ("FRI", "5"), ("SAT", "6"),
];

const MONTH_NAMES: [(&str, &str); 12] = [
    ("JAN", "1"), ("FEB", "2"), ("MAR", "3"), ("APR", "4"),
    ("MAY", "5"), ("JUN", "6"), ("JUL", "7"), ("AUG", "8"),
    ("SEP", "9"), ("OCT", "10"), ("NOV", "11"), ("DEC", "12"),
];

fn substitute_names(field: &str, names: &[(&str, &str)]) -> String {
    let mut out = field.to_uppercase();
    for (name, num) in names {
        out = out.replace(name, num);
    }
    out
}

fn expand_macro(expr: &str) -> Option<&'static str> {
    match expr.trim().to_ascii_lowercase().as_str() {
        "@hourly"  => Some("0 * * * *"),
        "@daily"   => Some("0 0 * * *"),
        "@weekly"  => Some("0 0 * * 0"),
        "@monthly" => Some("0 0 1 * *"),
        "@yearly"  | "@annually" => Some("0 0 1 1 *"),
        _ => None,
    }
}

fn parse_number(s: &str, label: &str, part: &str) -> Result<u32, String> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("Invalid {label} value: '{part}'"));
    }
    s.parse().map_err(|_| format!("Invalid {label} value: '{part}'"))
}

/// Parses one cron field into a bitmask with bit `v` set for every matching
/// value `v` in `min..=max`.
fn parse_field(field: &str, label: &str, min: u32, max: u32) -> Result<u64, String> {
    let mut set = 0u64;
    for part in field.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step_str)) => {
                let step = parse_number(step_str, label, part)?;
                if step == 0 {
                    return Err(format!("Step must be > 0 in {label} field: '{part}'"));
                }
                (range, Some(step))
            }
            None => (part, None),
        };

        let (lo, hi) = if range == "*" {
            (min, max)
        } else if let Some((lo_str, hi_str)) = range.split_once('-') {
            let lo = parse_number(lo_str, label, part)?;
            let hi = parse_number(hi_str, label, part)?;
            if lo > hi {
                return Err(format!(
                    "Invalid {label} range '{part}': start ({lo}) must not exceed end ({hi})"
                ));
            }
            (lo, hi)
        } else if step.is_some() {
            return Err(format!(
                "Invalid {label} value: '{part}' (a step needs '*' or a range before the '/')"
            ));
        } else {
            let v = parse_number(range, label, part)?;
            (v, v)
        };

        if lo < min || hi > max {
            return Err(format!(
                "{label} value out of range {min}-{max}: '{part}'"
            ));
        }

        let mut v = lo;
        while v <= hi {
            set |= 1u64 << v;
            v += step.unwrap_or(1);
        }
    }
    Ok(set)
}

/// Lowest set bit at or above `from`, or `None`. `from` must be below 64.
fn first_set_at_or_after(mask: u64, from: u32) -> Option<u32> {
    let shifted = mask >> from;
    (shifted != 0).then(|| from + shifted.trailing_zeros())
}

struct Schedule {
    minutes: u64,
    hours: u64,
    days_of_month: u64,
    months: u64,
    days_of_week: u64,
    dom_unrestricted: bool,
    dow_unrestricted: bool,
}

impl Schedule {
    fn parse(expr: &str) -> Result<Self, String> {
        let normalized = expand_macro(expr).unwrap_or_else(|| expr.trim());
        let fields: Vec<&str> = normalized.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!("Expected 5 cron fields, got {}", fields.len()));
        }

        let months = substitute_names(fields[3], &MONTH_NAMES);
        let weekdays = substitute_names(fields[4], &WEEKDAY_NAMES);

        let mut days_of_week = parse_field(&weekdays, "day-of-week", 0, 7)?;
        if days_of_week & (1 << 7) != 0 {
            days_of_week = (days_of_week & !(1 << 7)) | 1;
        }

        Ok(Self {
            minutes: parse_field(fields[0], "minute", 0, 59)?,
            hours: parse_field(fields[1], "hour", 0, 23)?,
            days_of_month: parse_field(fields[2], "day-of-month", 1, 31)?,
            months: parse_field(&months, "month", 1, 12)?,
            days_of_week,
            dom_unrestricted: fields[2].starts_with('*'),
            dow_unrestricted: fields[4].starts_with('*'),
        })
    }

    fn day_matches(&self, date: chrono::NaiveDate) -> bool {
        if self.months & (1 << date.month()) == 0 {
            return false;
        }
        let dom = self.days_of_month & (1 << date.day()) != 0;
        let dow = self.days_of_week & (1 << date.weekday().num_days_from_sunday()) != 0;
        if self.dom_unrestricted || self.dow_unrestricted {
            dom && dow
        } else {
            dom || dow
        }
    }

    /// Earliest matching `(hour, minute)` at or after `(from_h, from_m)`.
    fn first_time_in_day(&self, from_h: u32, from_m: u32) -> Option<(u32, u32)> {
        let mut h = from_h;
        while let Some(hour) = first_set_at_or_after(self.hours, h) {
            let m_from = if hour == from_h { from_m } else { 0 };
            if let Some(minute) = first_set_at_or_after(self.minutes, m_from) {
                return Some((hour, minute));
            }
            h = hour + 1;
        }
        None
    }
}

pub fn next_cron_delay_secs(expr: &str, now: &DateTime<Utc>) -> Result<u64, String> {
    let schedule = Schedule::parse(expr)?;

    let now_ts = now.timestamp();
    let first_ts = (now_ts.div_euclid(60) + 1) * 60;
    let first = DateTime::from_timestamp(first_ts, 0)
        .ok_or_else(|| "Reference time is out of range".to_string())?;
    let first_date = first.date_naive();

    for offset in 0..MAX_SEARCH_DAYS {
        let Some(date) = first_date.checked_add_signed(Duration::days(offset)) else {
            break;
        };
        if !schedule.day_matches(date) {
            continue;
        }
        let (from_h, from_m) = if offset == 0 {
            (first.hour(), first.minute())
        } else {
            (0, 0)
        };
        if let Some((h, m)) = schedule.first_time_in_day(from_h, from_m) {
            if let Some(target) = date.and_hms_opt(h, m, 0) {
                let delay = target.and_utc().timestamp() - now_ts;
                return Ok(delay.max(0) as u64);
            }
        }
    }
    Err("No matching cron time found within 8 years (check that the day and month can occur together, e.g. 30 February never does)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn bits(values: &[u32]) -> u64 {
        values.iter().fold(0, |acc, v| acc | (1u64 << v))
    }

    #[test]
    fn wildcard_covers_whole_range() {
        assert_eq!(parse_field("*", "minute", 0, 59).unwrap(), (1u64 << 60) - 1);
        assert_eq!(parse_field("*", "month", 1, 12).unwrap(), bits(&(1..=12).collect::<Vec<_>>()));
    }

    #[test]
    fn number_range_list_and_steps() {
        assert_eq!(parse_field("5", "minute", 0, 59).unwrap(), bits(&[5]));
        assert_eq!(parse_field("1-5", "minute", 0, 59).unwrap(), bits(&[1, 2, 3, 4, 5]));
        assert_eq!(parse_field("1,3,5", "minute", 0, 59).unwrap(), bits(&[1, 3, 5]));
        assert_eq!(parse_field("*/20", "minute", 0, 59).unwrap(), bits(&[0, 20, 40]));
        assert_eq!(parse_field("*/5", "day-of-month", 1, 31).unwrap(), bits(&[1, 6, 11, 16, 21, 26, 31]));
        assert_eq!(parse_field("1-5/2", "minute", 0, 59).unwrap(), bits(&[1, 3, 5]));
        assert_eq!(parse_field("0-10/5,30", "minute", 0, 59).unwrap(), bits(&[0, 5, 10, 30]));
    }

    #[test]
    fn invalid_fields_name_the_field() {
        let reversed = parse_field("5-1", "minute", 0, 59).unwrap_err();
        assert!(reversed.contains("minute") && reversed.contains("5-1"), "{reversed}");
        assert!(parse_field("1-5/0", "minute", 0, 59).is_err());
        assert!(parse_field("5/15", "minute", 0, 59).is_err());
        assert!(parse_field("+5", "minute", 0, 59).is_err());
        assert!(parse_field("", "minute", 0, 59).is_err());
        let range = parse_field("75", "minute", 0, 59).unwrap_err();
        assert!(range.contains("minute") && range.contains("0-59"), "{range}");
        assert!(parse_field("0", "day-of-month", 1, 31).is_err());
        assert!(parse_field("24", "hour", 0, 23).is_err());
        assert!(parse_field("1-13", "month", 1, 12).is_err());
    }

    #[test]
    fn names_apply_only_to_their_own_field() {
        assert_eq!(substitute_names("mon-fri", &WEEKDAY_NAMES), "1-5");
        assert_eq!(substitute_names("SAT,sun", &WEEKDAY_NAMES), "6,0");
        assert_eq!(substitute_names("jan", &MONTH_NAMES), "1");
        assert_eq!(substitute_names("DEC", &MONTH_NAMES), "12");
        assert!(Schedule::parse("MON * * * *").is_err());
        assert!(Schedule::parse("* * * MON *").is_err());
        assert!(Schedule::parse("* * * * JAN").is_err());
        assert!(Schedule::parse("0 9 * JAN-MAR MON-FRI").is_ok());
    }

    #[test]
    fn macros_expand_in_any_letter_case() {
        assert_eq!(expand_macro("@daily"), Some("0 0 * * *"));
        assert_eq!(expand_macro("@HOURLY"), Some("0 * * * *"));
        assert_eq!(expand_macro("@weekly"), Some("0 0 * * 0"));
        assert_eq!(expand_macro("@monthly"), Some("0 0 1 * *"));
        assert_eq!(expand_macro("@yearly"), Some("0 0 1 1 *"));
        assert_eq!(expand_macro("@annually"), Some("0 0 1 1 *"));
        assert_eq!(expand_macro("0 * * * *"), None);
    }

    #[test]
    fn day_of_week_seven_is_sunday() {
        assert_eq!(Schedule::parse("0 0 * * 7").unwrap().days_of_week, bits(&[0]));
        assert_eq!(Schedule::parse("0 0 * * 1,7").unwrap().days_of_week, bits(&[0, 1]));
        assert_eq!(Schedule::parse("0 0 * * 5-7").unwrap().days_of_week, bits(&[0, 5, 6]));
        assert_eq!(Schedule::parse("0 0 * * *").unwrap().days_of_week, bits(&[0, 1, 2, 3, 4, 5, 6]));
    }

    #[test]
    fn day_of_week_eight_is_rejected_up_front() {
        let err = next_cron_delay_secs("0 0 * * 8", &at("2026-07-18T23:30:00Z")).unwrap_err();
        assert!(err.contains("0-7") && err.contains('8'), "{err}");
    }

    #[test]
    fn bare_seven_matches_sunday() {
        let now = at("2026-07-18T23:30:00Z");
        assert_eq!(
            next_cron_delay_secs("0 0 * * 7", &now).unwrap(),
            next_cron_delay_secs("0 0 * * 0", &now).unwrap()
        );
        assert_eq!(next_cron_delay_secs("0 0 * * 7", &now).unwrap(), 30 * 60);
    }

    #[test]
    fn wrong_field_count_is_rejected() {
        assert!(next_cron_delay_secs("* * * *", &Utc::now()).is_err());
        assert!(next_cron_delay_secs("* * * * * *", &Utc::now()).is_err());
    }

    #[test]
    fn every_minute_waits_to_next_boundary() {
        assert_eq!(next_cron_delay_secs("* * * * *", &at("2026-01-01T10:00:00Z")).unwrap(), 60);
        assert_eq!(next_cron_delay_secs("* * * * *", &at("2026-01-01T10:00:30Z")).unwrap(), 30);
    }

    #[test]
    fn delay_is_whole_seconds_and_never_wakes_before_target() {
        // 10:00:59.900 → target 10:01:00. Waking after 1 s lands at 10:01:00.900.
        assert_eq!(next_cron_delay_secs("* * * * *", &at("2026-01-01T10:00:59.900Z")).unwrap(), 1);
        // 10:00:00.300 → target 10:01:00. Waking after 60 s lands at 10:01:00.300.
        assert_eq!(next_cron_delay_secs("* * * * *", &at("2026-01-01T10:00:00.300Z")).unwrap(), 60);
    }

    #[test]
    fn rolls_over_hour_day_month_and_year() {
        assert_eq!(next_cron_delay_secs("5 * * * *", &at("2026-01-01T10:30:00Z")).unwrap(), 35 * 60);
        assert_eq!(next_cron_delay_secs("0 9 * * *", &at("2026-01-01T10:00:00Z")).unwrap(), 23 * 3600);
        assert_eq!(next_cron_delay_secs("0 0 1 * *", &at("2026-01-31T12:00:00Z")).unwrap(), 12 * 3600);
        assert_eq!(next_cron_delay_secs("0 0 1 1 *", &at("2026-12-31T23:59:30Z")).unwrap(), 30);
    }

    fn delay_to(now: &str, target: &str) -> u64 {
        (at(target) - at(now)).num_seconds() as u64
    }

    #[test]
    fn both_day_fields_restricted_match_either() {
        // Thu 2026-07-02 → next Monday is 2026-07-06.
        assert_eq!(
            next_cron_delay_secs("0 9 1 * MON", &at("2026-07-02T12:00:00Z")).unwrap(),
            delay_to("2026-07-02T12:00:00Z", "2026-07-06T09:00:00Z")
        );
        // Fri 2026-07-31 → the 1st (a Saturday) comes before any Monday.
        assert_eq!(
            next_cron_delay_secs("0 9 1 * MON", &at("2026-07-31T10:00:00Z")).unwrap(),
            delay_to("2026-07-31T10:00:00Z", "2026-08-01T09:00:00Z")
        );
    }

    #[test]
    fn star_in_either_day_field_means_only_the_other_applies() {
        let now = at("2026-07-02T12:00:00Z");
        assert_eq!(
            next_cron_delay_secs("0 9 * * MON", &now).unwrap(),
            delay_to("2026-07-02T12:00:00Z", "2026-07-06T09:00:00Z")
        );
        assert_eq!(
            next_cron_delay_secs("0 9 15 * *", &now).unwrap(),
            delay_to("2026-07-02T12:00:00Z", "2026-07-15T09:00:00Z")
        );
    }

    #[test]
    fn star_step_counts_as_unrestricted_in_day_of_month() {
        // Odd days AND Monday: 2026-07-06 is even, 2026-07-13 is the first odd Monday.
        assert_eq!(
            next_cron_delay_secs("0 0 */2 * MON", &at("2026-07-02T12:00:00Z")).unwrap(),
            delay_to("2026-07-02T12:00:00Z", "2026-07-13T00:00:00Z")
        );
    }

    #[test]
    fn star_step_counts_as_unrestricted_in_day_of_week() {
        // */2 = Sun, Tue, Thu, Sat. The 13th must also fall on one of them:
        // Mon 2026-07-13 does not, Thu 2026-08-13 does.
        assert_eq!(
            next_cron_delay_secs("0 0 13 * */2", &at("2026-07-02T12:00:00Z")).unwrap(),
            delay_to("2026-07-02T12:00:00Z", "2026-08-13T00:00:00Z")
        );
    }

    #[test]
    fn thirteenth_or_friday_matches_either() {
        // Thu 2026-07-02 → Friday the 3rd, though it is not the 13th.
        assert_eq!(
            next_cron_delay_secs("0 0 13 * 5", &at("2026-07-02T12:00:00Z")).unwrap(),
            delay_to("2026-07-02T12:00:00Z", "2026-07-03T00:00:00Z")
        );
        // Sat 2026-07-11 → Monday the 13th, before the next Friday (the 17th).
        assert_eq!(
            next_cron_delay_secs("0 0 13 * 5", &at("2026-07-11T12:00:00Z")).unwrap(),
            delay_to("2026-07-11T12:00:00Z", "2026-07-13T00:00:00Z")
        );
    }

    #[test]
    fn month_restricts_the_either_match() {
        // "1st or Monday" limited to August: first match after 2026-07-02 is Sat 2026-08-01.
        assert_eq!(
            next_cron_delay_secs("0 9 1 AUG MON", &at("2026-07-02T12:00:00Z")).unwrap(),
            delay_to("2026-07-02T12:00:00Z", "2026-08-01T09:00:00Z")
        );
    }

    #[test]
    fn weekday_range_skips_weekend() {
        // Friday 2026-07-17 10:00 → next "0 9 * * 1-5" is Monday 2026-07-20 09:00.
        let now = at("2026-07-17T10:00:00Z");
        let expected = (at("2026-07-20T09:00:00Z") - now).num_seconds() as u64;
        assert_eq!(next_cron_delay_secs("0 9 * * 1-5", &now).unwrap(), expected);
    }

    #[test]
    fn leap_day_schedule_is_found_beyond_366_days() {
        let now = at("2026-03-01T00:00:00Z");
        let expected = (at("2028-02-29T00:00:00Z") - now).num_seconds() as u64;
        assert_eq!(next_cron_delay_secs("0 0 29 2 *", &now).unwrap(), expected);
    }

    #[test]
    fn impossible_date_fails_with_clear_error() {
        let err = next_cron_delay_secs("0 0 30 2 *", &at("2026-01-01T00:00:00Z")).unwrap_err();
        assert!(err.contains("No matching cron time"), "{err}");
    }

    #[test]
    fn out_of_range_minute_fails_immediately_with_field_name() {
        let err = next_cron_delay_secs("75 * * * *", &Utc::now()).unwrap_err();
        assert!(err.contains("minute"), "{err}");
    }
}
