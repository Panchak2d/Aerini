//! Column types the Postgres and MySQL drivers return as raw bytes. Each
//! function reads one wire format and returns `None` for bytes it does not
//! recognise, so the caller reports the column as unsupported instead of
//! guessing.

use serde_json::Value;

const MICROS_PER_SECOND: i64 = 1_000_000;
const MICROS_PER_DAY: i64 = 86_400 * MICROS_PER_SECOND;

fn pg_epoch() -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(2000, 1, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .expect("2000-01-01 is a valid date")
}

fn fraction(micros: u32) -> String {
    if micros == 0 {
        return String::new();
    }
    let digits = format!("{:06}", micros);
    format!(".{}", digits.trim_end_matches('0'))
}

/// Postgres `NUMERIC`: base-10000 digits, as an exact decimal string.
pub(super) fn pg_numeric(bytes: &[u8]) -> Option<String> {
    const NEGATIVE: u16 = 0x4000;
    const NAN: u16 = 0xC000;
    const POSITIVE_INFINITY: u16 = 0xD000;
    const NEGATIVE_INFINITY: u16 = 0xF000;

    let header = bytes.get(..8)?;
    let digit_count = usize::try_from(i16::from_be_bytes([header[0], header[1]])).ok()?;
    let weight = i32::from(i16::from_be_bytes([header[2], header[3]]));
    let sign = u16::from_be_bytes([header[4], header[5]]);
    let scale = usize::from(u16::from_be_bytes([header[6], header[7]]));
    let body = bytes.get(8..)?;
    if body.len() != digit_count * 2 {
        return None;
    }
    match sign {
        NAN => return Some("NaN".to_string()),
        POSITIVE_INFINITY => return Some("Infinity".to_string()),
        NEGATIVE_INFINITY => return Some("-Infinity".to_string()),
        0 | NEGATIVE => {}
        _ => return None,
    }
    let digits: Vec<u16> = body
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_be_bytes(pair))
        .collect();
    if digits.iter().any(|&d| d > 9999) {
        return None;
    }
    let group = |power: i32| -> u16 {
        usize::try_from(weight - power)
            .ok()
            .and_then(|index| digits.get(index))
            .copied()
            .unwrap_or(0)
    };

    let mut text = String::new();
    if sign == NEGATIVE {
        text.push('-');
    }
    if weight < 0 {
        text.push('0');
    } else {
        text.push_str(&group(weight).to_string());
        for power in (0..weight).rev() {
            text.push_str(&format!("{:04}", group(power)));
        }
    }
    if scale > 0 {
        let mut fractional = String::with_capacity(scale + 4);
        let mut power = -1;
        while fractional.len() < scale {
            fractional.push_str(&format!("{:04}", group(power)));
            power -= 1;
        }
        fractional.truncate(scale);
        text.push('.');
        text.push_str(&fractional);
    }
    Some(text)
}

/// Postgres `DATE`: days since 2000-01-01.
pub(super) fn pg_date(bytes: &[u8]) -> Option<String> {
    let days = i32::from_be_bytes(bytes.try_into().ok()?);
    match days {
        i32::MAX => return Some("infinity".to_string()),
        i32::MIN => return Some("-infinity".to_string()),
        _ => {}
    }
    let date = pg_epoch().date().checked_add_signed(chrono::TimeDelta::try_days(i64::from(days))?)?;
    Some(date.format("%Y-%m-%d").to_string())
}

/// Postgres `TIMESTAMP` and `TIMESTAMPTZ`: microseconds since 2000-01-01. A
/// `TIMESTAMPTZ` is returned in UTC with a trailing `Z`.
pub(super) fn pg_timestamp(bytes: &[u8], with_zone: bool) -> Option<String> {
    let micros = i64::from_be_bytes(bytes.try_into().ok()?);
    match micros {
        i64::MAX => return Some("infinity".to_string()),
        i64::MIN => return Some("-infinity".to_string()),
        _ => {}
    }
    let moment = pg_epoch().checked_add_signed(chrono::TimeDelta::microseconds(micros))?;
    let text = format!(
        "{}{}",
        moment.format("%Y-%m-%dT%H:%M:%S"),
        fraction(moment.and_utc().timestamp_subsec_micros())
    );
    Some(if with_zone { format!("{}Z", text) } else { text })
}

