use std::collections::HashMap;
use aerini_engine::executor::{CredentialResolveError, CredentialResolver};

/// Resolves workflow credentials from environment variables.
///
/// Naming convention: credential ID "openai-prod" maps to env var
/// AERINI_CRED_OPENAI_PROD (uppercased, hyphens and dots replaced with underscores).
///
/// `validate_all` enforces a hard failure at server startup, but only for
/// credential IDs present in `mapping` (the deployer's declared config) — see
/// `aerini-server::main::serve_mode`. `resolve` itself never hard-fails: a
/// missing or empty var returns `Err`, which the executor logs as a warning
/// and treats as "field left unset", not a workflow failure. A credential ID
/// a workflow references but that isn't in `mapping` skips the startup check
/// entirely and only ever surfaces at that runtime warning.
pub struct EnvCredentialResolver {
    /// Maps credential_id → env_var_name. Built from the config file.
    mapping: HashMap<String, String>,
}

impl EnvCredentialResolver {
    pub fn new(mapping: HashMap<String, String>) -> Self {
        Self { mapping }
    }

    /// Validate all required credentials are present before starting.
    /// Returns a list of missing variable names, or Ok(()) if all are set.
    pub fn validate_all(&self) -> Result<(), Vec<String>> {
        let missing: Vec<String> = self.mapping.values()
            .filter(|var| std::env::var(var).is_err())
            .cloned()
            .collect();
        if missing.is_empty() { Ok(()) } else { Err(missing) }
    }
}

pub fn credential_id_to_env_var(id: &str) -> String {
    format!(
        "AERINI_CRED_{}",
        id.to_uppercase()
            .replace(['-', '.'], "_")
    )
}

#[async_trait::async_trait]
impl CredentialResolver for EnvCredentialResolver {
    async fn resolve(&self, credential_id: &str) -> Result<String, CredentialResolveError> {
        let var_name = self.mapping
            .get(credential_id)
            .cloned()
            .unwrap_or_else(|| credential_id_to_env_var(credential_id));

        match std::env::var(&var_name) {
            Ok(val) if !val.is_empty() => Ok(val),
            Ok(_) => {
                tracing::warn!(
                    var = %var_name,
                    credential = %credential_id,
                    "env var is set but empty for credential"
                );
                Err(CredentialResolveError::NotFound)
            }
            Err(_) => {
                tracing::warn!(
                    var = %var_name,
                    credential = %credential_id,
                    "env var not set for credential"
                );
                Err(CredentialResolveError::NotFound)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[serial_test::serial]
    async fn resolve_returns_ok_when_var_is_set() {
        std::env::set_var("AERINI_ENV_CRED_TEST_SET", "sk-test-value");
        let mut mapping = HashMap::new();
        mapping.insert("test-cred".to_string(), "AERINI_ENV_CRED_TEST_SET".to_string());
        let resolver = EnvCredentialResolver::new(mapping);

        let result = resolver.resolve("test-cred").await;
        std::env::remove_var("AERINI_ENV_CRED_TEST_SET");

        assert_eq!(result.unwrap(), "sk-test-value");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn resolve_returns_not_found_when_var_is_unset() {
        std::env::remove_var("AERINI_ENV_CRED_TEST_UNSET");
        let mut mapping = HashMap::new();
        mapping.insert("test-cred".to_string(), "AERINI_ENV_CRED_TEST_UNSET".to_string());
        let resolver = EnvCredentialResolver::new(mapping);

        let result = resolver.resolve("test-cred").await;
        assert!(matches!(result, Err(CredentialResolveError::NotFound)));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn resolve_returns_not_found_when_var_is_empty() {
        std::env::set_var("AERINI_ENV_CRED_TEST_EMPTY", "");
        let mut mapping = HashMap::new();
        mapping.insert("test-cred".to_string(), "AERINI_ENV_CRED_TEST_EMPTY".to_string());
        let resolver = EnvCredentialResolver::new(mapping);

        let result = resolver.resolve("test-cred").await;
        std::env::remove_var("AERINI_ENV_CRED_TEST_EMPTY");

        assert!(matches!(result, Err(CredentialResolveError::NotFound)));
    }
}
