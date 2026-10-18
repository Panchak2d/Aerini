//! Environment-variable names for credentials in exported server packages.
//!
//! The desktop export and `aerini-server serve` both go through this module so
//! a credential id is turned into exactly one variable name everywhere.

use std::collections::HashMap;
use std::ffi::OsString;

pub const ENV_PREFIX: &str = "AERINI_CRED_";

/// `openai-prod` becomes `AERINI_CRED_OPENAI_PROD`: ASCII letters are
/// uppercased and every other character becomes `_`.
pub fn env_var_name(credential_id: &str) -> String {
    let mut name = String::with_capacity(ENV_PREFIX.len() + credential_id.len());
    name.push_str(ENV_PREFIX);
    name.extend(credential_id.chars().map(|c| {
        if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }
    }));
    name
}

/// Two credential ids that would share one variable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("credentials '{first}' and '{second}' both map to environment variable {var}")]
pub struct EnvNameCollision {
    pub first:  String,
    pub second: String,
    pub var:    String,
}

/// Maps each distinct id to its variable name, in the order given. Fails on the
/// first pair of different ids that land on the same name.
pub fn assign_env_vars<'a>(
    ids: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<(String, String)>, EnvNameCollision> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut owner: HashMap<String, String> = HashMap::new();
    for id in ids {
        if out.iter().any(|(seen, _)| seen == id) {
            continue;
        }
        let var = env_var_name(id);
        if let Some(first) = owner.get(&var) {
            return Err(EnvNameCollision { first: first.clone(), second: id.to_string(), var });
        }
        owner.insert(var.clone(), id.to_string());
        out.push((id.to_string(), var));
    }
    Ok(out)
}

/// Checks a name taken from a config file: it must carry [`ENV_PREFIX`] followed
/// by at least one of `A-Z`, `0-9`, `_`, so a mapping can never point at
/// `AERINI_TOKEN`, `PATH` or any other variable the server did not hand out.
pub fn validate_mapped_name(name: &str) -> Result<(), String> {
    let rest = name.strip_prefix(ENV_PREFIX).ok_or_else(|| {
        format!("'{name}' must start with {ENV_PREFIX}")
    })?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') {
        return Err(format!(
            "'{name}' must be {ENV_PREFIX} followed by uppercase letters, digits or underscores"
        ));
    }
    Ok(())
}

/// Checks every mapped name and that no two ids share one. Ids are visited in
/// sorted order so the error names the same pair on every start.
pub fn validate_mapping(mapping: &HashMap<String, String>) -> Result<(), String> {
    let mut ids: Vec<&String> = mapping.keys().collect();
    ids.sort();
    let mut owner: HashMap<&str, &str> = HashMap::new();
    for id in ids {
        let var = mapping[id].as_str();
        validate_mapped_name(var).map_err(|e| format!("credential '{id}': {e}"))?;
        if let Some(first) = owner.insert(var, id) {
            return Err(EnvNameCollision {
                first:  first.to_string(),
                second: id.clone(),
                var:    var.to_string(),
            }
            .to_string());
        }
    }
    Ok(())
}

/// What reading one variable found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvLookup {
    Value(String),
    NotSet,
    Empty,
    NotUnicode,
}

pub fn lookup_env(name: &str) -> EnvLookup {
    lookup_os(std::env::var_os(name))
}

fn lookup_os(value: Option<OsString>) -> EnvLookup {
    match value {
        None => EnvLookup::NotSet,
        Some(v) if v.is_empty() => EnvLookup::Empty,
        Some(v) => match v.into_string() {
            Ok(s) => EnvLookup::Value(s),
            Err(_) => EnvLookup::NotUnicode,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_prefixed_uppercased_and_replaces_other_characters() {
        assert_eq!(env_var_name("openai-prod"), "AERINI_CRED_OPENAI_PROD");
        assert_eq!(env_var_name("a.b c"), "AERINI_CRED_A_B_C");
        assert_eq!(env_var_name("é"), "AERINI_CRED__");
    }

    #[test]
    fn ids_that_share_a_name_are_reported_with_both_ids() {
        let err = assign_env_vars(["a-b", "x", "a_b"]).unwrap_err();
        assert_eq!(err.first, "a-b");
        assert_eq!(err.second, "a_b");
        assert_eq!(err.var, "AERINI_CRED_A_B");
        assert!(err.to_string().contains("'a-b'") && err.to_string().contains("'a_b'"));
    }

    #[test]
    fn repeated_id_is_assigned_once() {
        let out = assign_env_vars(["a", "b", "a"]).unwrap();
        assert_eq!(out, vec![
            ("a".to_string(), "AERINI_CRED_A".to_string()),
            ("b".to_string(), "AERINI_CRED_B".to_string()),
        ]);
    }

    #[test]
    fn mapped_names_need_the_prefix_and_a_safe_suffix() {
        assert!(validate_mapped_name("AERINI_CRED_X1").is_ok());
        for bad in ["AERINI_TOKEN", "PATH", "AERINI_CRED_", "AERINI_CRED_lower", "AERINI_CRED_A=B", "aerini_cred_x"] {
            assert!(validate_mapped_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn mapping_with_a_shared_variable_names_both_ids() {
        let mut m = HashMap::new();
        m.insert("one".to_string(), "AERINI_CRED_SAME".to_string());
        m.insert("two".to_string(), "AERINI_CRED_SAME".to_string());
        let err = validate_mapping(&m).unwrap_err();
        assert!(err.contains("'one'") && err.contains("'two'"), "{err}");
        m.insert("two".to_string(), "AERINI_CRED_OTHER".to_string());
        assert!(validate_mapping(&m).is_ok());
    }

    #[test]
    fn lookup_separates_unset_empty_value_and_invalid_unicode() {
        assert_eq!(lookup_os(None), EnvLookup::NotSet);
        assert_eq!(lookup_os(Some(OsString::new())), EnvLookup::Empty);
        assert_eq!(lookup_os(Some("v".into())), EnvLookup::Value("v".into()));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            assert_eq!(lookup_os(Some(OsString::from_vec(vec![0xff, 0xfe]))), EnvLookup::NotUnicode);
        }
    }
}