/// Postgres `TIME`: microseconds since midnight. `24:00:00` is a valid value.
pub(super) fn pg_time(bytes: &[u8]) -> Option<String> {
    let micros = i64::from_be_bytes(bytes.try_into().ok()?);
    if !(0..=MICROS_PER_DAY).contains(&micros) {
        return None;
    }
    let seconds = micros / MICROS_PER_SECOND;
    Some(format!(
        "{:02}:{:02}:{:02}{}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        fraction((micros % MICROS_PER_SECOND) as u32)
    ))
}

pub(super) fn pg_uuid(bytes: &[u8]) -> Option<String> {
    let b: [u8; 16] = bytes.try_into().ok()?;
    let hex = |range: std::ops::Range<usize>| b[range].iter().map(|x| format!("{:02x}", x)).collect::<String>();
    Some(format!("{}-{}-{}-{}-{}", hex(0..4), hex(4..6), hex(6..8), hex(8..10), hex(10..16)))
}

/// Postgres `JSON`, or `JSONB` (a version byte, then the text), as a JSON value.
pub(super) fn pg_json(bytes: &[u8], binary: bool) -> Option<Value> {
    let text = if binary {
        match bytes.split_first()? {
            (1, rest) => rest,
            _ => return None,
        }
    } else {
        bytes
    };
    serde_json::from_slice(text).ok()
}

/// MySQL `DATE`, `DATETIME` and `TIMESTAMP`: a length byte (0, 4, 7 or 11), then
/// year (two bytes, little-endian), month, day, hour, minute, second and
/// microseconds (four bytes, little-endian). Length 0 is MySQL's zero date. A
/// `DATE` column returns the date alone.
pub(super) fn mysql_datetime(bytes: &[u8], date_only: bool) -> Option<String> {
    let (&length, body) = bytes.split_first()?;
    if body.len() != usize::from(length) || !matches!(length, 0 | 4 | 7 | 11) {
        return None;
    }
    let field = |i: usize| body.get(i).copied().map_or(0, u32::from);
    let year = if length >= 4 { u32::from(u16::from_le_bytes([body[0], body[1]])) } else { 0 };
    let date = format!("{:04}-{:02}-{:02}", year, field(2), field(3));
    if date_only {
        return Some(date);
    }
    let micros = if length == 11 { u32::from_le_bytes([body[7], body[8], body[9], body[10]]) } else { 0 };
    Some(format!(
        "{}T{:02}:{:02}:{:02}{}",
        date,
        field(4),
        field(5),
        field(6),
        fraction(micros)
    ))
}

