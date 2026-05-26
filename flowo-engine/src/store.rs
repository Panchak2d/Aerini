//! Credential store — AES-256-GCM encrypted values in SQLite.
//!
//! Moved from commands/credentials.rs so both the Tauri app and the server
//! binary can use it without any Tauri dependency.

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand::RngCore;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::EngineError;
use crate::executor::CredentialResolver;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialEntry {
    pub id:        String,
    pub name:      String,
    pub cred_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCredentialRequest {
    pub id:        String,
    pub name:      String,
    pub value:     String,
    pub cred_type: String,
}

/// How the AES-256 encryption key is sourced.
///
/// - `File(path)` — read/generate the key in a local file (server default).
/// - `OsKeychain { fallback }` — read/generate the key in the OS-native
///   credential store (macOS Keychain, Windows Credential Manager, Linux
///   SecretService). If the keychain is unavailable or fails, falls back to a
///   file at `fallback`. When `fallback` already contains a key and the
///   keychain has no entry, the key is automatically migrated into the keychain
///   and the file is deleted.
pub enum KeySource {
    File(PathBuf),
    OsKeychain { fallback: PathBuf },
}

pub struct CredentialStore {
    conn:   Mutex<Connection>,
    cipher: Aes256Gcm,
}

impl CredentialStore {
    pub fn open(db_path: &Path, key_source: KeySource) -> Result<Self, EngineError> {
        let key_bytes = Self::load_key(key_source)?;
        let key    = Key::<Aes256Gcm>::from_slice(&key_bytes);
        let cipher = Aes256Gcm::new(key);

        let conn = Connection::open(db_path)
            .map_err(|e| EngineError::Database(e.to_string()))?;

        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS credentials (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                cred_type   TEXT NOT NULL DEFAULT 'api_key',
                value_enc   BLOB NOT NULL,
                nonce       BLOB NOT NULL,
                created_at  TEXT NOT NULL
            );
        ").map_err(|e| EngineError::Database(e.to_string()))?;

        // Add cred_type column to existing databases that predate this field.
        // SQLite does not support ALTER TABLE ADD COLUMN IF NOT EXISTS, so we
        // attempt the migration and silently ignore the "duplicate column name" error.
        let _ = conn.execute(
            "ALTER TABLE credentials ADD COLUMN cred_type TEXT NOT NULL DEFAULT 'api_key'",
            [],
        );

        Ok(Self { conn: Mutex::new(conn), cipher })
    }

    pub fn store(&self, req: &CreateCredentialRequest) -> Result<(), EngineError> {
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let encrypted = self.cipher
            .encrypt(nonce, req.value.as_bytes())
            .map_err(|e| EngineError::Encryption(e.to_string()))?;

        let conn = self.conn.lock().expect("credential store mutex poisoned");
        conn.execute(
            "INSERT OR REPLACE INTO credentials (id, name, cred_type, value_enc, nonce, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                req.id, req.name, req.cred_type, encrypted, nonce_bytes.to_vec(),
                chrono::Utc::now().to_rfc3339(),
            ],
        ).map_err(|e| EngineError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn retrieve(&self, id: &str) -> Result<Option<String>, EngineError> {
        let conn = self.conn.lock().expect("credential store mutex poisoned");
        let result = conn.query_row(
            "SELECT value_enc, nonce FROM credentials WHERE id = ?1",
            params![id],
            |row| {
                let enc: Vec<u8>   = row.get(0)?;
                let nonce: Vec<u8> = row.get(1)?;
                Ok((enc, nonce))
            },
        );
        match result {
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(EngineError::Database(e.to_string())),
            Ok((enc, nonce_bytes)) => {
                let nonce = Nonce::from_slice(&nonce_bytes);
                let decrypted = self.cipher
                    .decrypt(nonce, enc.as_ref())
                    .map_err(|e| EngineError::Encryption(e.to_string()))?;
                Ok(Some(String::from_utf8(decrypted)
                    .map_err(|e| EngineError::Encryption(e.to_string()))?))
            }
        }
    }

    pub fn list(&self) -> Result<Vec<CredentialEntry>, EngineError> {
        let conn = self.conn.lock().expect("credential store mutex poisoned");
        let mut stmt = conn
            .prepare("SELECT id, name, cred_type FROM credentials ORDER BY name")
            .map_err(|e| EngineError::Database(e.to_string()))?;
        let entries = stmt.query_map([], |row| {
            Ok(CredentialEntry {
                id:        row.get(0)?,
                name:      row.get(1)?,
                cred_type: row.get(2)?,
            })
        }).map_err(|e| EngineError::Database(e.to_string()))?;
        entries.collect::<Result<Vec<_>, _>>()
            .map_err(|e| EngineError::Database(e.to_string()))
    }

    pub fn delete(&self, id: &str) -> Result<(), EngineError> {
        let conn = self.conn.lock().expect("credential store mutex poisoned");
        conn.execute("DELETE FROM credentials WHERE id = ?1", params![id])
            .map_err(|e| EngineError::Database(e.to_string()))?;
        Ok(())
    }

    // ── Key loading ───────────────────────────────────────────────────────────

    fn load_key(source: KeySource) -> Result<Vec<u8>, EngineError> {
        match source {
            KeySource::File(path) => Self::key_from_file(&path),
            KeySource::OsKeychain { fallback } => {
                Self::key_from_keychain(&fallback)
            }
        }
    }

    /// Load the AES key from the OS keychain.
    ///
    /// Decision tree:
    /// 1. Keychain has entry → decode + return.
    /// 2. No entry, fallback file exists → read file, write to keychain,
    ///    delete file (migration), return key.
    /// 3. No entry, no file → generate, write to keychain, return.
    /// 4. Any keychain error except NoEntry, or keychain write fails in step 3
    ///    → warn + fall back to file.
    fn key_from_keychain(fallback: &Path) -> Result<Vec<u8>, EngineError> {
        use keyring::{Entry, Error as KeyringError};

        const SERVICE: &str = "flowo";
        const USER: &str = "encryption_key";

        let entry = match Entry::new(SERVICE, USER) {
            Ok(e) => e,
            Err(e) => {
                eprintln!(
                    "[store] OS keychain init failed: {e}. \
                     Falling back to file at {}.",
                    fallback.display()
                );
                return Self::key_from_file(fallback);
            }
        };

        match entry.get_password() {
            Ok(encoded) => {
                let key_bytes = B64.decode(encoded.trim()).map_err(|e| {
                    EngineError::Encryption(format!(
                        "keychain key is corrupt (base64 decode failed): {e}"
                    ))
                })?;
                if key_bytes.len() != 32 {
                    return Err(EngineError::Encryption(format!(
                        "keychain key is corrupt: expected 32 bytes, got {}",
                        key_bytes.len()
                    )));
                }
                Ok(key_bytes)
            }

            Err(KeyringError::NoEntry) => {
                if fallback.exists() {
                    // Migrate key from legacy file into the keychain.
                    let key_bytes = Self::key_from_file(fallback)?;
                    let encoded = B64.encode(&key_bytes);
                    match entry.set_password(&encoded) {
                        Ok(()) => {
                            let _ = std::fs::remove_file(fallback);
                            eprintln!(
                                "[store] Encryption key migrated from file to OS keychain."
                            );
                        }
                        Err(e) => {
                            eprintln!(
                                "[store] Keychain write failed during migration: {e}. \
                                 Key remains in file."
                            );
                        }
                    }
                    Ok(key_bytes)
                } else {
                    // No existing key — generate a new one and store in keychain.
                    let mut key = [0u8; 32];
                    OsRng.fill_bytes(&mut key);
                    let encoded = B64.encode(key);
                    match entry.set_password(&encoded) {
                        Ok(()) => Ok(key.to_vec()),
                        Err(e) => {
                            eprintln!(
                                "[store] OS keychain unavailable: {e}. \
                                 Falling back to file at {}.",
                                fallback.display()
                            );
                            // key_from_file creates the key file if absent.
                            Self::key_from_file(fallback)
                        }
                    }
                }
            }

            Err(e) => {
                eprintln!(
                    "[store] OS keychain read failed: {e}. \
                     Falling back to file at {}.",
                    fallback.display()
                );
                Self::key_from_file(fallback)
            }
        }
    }

    fn key_from_file(key_path: &Path) -> Result<Vec<u8>, EngineError> {
        if key_path.exists() {
            let encoded = std::fs::read_to_string(key_path)
                .map_err(|e| EngineError::Encryption(e.to_string()))?;
            let key_bytes = B64.decode(encoded.trim())
                .map_err(|e| EngineError::Encryption(e.to_string()))?;
            if key_bytes.len() != 32 {
                return Err(EngineError::Encryption(format!(
                    "Key file is corrupt: expected 32 bytes after base64 decode, got {}",
                    key_bytes.len()
                )));
            }
            Ok(key_bytes)
        } else {
            let mut key = [0u8; 32];
            OsRng.fill_bytes(&mut key);
            let encoded = B64.encode(key);

            if let Some(parent) = key_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| EngineError::Encryption(e.to_string()))?;
            }
            std::fs::write(key_path, &encoded)
                .map_err(|e| EngineError::Encryption(e.to_string()))?;

            // Restrict to owner-only: prevents world-readable key file (default umask = 0644)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| EngineError::Encryption(e.to_string()))?;
            }

            Ok(key.to_vec())
        }
    }
}

pub struct StoreCredentialResolver {
    pub store: Arc<CredentialStore>,
}

#[async_trait::async_trait]
impl CredentialResolver for StoreCredentialResolver {
    async fn resolve(&self, credential_id: &str) -> Option<String> {
        self.store.retrieve(credential_id).ok().flatten()
    }
}
