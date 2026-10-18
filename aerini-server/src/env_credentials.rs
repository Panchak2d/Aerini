use std::collections::HashMap;
use aerini_engine::credential_env::{lookup_env, validate_mapping, EnvLookup};
use aerini_engine::executor::{CredentialResolveError, CredentialResolver, SecretString};

/// Resolves workflow credentials from environment variables named in the
/// config's `credential_env_vars` mapping.
///
/// A credential id that is not in the mapping is never looked up in the
/// environment: `resolve` reports it as not found, which the executor logs as a
/// warning and treats as "field left unset". `validate_all` is the startup
/// check for every mapped variable.
pub struct EnvCredentialResolver {
    /// Maps credential_id → env_var_name. Built from the config file.
    mapping: HashMap<String, String>,
}

impl EnvCredentialResolver {
    /// Fails when a mapped name lacks the `AERINI_CRED_` prefix or two ids share
    /// one variable.
    pub fn new(mapping: HashMap<String, String>) -> Result<Self, String> {
        validate_mapping(&mapping)?;
        Ok(Self { mapping })
    }

    /// One message per mapped variable that is unset, empty or not valid
    /// Unicode, sorted by variable name.
    pub fn validate_all(&self) -> Result<(), Vec<String>> {
        let mut problems: Vec<(&str, &str)> = self.mapping.values()
            .filter_map(|var| match lookup_env(var) {
                EnvLookup::Value(_)  => None,
                EnvLookup::NotSet    => Some((var.as_str(), "is not set")),
                EnvLookup::Empty     => Some((var.as_str(), "is set but empty")),
                EnvLookup::NotUnicode => Some((var.as_str(), "is not valid Unicode")),
            })
            .collect();
        problems.sort();
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.into_iter().map(|(var, why)| format!("{var} {why}")).collect())
        }
    }
}

#[async_trait::async_trait]
impl CredentialResolver for EnvCredentialResolver {
    async fn resolve(&self, credential_id: &str) -> Result<SecretString, CredentialResolveError> {
        let Some(var_name) = self.mapping.get(credential_id) else {
            tracing::warn!(credential = %credential_id, "credential is not in credential_env_vars");
            return Err(CredentialResolveError::NotFound);
        };
        match lookup_env(var_name) {
            EnvLookup::Value(val) => Ok(SecretString::new(val)),
            EnvLookup::Empty => {
                tracing::warn!(var = %var_name, credential = %credential_id, "env var is set but empty for credential");
                Err(CredentialResolveError::NotFound)
            }
            EnvLookup::NotSet => {
                tracing::warn!(var = %var_name, credential = %credential_id, "env var not set for credential");
                Err(CredentialResolveError::NotFound)
            }
            EnvLookup::NotUnicode => Err(CredentialResolveError::Unreadable(
                format!("environment variable {var_name} is not valid Unicode"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(pairs: &[(&str, &str)]) -> EnvCredentialResolver {
        let mapping = pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        EnvCredentialResolver::new(mapping).unwrap()
    }

    #[test]
    #[serial_test::serial]
    fn validate_all_reports_unset_and_empty_variables_by_name() {
        std::env::set_var("AERINI_CRED_TEST_VALIDATE_EMPTY", "");
        std::env::remove_var("AERINI_CRED_TEST_VALIDATE_UNSET");
        std::env::set_var("AERINI_CRED_TEST_VALIDATE_SET", "sk-test-value");
        let r = resolver(&[
            ("a", "AERINI_CRED_TEST_VALIDATE_EMPTY"),
            ("b", "AERINI_CRED_TEST_VALIDATE_UNSET"),
            ("c", "AERINI_CRED_TEST_VALIDATE_SET"),
        ]);
        let result = r.validate_all();
        std::env::remove_var("AERINI_CRED_TEST_VALIDATE_EMPTY");
        std::env::remove_var("AERINI_CRED_TEST_VALIDATE_SET");
        assert_eq!(
            result.unwrap_err(),
            vec![
                "AERINI_CRED_TEST_VALIDATE_EMPTY is set but empty".to_string(),
                "AERINI_CRED_TEST_VALIDATE_UNSET is not set".to_string(),
            ]
        );
    }

    #[test]
    fn mapping_outside_the_prefix_or_with_a_shared_variable_is_refused() {
        let bad = |pairs: &[(&str, &str)]| {
            let m = pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
            EnvCredentialResolver::new(m).err().expect("must be refused")
        };
        assert!(bad(&[("t", "AERINI_TOKEN")]).contains("AERINI_CRED_"));
        let shared = bad(&[("one", "AERINI_CRED_X"), ("two", "AERINI_CRED_X")]);
        assert!(shared.contains("'one'") && shared.contains("'two'"), "{shared}");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn resolve_returns_the_value_of_a_mapped_variable() {
        std::env::set_var("AERINI_CRED_TEST_SET", "sk-test-value");
        let result = resolver(&[("test-cred", "AERINI_CRED_TEST_SET")]).resolve("test-cred").await;
        std::env::remove_var("AERINI_CRED_TEST_SET");
        assert_eq!(result.unwrap().as_str(), "sk-test-value");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn resolve_never_reads_the_naming_convention_for_an_unmapped_id() {
        std::env::set_var("AERINI_CRED_UNMAPPED_ID", "leaked");
        let result = resolver(&[("other", "AERINI_CRED_OTHER")]).resolve("unmapped-id").await;
        std::env::remove_var("AERINI_CRED_UNMAPPED_ID");
        assert!(matches!(result, Err(CredentialResolveError::NotFound)));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn resolve_returns_not_found_when_var_is_unset_or_empty() {
        std::env::remove_var("AERINI_CRED_TEST_UNSET");
        std::env::set_var("AERINI_CRED_TEST_EMPTY", "");
        let r = resolver(&[("u", "AERINI_CRED_TEST_UNSET"), ("e", "AERINI_CRED_TEST_EMPTY")]);
        let unset = r.resolve("u").await;
        let empty = r.resolve("e").await;
        std::env::remove_var("AERINI_CRED_TEST_EMPTY");
        assert!(matches!(unset, Err(CredentialResolveError::NotFound)));
        assert!(matches!(empty, Err(CredentialResolveError::NotFound)));
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn resolve_reports_a_variable_that_is_not_valid_unicode_as_unreadable() {
        use std::os::unix::ffi::OsStrExt;
        std::env::set_var("AERINI_CRED_TEST_BYTES", std::ffi::OsStr::from_bytes(&[0xff, 0xfe]));
        let r = resolver(&[("b", "AERINI_CRED_TEST_BYTES")]);
        let result = r.resolve("b").await;
        let validated = r.validate_all();
        std::env::remove_var("AERINI_CRED_TEST_BYTES");
        assert!(matches!(result, Err(CredentialResolveError::Unreadable(_))));
        assert_eq!(validated.unwrap_err(), vec!["AERINI_CRED_TEST_BYTES is not valid Unicode".to_string()]);
    }
}
