use std::collections::HashMap;
use flowo_engine::executor::CredentialResolver;

/// Resolves workflow credentials from environment variables.
///
/// Naming convention: credential ID "openai-prod" maps to env var
/// FLOWO_CRED_OPENAI_PROD (uppercased, hyphens and dots replaced with underscores).
///
/// A missing env var is a hard failure with a descriptive error naming the
/// exact variable. There is no silent fallback — a missing credential causes
/// the workflow to fail immediately with a clear message.
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
        "FLOWO_CRED_{}",
        id.to_uppercase()
            .replace(['-', '.'], "_")
    )
}

#[async_trait::async_trait]
impl CredentialResolver for EnvCredentialResolver {
    async fn resolve(&self, credential_id: &str) -> Option<String> {
        let var_name = self.mapping
            .get(credential_id)
            .cloned()
            .unwrap_or_else(|| credential_id_to_env_var(credential_id));

        match std::env::var(&var_name) {
            Ok(val) if !val.is_empty() => Some(val),
            Ok(_) => {
                eprintln!(
                    "[credentials] WARNING: env var {} is set but empty for credential '{}'",
                    var_name, credential_id
                );
                None
            }
            Err(_) => {
                eprintln!(
                    "[credentials] ERROR: env var {} not set for credential '{}'",
                    var_name, credential_id
                );
                None
            }
        }
    }
}