/// MySQL `TIME`: a length byte (0, 8 or 12), a sign byte, days (four bytes,
/// little-endian), hour, minute, second and microseconds (four bytes). The
/// value can be negative and run past 24 hours (-838:59:59 to 838:59:59), so
/// days fold into the hour count.
pub(super) fn mysql_time(bytes: &[u8]) -> Option<String> {
    let (&length, body) = bytes.split_first()?;
    if body.len() != usize::from(length) || !matches!(length, 0 | 8 | 12) {
        return None;
    }
    if length == 0 {
        return Some("00:00:00".to_string());
    }
    let negative = body[0] != 0;
    let days = u32::from_le_bytes([body[1], body[2], body[3], body[4]]);
    let micros = if length == 12 { u32::from_le_bytes([body[8], body[9], body[10], body[11]]) } else { 0 };
    Some(format!(
        "{}{:02}:{:02}:{:02}{}",
        if negative { "-" } else { "" },
        u64::from(days) * 24 + u64::from(body[5]),
        body[6],
        body[7],
        fraction(micros)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn numeric(digits: &[u16], weight: i16, sign: u16, scale: u16) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&(digits.len() as i16).to_be_bytes());
        b.extend_from_slice(&weight.to_be_bytes());
        b.extend_from_slice(&sign.to_be_bytes());
        b.extend_from_slice(&scale.to_be_bytes());
        for d in digits {
            b.extend_from_slice(&d.to_be_bytes());
        }
        b
    }

    #[test]
    fn numeric_keeps_every_digit_and_the_declared_scale() {
        assert_eq!(pg_numeric(&numeric(&[123, 4500], 0, 0, 2)).unwrap(), "123.45");
        assert_eq!(pg_numeric(&numeric(&[12, 3456, 7890], 1, 0, 0)).unwrap(), "123456");
        assert_eq!(pg_numeric(&numeric(&[1, 0, 5], 1, 0x4000, 4)).unwrap(), "-10000.0005");
        assert_eq!(pg_numeric(&numeric(&[5], -1, 0, 4)).unwrap(), "0.0005");
        assert_eq!(pg_numeric(&numeric(&[5000], -2, 0, 5)).unwrap(), "0.00005");
        assert_eq!(pg_numeric(&numeric(&[], 0, 0, 3)).unwrap(), "0.000");
        assert_eq!(pg_numeric(&numeric(&[1], 2, 0, 0)).unwrap(), "100000000");
    }

    #[test]
    fn numeric_special_and_malformed_values() {
        assert_eq!(pg_numeric(&numeric(&[], 0, 0xC000, 0)).unwrap(), "NaN");
        assert_eq!(pg_numeric(&numeric(&[], 0, 0xF000, 0)).unwrap(), "-Infinity");
        assert_eq!(pg_numeric(&numeric(&[10000], 0, 0, 0)), None);
        assert_eq!(pg_numeric(&numeric(&[1], 0, 0x1234, 0)), None);
        assert_eq!(pg_numeric(&[0, 1, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(pg_numeric(&[]), None);
    }

    #[test]
    fn postgres_dates_and_times_are_iso_strings_with_infinity_kept() {
        assert_eq!(pg_date(&0i32.to_be_bytes()).unwrap(), "2000-01-01");
        assert_eq!(pg_date(&(-1i32).to_be_bytes()).unwrap(), "1999-12-31");
        assert_eq!(pg_date(&i32::MAX.to_be_bytes()).unwrap(), "infinity");
        assert_eq!(pg_date(&i32::MIN.to_be_bytes()).unwrap(), "-infinity");
        assert_eq!(pg_date(&2_000_000_000i32.to_be_bytes()), None);

        assert_eq!(pg_timestamp(&0i64.to_be_bytes(), false).unwrap(), "2000-01-01T00:00:00");
        assert_eq!(
            pg_timestamp(&(86_400_000_000i64 + 1_500_000).to_be_bytes(), true).unwrap(),
            "2000-01-02T00:00:01.5Z"
        );
        assert_eq!(pg_timestamp(&i64::MIN.to_be_bytes(), true).unwrap(), "-infinity");
        assert_eq!(pg_timestamp(&(i64::MAX - 1).to_be_bytes(), false), None);

        assert_eq!(pg_time(&45_296_000_123i64.to_be_bytes()).unwrap(), "12:34:56.000123");
        assert_eq!(pg_time(&MICROS_PER_DAY.to_be_bytes()).unwrap(), "24:00:00");
        assert_eq!(pg_time(&(-1i64).to_be_bytes()), None);
        assert_eq!(pg_time(&[1, 2, 3]), None);
    }

    #[test]
    fn uuid_and_json_decode_and_wrong_lengths_are_refused() {
        let id: Vec<u8> = (0u8..16).collect();
        assert_eq!(pg_uuid(&id).unwrap(), "00010203-0405-0607-0809-0a0b0c0d0e0f");
        assert_eq!(pg_uuid(&id[..15]), None);

        assert_eq!(pg_json(b"{\"a\":[1,2]}", false), Some(json!({"a": [1, 2]})));
        assert_eq!(pg_json(b"\x01{\"a\":1}", true), Some(json!({"a": 1})));
        assert_eq!(pg_json(b"\x02{\"a\":1}", true), None);
        assert_eq!(pg_json(b"not json", false), None);
    }

    #[test]
    fn mysql_dates_times_and_zero_values() {
        let datetime = [11, 0xE9, 0x07, 3, 9, 14, 5, 7, 0xA0, 0x86, 0x01, 0];
        assert_eq!(mysql_datetime(&datetime, false).unwrap(), "2025-03-09T14:05:07.1");
        assert_eq!(mysql_datetime(&[4, 0xE9, 0x07, 3, 9], true).unwrap(), "2025-03-09");
        assert_eq!(mysql_datetime(&[4, 0xE9, 0x07, 3, 9], false).unwrap(), "2025-03-09T00:00:00");
        assert_eq!(mysql_datetime(&[0], false).unwrap(), "0000-00-00T00:00:00");
        assert_eq!(mysql_datetime(&[7, 0xE9, 0x07, 3, 9], false), None);
        assert_eq!(mysql_datetime(&[5, 1, 2, 3, 4, 5], false), None);

        assert_eq!(mysql_time(&[0]).unwrap(), "00:00:00");
        let time = [12, 1, 1, 0, 0, 0, 2, 3, 4, 0x0A, 0, 0, 0];
        assert_eq!(mysql_time(&time).unwrap(), "-26:03:04.00001");
        assert_eq!(mysql_time(&[8, 0, 0, 0, 0, 0, 23, 59, 59]).unwrap(), "23:59:59");
        assert_eq!(mysql_time(&[8, 0, 0]), None);
    }
}
