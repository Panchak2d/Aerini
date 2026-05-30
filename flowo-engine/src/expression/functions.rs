//! Built-in expression functions: string, number, date, array/object, conditional.

use serde_json::Value;
use super::resolver::value_to_string;

/// Apply a named function to resolved string arguments.
pub(super) fn apply_function(name: &str, args: &[String], warnings: &mut Vec<String>) -> String {
    macro_rules! need {
        ($n:expr) => {
            if args.len() < $n {
                warnings.push(format!(
                    "{}() requires at least {} argument(s), got {}",
                    name, $n, args.len()
                ));
                return String::new();
            }
        };
    }

    match name {
        // ── String ────────────────────────────────────────────────────────────
        "upper"      => return args.first().map(|s| s.to_uppercase()).unwrap_or_default(),
        "lower"      => return args.first().map(|s| s.to_lowercase()).unwrap_or_default(),
        "trim"       => return args.first().map(|s| s.trim().to_string()).unwrap_or_default(),
        "trim_start" => return args.first().map(|s| s.trim_start().to_string()).unwrap_or_default(),
        "trim_end"   => return args.first().map(|s| s.trim_end().to_string()).unwrap_or_default(),
        _ => {}
    }

    match name {
        "len" => {
            let s = args.first().map(String::as_str).unwrap_or("");
            if let Ok(Value::Array(a)) = serde_json::from_str::<Value>(s) {
                return a.len().to_string();
            }
            s.chars().count().to_string()
        }
        "contains" => {
            need!(2);
            args[0].contains(args[1].as_str()).to_string()
        }
        "starts_with" => {
            need!(2);
            args[0].starts_with(args[1].as_str()).to_string()
        }
        "ends_with" => {
            need!(2);
            args[0].ends_with(args[1].as_str()).to_string()
        }
        "slice" => {
            need!(3);
            let chars: Vec<char> = args[0].chars().collect();
            let len = chars.len();
            let start = parse_usize_clamped(&args[1], len);
            let end   = parse_usize_clamped(&args[2], len);
            if start > end { return String::new(); }
            chars[start..end].iter().collect()
        }
        "replace" => {
            need!(3);
            match args[0].find(args[1].as_str()) {
                Some(pos) => {
                    let mut r = args[0][..pos].to_string();
                    r.push_str(&args[2]);
                    r.push_str(&args[0][pos + args[1].len()..]);
                    r
                }
                None => args[0].clone(),
            }
        }
        "replace_all" => {
            need!(3);
            args[0].replace(args[1].as_str(), &args[2])
        }
        "split" => {
            need!(2);
            let parts: Vec<Value> = args[0].split(args[1].as_str())
                .map(|p| Value::String(p.to_string()))
                .collect();
            Value::Array(parts).to_string()
        }
        "join" => {
            need!(2);
            match serde_json::from_str::<Value>(&args[0]) {
                Ok(Value::Array(arr)) => arr.iter()
                    .map(value_to_string)
                    .collect::<Vec<_>>()
                    .join(&args[1]),
                _ => args[0].clone(),
            }
        }
        "pad_start" => {
            need!(3);
            let target = parse_usize_clamped(&args[1], 10_000);
            let pad_ch = args[2].chars().next().unwrap_or(' ');
            let n = args[0].chars().count();
            if n >= target { return args[0].clone(); }
            let padding: String = std::iter::repeat_n(pad_ch, target - n).collect();
            format!("{}{}", padding, args[0])
        }
        "pad_end" => {
            need!(3);
            let target = parse_usize_clamped(&args[1], 10_000);
            let pad_ch = args[2].chars().next().unwrap_or(' ');
            let n = args[0].chars().count();
            if n >= target { return args[0].clone(); }
            let padding: String = std::iter::repeat_n(pad_ch, target - n).collect();
            format!("{}{}", args[0], padding)
        }

        // ── Number ────────────────────────────────────────────────────────────
        "round" => {
            let n = parse_f64_arg(args.first());
            let decimals = args.get(1).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
            let factor = 10f64.powi(decimals as i32);
            format!("{:.prec$}", (n * factor).round() / factor, prec = decimals as usize)
        }
        "floor" => parse_f64_arg(args.first()).floor().to_string(),
        "ceil"  => parse_f64_arg(args.first()).ceil().to_string(),
        "abs"   => parse_f64_arg(args.first()).abs().to_string(),
        "min"   => {
            need!(2);
            parse_f64_arg(Some(&args[0])).min(parse_f64_arg(Some(&args[1]))).to_string()
        }
        "max"   => {
            need!(2);
            parse_f64_arg(Some(&args[0])).max(parse_f64_arg(Some(&args[1]))).to_string()
        }
        "to_int" => {
            let s = args.first().map(|s| s.trim()).unwrap_or("");
            s.parse::<i64>()
                .or_else(|_| s.parse::<f64>().map(|f| f as i64))
                .map(|n| n.to_string())
                .unwrap_or_default()
        }
        "to_float" => {
            args.first().map(|s| s.trim()).unwrap_or("")
                .parse::<f64>().map(|n| n.to_string()).unwrap_or_default()
        }

        // ── Date ──────────────────────────────────────────────────────────────
        "now" => chrono::Utc::now().to_rfc3339(),
        "format_date" => {
            if args.is_empty() {
                warnings.push("format_date() requires at least 1 argument: format_date(timestamp, format)".to_string());
                return String::new();
            }
            let fmt = args.get(1).map(String::as_str).unwrap_or("YYYY-MM-DD");
            format_date_string(&args[0], fmt, warnings)
        }
        "parse_date" => {
            if args.is_empty() {
                warnings.push("parse_date() requires 1 argument: parse_date(string)".to_string());
                return String::new();
            }
            parse_date_to_iso(&args[0], warnings)
        }
        "add_days" => {
            need!(2);
            date_add(&args[0], &args[1], "days", warnings)
        }
        "add_hours" => {
            need!(2);
            date_add(&args[0], &args[1], "hours", warnings)
        }
        "add_minutes" => {
            need!(2);
            date_add(&args[0], &args[1], "minutes", warnings)
        }
        "date_diff" => {
            need!(3);
            date_diff(&args[0], &args[1], &args[2], warnings)
        }

        // ── Array / Object ────────────────────────────────────────────────────
        "first" => {
            let s = args.first().map(String::as_str).unwrap_or("[]");
            match serde_json::from_str::<Value>(s) {
                Ok(Value::Array(arr)) => arr.first().map(value_to_string).unwrap_or_default(),
                _ => String::new(),
            }
        }
        "last" => {
            let s = args.first().map(String::as_str).unwrap_or("[]");
            match serde_json::from_str::<Value>(s) {
                Ok(Value::Array(arr)) => arr.last().map(value_to_string).unwrap_or_default(),
                _ => String::new(),
            }
        }
        "nth" => {
            need!(2);
            let idx = parse_usize_clamped(&args[1], usize::MAX);
            match serde_json::from_str::<Value>(&args[0]) {
                Ok(Value::Array(arr)) => arr.get(idx).map(value_to_string).unwrap_or_default(),
                _ => String::new(),
            }
        }
        "keys" => {
            let s = args.first().map(String::as_str).unwrap_or("{}");
            match serde_json::from_str::<Value>(s) {
                Ok(Value::Object(map)) => {
                    let keys: Vec<Value> = map.keys().map(|k| Value::String(k.clone())).collect();
                    Value::Array(keys).to_string()
                }
                _ => "[]".to_string(),
            }
        }
        "values" => {
            let s = args.first().map(String::as_str).unwrap_or("{}");
            match serde_json::from_str::<Value>(s) {
                Ok(Value::Object(map)) => {
                    let vals: Vec<Value> = map.values().cloned().collect();
                    Value::Array(vals).to_string()
                }
                _ => "[]".to_string(),
            }
        }

        // ── Conditional ───────────────────────────────────────────────────────
        "if" => {
            need!(3);
            if is_truthy(&args[0]) { args[1].clone() } else { args[2].clone() }
        }

        // ── Unknown ───────────────────────────────────────────────────────────
        other => {
            warnings.push(format!(
                "Unknown function '{}'. See the Functions section in the expression picker for all available functions.",
                other
            ));
            String::new()
        }
    }
}

fn format_date_string(ts: &str, fmt: &str, warnings: &mut Vec<String>) -> String {
    use chrono::{Datelike, DateTime, Timelike, Utc};
    let dt: DateTime<Utc> = match ts.parse::<DateTime<Utc>>()
        .or_else(|_| chrono::DateTime::parse_from_rfc2822(ts).map(|d| d.with_timezone(&Utc)))
    {
        Ok(d) => d,
        Err(_) => {
            warnings.push(format!("format_date(): could not parse '{}' as an ISO 8601 timestamp", ts));
            return String::new();
        }
    };
    fmt.replace("YYYY", &format!("{:04}", dt.year()))
       .replace("MM",   &format!("{:02}", dt.month()))
       .replace("DD",   &format!("{:02}", dt.day()))
       .replace("HH",   &format!("{:02}", dt.hour()))
       .replace("mm",   &format!("{:02}", dt.minute()))
       .replace("ss",   &format!("{:02}", dt.second()))
}

fn parse_date_to_iso(s: &str, warnings: &mut Vec<String>) -> String {
    use chrono::{DateTime, NaiveDate, TimeZone, Utc};
    if let Ok(d) = s.parse::<DateTime<Utc>>() { return d.to_rfc3339(); }
    if let Ok(d) = chrono::DateTime::parse_from_rfc2822(s).map(|d| d.with_timezone(&Utc)) {
        return d.to_rfc3339();
    }
    if let Ok(nd) = NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d") {
        if let Some(naive_dt) = nd.and_hms_opt(0, 0, 0) {
            return Utc.from_utc_datetime(&naive_dt).to_rfc3339();
        }
    }
    warnings.push(format!("parse_date(): could not parse '{}' as a date", s));
    String::new()
}

fn date_add(ts: &str, n_str: &str, unit: &str, warnings: &mut Vec<String>) -> String {
    use chrono::{DateTime, Duration, Utc};
    let dt: DateTime<Utc> = match ts.parse() {
        Ok(d) => d,
        Err(_) => { warnings.push(format!("date arithmetic: invalid timestamp '{}'", ts)); return String::new(); }
    };
    let n: i64 = match n_str.trim().parse() {
        Ok(v) => v,
        Err(_) => { warnings.push(format!("date arithmetic: '{}' is not a valid integer", n_str)); return String::new(); }
    };
    let result = match unit {
        "days"    => dt + Duration::days(n),
        "hours"   => dt + Duration::hours(n),
        "minutes" => dt + Duration::minutes(n),
        _         => dt,
    };
    result.to_rfc3339()
}

fn date_diff(ts1: &str, ts2: &str, unit: &str, warnings: &mut Vec<String>) -> String {
    use chrono::{DateTime, Utc};
    let d1: DateTime<Utc> = match ts1.parse() {
        Ok(d) => d,
        Err(_) => { warnings.push(format!("date_diff(): invalid timestamp '{}'", ts1)); return String::new(); }
    };
    let d2: DateTime<Utc> = match ts2.parse() {
        Ok(d) => d,
        Err(_) => { warnings.push(format!("date_diff(): invalid timestamp '{}'", ts2)); return String::new(); }
    };
    let diff = d1.signed_duration_since(d2);
    match unit {
        "days"    => diff.num_days().to_string(),
        "hours"   => diff.num_hours().to_string(),
        "minutes" => diff.num_minutes().to_string(),
        "seconds" => diff.num_seconds().to_string(),
        _ => {
            warnings.push(format!("date_diff(): unknown unit '{}'. Valid: days, hours, minutes, seconds", unit));
            String::new()
        }
    }
}

fn parse_f64_arg(s: Option<&String>) -> f64 {
    s.and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.0)
}

fn parse_usize_clamped(s: &str, max: usize) -> usize {
    s.trim().parse::<i64>().map(|n| (n.max(0) as usize).min(max)).unwrap_or(0)
}

fn is_truthy(s: &str) -> bool {
    !s.is_empty() && s != "false" && s != "0" && s != "null"
}
