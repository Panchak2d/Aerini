//! Credential store — AES-256-GCM encrypted values in SQLite, used by both the
//! Tauri app and the server binary.

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Key, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand_core::{OsRng, RngCore};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

use crate::error::EngineError;
use crate::executor::{CredentialResolveError, CredentialResolver, SecretString};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialEntry {
    pub id:        String,
    pub name:      String,
    pub cred_type: String,
}

/// Optional, non-secret metadata attached to a credential — used to auto-fill
/// a node's provider/model/base_url fields when that credential is selected.
/// Stored as a JSON blob alongside the encrypted secret (no encryption needed,
/// none of these values are secret).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CredentialMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model:    Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

impl CredentialMetadata {
    fn is_empty(&self) -> bool {
        self.provider.is_none() && self.model.is_none() && self.base_url.is_none()
    }
}

#[derive(Clone, Deserialize)]
pub struct CreateCredentialRequest {
    pub id:        String,
    pub name:      String,
    pub value:     String,
    pub cred_type: String,
    #[serde(default)]
    pub provider:  Option<String>,
    #[serde(default)]
    pub model:     Option<String>,
    #[serde(default)]
    pub base_url:  Option<String>,
}

impl std::fmt::Debug for CreateCredentialRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateCredentialRequest")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("value", &format_args!("<redacted>"))
            .field("cred_type", &self.cred_type)
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .finish()
    }
}

impl Drop for CreateCredentialRequest {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

pub const MAX_ID_LEN: usize = 128;
pub const MAX_NAME_CHARS: usize = 256;
pub const MAX_VALUE_BYTES: usize = 65_536;
const MAX_PROVIDER_CHARS: usize = 64;
const MAX_MODEL_CHARS: usize = 256;
const MAX_BASE_URL_CHARS: usize = 2048;
pub const CREDENTIAL_TYPES: [&str; 5] = ["api_key", "bearer", "basic", "oauth", "other"];

fn invalid(message: String) -> EngineError {
    EngineError::CredentialInvalid(message)
}

fn check_text(field: &str, value: &str, max_chars: usize) -> Result<(), EngineError> {
    if value.trim().is_empty() || value.chars().count() > max_chars {
        return Err(invalid(format!("{field} must be 1-{max_chars} characters")));
    }
    if value.chars().any(char::is_control) {
        return Err(invalid(format!("{field} must not contain control characters")));
    }
    Ok(())
}

impl CreateCredentialRequest {
    /// Checks every field without echoing any value into the error. An empty
    /// `value` is allowed on update, where it means "keep the stored secret",
    /// and on create only for provider `local` (a server that needs no key).
    /// The id charset is enforced on create only, so a stored id from an older
    /// build stays updatable.
    fn validate(&self, creating: bool) -> Result<(), EngineError> {
        let id_ok = !self.id.is_empty()
            && self.id.len() <= MAX_ID_LEN
            && self.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if creating && !id_ok {
            return Err(invalid(format!(
                "id must be 1-{MAX_ID_LEN} characters: letters, digits, '_' or '-'"
            )));
        }
        check_text("name", &self.name, MAX_NAME_CHARS)?;
        if self.value.len() > MAX_VALUE_BYTES {
            return Err(invalid(format!("value must be at most {MAX_VALUE_BYTES} bytes")));
        }
        if creating && self.value.is_empty() && self.provider.as_deref() != Some("local") {
            return Err(invalid("value must not be empty".to_string()));
        }
        if !CREDENTIAL_TYPES.contains(&self.cred_type.as_str()) {
            return Err(invalid(format!(
                "cred_type must be one of: {}",
                CREDENTIAL_TYPES.join(", ")
            )));
        }
        if let Some(v) = &self.provider { check_text("provider", v, MAX_PROVIDER_CHARS)?; }
        if let Some(v) = &self.model { check_text("model", v, MAX_MODEL_CHARS)?; }
        if let Some(v) = &self.base_url {
            check_text("base_url", v, MAX_BASE_URL_CHARS)?;
            let parsed = url::Url::parse(v)
                .map_err(|_| invalid("base_url must be a valid http(s) URL".to_string()))?;
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                return Err(invalid("base_url must be a valid http(s) URL".to_string()));
            }
            if !parsed.username().is_empty() || parsed.password().is_some() {
                return Err(invalid("base_url must not contain a username or password".to_string()));
            }
        }
        Ok(())
    }

    fn metadata_json(&self) -> Result<Option<String>, EngineError> {
        let meta = CredentialMetadata {
            provider: self.provider.clone(),
            model:    self.model.clone(),
            base_url: self.base_url.clone(),
        };
        if meta.is_empty() {
            return Ok(None);
        }
        serde_json::to_string(&meta)
            .map(Some)
            .map_err(|e| EngineError::Database(format!("failed to serialize credential metadata: {e}")))
    }
}

/// How the AES-256 encryption key is sourced.
///
/// - `File(path)` — read/generate the key in a local file (server default).
/// - `OsKeychain { fallback }` — read/generate the key in the OS-native
///   credential store (macOS Keychain, Windows Credential Manager, Linux
///   SecretService). The keychain entry belongs to the directory holding
///   `fallback`, so each data directory has its own key. If the keychain is
///   unavailable or fails, falls back to a file at `fallback`. When `fallback`
///   already contains a key and the keychain has no entry, the key is
///   automatically migrated into the keychain and the file is deleted.
///
/// A new key is only ever generated when the credential database holds no
/// saved credentials. If credentials exist and no key can be found,
/// `open` returns `EngineError::KeyUnavailable` instead of silently starting
/// over with a key that cannot decrypt them.
pub enum KeySource {
    File(PathBuf),
    OsKeychain { fallback: PathBuf },
}

/// `raw_key` and `cipher` are wiped on drop (`cipher` through aes-gcm's
/// `zeroize` feature). Copies the key schedule or a decrypted value leaves in
/// registers, the stack, SQLite's page cache or the allocator's freed pages
/// are outside this crate's control and are not wiped.
pub struct CredentialStore {
    conn:    Mutex<Connection>,
    cipher:  Aes256Gcm,
    raw_key: Zeroizing<Vec<u8>>,
    /// Ids whose move to the current format already failed once; keeps a
    /// read-only database from logging the same warning on every read.
    rewrite_failed: Mutex<HashSet<String>>,
}

/// Storage format of `credentials.value_enc`: 0 is the original layout with
/// no associated data, 1 binds the ciphertext to the row's id, type and
/// metadata.
const ENC_VERSION_CURRENT: i64 = 1;
const AAD_TAG_V1: &[u8] = b"aerini-cred-v1";
const KEY_CHECK_NAME: &str = "key_check";
const KEY_CHECK_AAD: &[u8] = b"aerini-key-check-v1";
const KEY_CHECK_PLAINTEXT: &[u8] = b"aerini-key-check";
const ROW_COLUMNS: &str = "id, cred_type, metadata, enc_version, value_enc, nonce";

/// Each field is length-prefixed so two different (id, type, metadata)
/// triples can never produce the same bytes.
fn v1_aad(id: &str, cred_type: &str, metadata: Option<&str>) -> Vec<u8> {
    let mut aad = Vec::with_capacity(AAD_TAG_V1.len() + 12 + id.len() + cred_type.len());
    aad.extend_from_slice(AAD_TAG_V1);
    for field in [id, cred_type, metadata.unwrap_or("")] {
        aad.extend_from_slice(&(field.len() as u32).to_be_bytes());
        aad.extend_from_slice(field.as_bytes());
    }
    aad
}

struct StoredRow {
    id:        String,
    cred_type: String,
    metadata:  Option<String>,
    version:   i64,
    enc:       Vec<u8>,
    nonce:     Vec<u8>,
}

impl StoredRow {
    fn from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id:        row.get(0)?,
            cred_type: row.get(1)?,
            metadata:  row.get(2)?,
            version:   row.get(3)?,
            enc:       row.get(4)?,
            nonce:     row.get(5)?,
        })
    }

    fn aad(&self) -> Option<Vec<u8>> {
        match self.version {
            0 => Some(Vec::new()),
            1 => Some(v1_aad(&self.id, &self.cred_type, self.metadata.as_deref())),
            _ => None,
        }
    }
}

fn nonce_array(nonce: &[u8], id: &str) -> Result<[u8; 12], EngineError> {
    <[u8; 12]>::try_from(nonce).map_err(|_| {
        EngineError::Encryption(format!(
            "credential '{id}' has a corrupt nonce: expected 12 bytes, got {}",
            nonce.len()
        ))
    })
}

fn open_row(cipher: &Aes256Gcm, row: &StoredRow) -> Result<Zeroizing<Vec<u8>>, EngineError> {
    let nonce = Nonce::from(nonce_array(&row.nonce, &row.id)?);
    let aad = row.aad().ok_or_else(|| {
        EngineError::Encryption(format!(
            "credential '{}' uses storage format {}, which this version cannot read",
            row.id, row.version
        ))
    })?;
    cipher
        .decrypt(&nonce, Payload { msg: &row.enc, aad: &aad })
        .map(Zeroizing::new)
        .map_err(|e| EngineError::Encryption(e.to_string()))
}

fn seal(cipher: &Aes256Gcm, plaintext: &[u8], aad: &[u8]) -> Result<(Vec<u8>, [u8; 12]), EngineError> {
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let encrypted = cipher
        .encrypt(&Nonce::from(nonce_bytes), Payload { msg: plaintext, aad })
        .map_err(|e| EngineError::Encryption(e.to_string()))?;
    Ok((encrypted, nonce_bytes))
}

impl CredentialStore {
    pub fn open(db_path: &Path, key_source: KeySource) -> Result<Self, EngineError> {
        create_db_file_private(db_path);
        let mut conn = Connection::open(db_path)
            .map_err(|e| EngineError::Database(e.to_string()))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| EngineError::Database(e.to_string()))?;
        conn.execute_batch("PRAGMA secure_delete = ON;")
            .map_err(|e| EngineError::Database(e.to_string()))?;

        Self::migrate_schema(&mut conn)?;
        restrict_db_file(db_path);

        let has_credentials = conn
            .query_row("SELECT EXISTS(SELECT 1 FROM credentials)", [], |row| row.get::<_, i64>(0))
            .map_err(|e| EngineError::Database(e.to_string()))?
            != 0;
        let probe = if has_credentials { KeyProbe::load(&conn) } else { None };

        let downgraded = keychain_downgraded(&key_source, KEYCHAIN_PERSISTENT);
        let key_source = persistent_source(key_source, KEYCHAIN_PERSISTENT);
        let key_parent = key_dir(&key_source).to_path_buf();
        if downgraded {
            if let KeySource::File(path) = &key_source {
                eprintln!(
                    "[store] This build has no native OS keychain, so the encryption key is kept in {}. \
                     Anyone who can read that file can decrypt the saved credentials; keep it out of backups you share.",
                    path.display()
                );
            }
        }
        let key_bytes = Self::load_key(key_source, has_credentials, probe.as_ref())?;
        sweep_stale_key_temp_files(&key_parent, STALE_KEY_TEMP_AGE);
        let key = Key::<Aes256Gcm>::try_from(key_bytes.as_slice())
            .map_err(|_| EngineError::Encryption(format!(
                "credential store key is corrupt: expected 32 bytes, got {}",
                key_bytes.len()
            )))?;
        let cipher = Aes256Gcm::new(&key);
        Self::verify_key(&conn, &cipher, has_credentials)?;

        Ok(Self {
            conn: Mutex::new(conn),
            cipher,
            raw_key: key_bytes,
            rewrite_failed: Mutex::new(HashSet::new()),
        })
    }

    /// Confirms `cipher` belongs to this database before any credential is
    /// read. An encrypted check value in `credential_meta` settles it. A
    /// store without one (created by an older build) gets it as soon as one
    /// stored credential decrypts; if none does, the store is opened without
    /// a check and each credential reports its own failure. A check value
    /// that fails while a credential still decrypts is rewritten, so one
    /// damaged row cannot lock the store. A mismatch is refused only when no
    /// credential decrypts either, and an empty store simply takes the key.
    fn verify_key(conn: &Connection, cipher: &Aes256Gcm, has_credentials: bool) -> Result<(), EngineError> {
        let check = Self::read_key_check(conn)?;
        if let Some((enc, nonce)) = &check {
            if key_check_accepts(cipher, enc, nonce) {
                return Ok(());
            }
        }
        if has_credentials {
            if Self::any_row_decrypts(conn, cipher)? {
                return Self::write_key_check(conn, cipher);
            }
            if check.is_some() {
                return Err(EngineError::KeyMismatch(
                    "the key does not decrypt the saved credentials".to_string(),
                ));
            }
            tracing::warn!("no saved credential decrypts with the current key; key check not recorded");
            return Ok(());
        }
        Self::write_key_check(conn, cipher)
    }

    fn read_key_check(conn: &Connection) -> Result<Option<KeyCheckRow>, EngineError> {
        match conn.query_row(
            "SELECT value_enc, nonce FROM credential_meta WHERE name = ?1",
            params![KEY_CHECK_NAME],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ) {
            Ok(found) => Ok(Some(found)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(EngineError::Database(e.to_string())),
        }
    }

    fn write_key_check(conn: &Connection, cipher: &Aes256Gcm) -> Result<(), EngineError> {
        let (enc, nonce) = seal(cipher, KEY_CHECK_PLAINTEXT, KEY_CHECK_AAD)?;
        conn.execute(
            "INSERT OR REPLACE INTO credential_meta (name, value_enc, nonce) VALUES (?1, ?2, ?3)",
            params![KEY_CHECK_NAME, enc, nonce.to_vec()],
        )
        .map_err(|e| EngineError::Database(e.to_string()))?;
        Ok(())
    }

    fn any_row_decrypts(conn: &Connection, cipher: &Aes256Gcm) -> Result<bool, EngineError> {
        let mut stmt = conn
            .prepare(&format!("SELECT {ROW_COLUMNS} FROM credentials"))
            .map_err(|e| EngineError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], StoredRow::from_sql)
            .map_err(|e| EngineError::Database(e.to_string()))?;
        for row in rows.flatten() {
            if open_row(cipher, &row).is_ok() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Replaces the stored key with `encoded`, a key from `export_key_base64`,
    /// after checking that it decrypts this database. Works on a store that
    /// `open` refuses (`KeyUnavailable`, `KeyMismatch`), which is the case
    /// this exists for. A key that does not decrypt the store changes
    /// nothing. A store with no credential and no check value has nothing to
    /// check a key against and is refused.
    pub fn restore_key_base64(
        db_path: &Path,
        key_source: KeySource,
        encoded: &str,
    ) -> Result<(), EngineError> {
        let key_source = persistent_source(key_source, KEYCHAIN_PERSISTENT);
        let key = Self::decode_keychain_key(encoded)?;
        if !db_path.exists() {
            return Err(EngineError::Database(format!(
                "credential database {} does not exist",
                db_path.display()
            )));
        }
        let mut conn = Connection::open(db_path)
            .map_err(|e| EngineError::Database(e.to_string()))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| EngineError::Database(e.to_string()))?;
        Self::migrate_schema(&mut conn)?;
        let probe = KeyProbe::load(&conn).ok_or_else(|| {
            EngineError::CredentialInvalid(
                "the credential store holds nothing to check a key against".to_string(),
            )
        })?;
        drop(conn);
        if !probe.accepts(&key) {
            return Err(EngineError::KeyMismatch(
                "the key does not decrypt the saved credentials; nothing was changed".to_string(),
            ));
        }
        let normalized = Zeroizing::new(B64.encode(key.as_slice()));
        match key_source {
            KeySource::File(path) => {
                let _guard = lock_beside(&path).map_err(lock_error)?;
                publish_secret_file(&path, normalized.as_bytes())
                    .map_err(|e| EngineError::Encryption(e.to_string()))
            }
            KeySource::OsKeychain { fallback } => {
                let user = keychain_user(&fallback);
                let entry = keyring::Entry::new(KEYCHAIN_SERVICE, &user)
                    .map_err(|e| EngineError::Encryption(format!("OS keychain init failed: {e}")))?;
                Self::restore_into_slot(&entry, &fallback, &normalized)
            }
        }
    }

    fn restore_into_slot(slot: &dyn KeychainSlot, fallback: &Path, encoded: &str) -> Result<(), EngineError> {
        let _guard = lock_beside(fallback).map_err(lock_error)?;
        Self::write_keychain_key(slot, encoded)
            .map_err(|e| EngineError::Encryption(format!("OS keychain write failed: {e}")))?;
        if fallback.exists() {
            wipe_and_remove(fallback).map_err(|e| {
                EngineError::Encryption(format!(
                    "the key was restored to the OS keychain, but the old key file {} could not be removed: {e}",
                    fallback.display()
                ))
            })?;
        }
        Ok(())
    }

    /// Returns the raw AES-256 encryption key that decrypts every credential
    /// in this store, base64-encoded — the same format already written to
    /// disk by `key_from_file` and to the OS keychain by `key_from_keychain`.
    ///
    /// This is the only recovery path if the OS keychain entry is ever lost
    /// (reset, migrated to a new machine, SecretService unavailable, ...)
    /// with no surviving fallback file: without a copy of this value saved
    /// somewhere else, every credential encrypted under it becomes
    /// permanently undecryptable.
    ///
    /// To restore from a backup, pass this string to
    /// `CredentialStore::restore_key_base64`, which checks it against the
    /// stored credentials before it replaces the key. Writing it to the key
    /// file by hand also works when no keychain entry exists.
    ///
    /// Anyone holding this value can decrypt every credential in this store.
    /// Treat it with at least as much care as the credentials themselves.
    pub fn export_key_base64(&self) -> SecretString {
        Zeroizing::new(B64.encode(self.raw_key.as_slice()))
    }

    /// Saves a new credential. Fails with `CredentialExists` when the id is
    /// taken and `CredentialInvalid` when any field is out of bounds.
    pub fn create(&self, req: &CreateCredentialRequest) -> Result<(), EngineError> {
        req.validate(true)?;
        let metadata_json = req.metadata_json()?;
        let aad = v1_aad(&req.id, &req.cred_type, metadata_json.as_deref());
        let (encrypted, nonce_bytes) = seal(&self.cipher, req.value.as_bytes(), &aad)?;

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO credentials (id, name, cred_type, value_enc, nonce, created_at, metadata, enc_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                req.id, req.name, req.cred_type, encrypted, nonce_bytes.to_vec(),
                chrono::Utc::now().to_rfc3339(), metadata_json, ENC_VERSION_CURRENT,
            ],
        ).map_err(|e| match e {
            rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
                EngineError::CredentialExists(req.id.clone())
            }
            other => EngineError::Database(other.to_string()),
        })?;
        Ok(())
    }

    /// Replaces name, type and metadata of an existing credential and keeps its
    /// `created_at`. An empty `value` keeps the stored secret; a non-empty one
    /// replaces it. Fails with `CredentialNotFound` for an unknown id. The
    /// stored secret is bound to the type and metadata, so changing either
    /// re-encrypts it, which needs the old secret to be readable.
    pub fn update(&self, req: &CreateCredentialRequest) -> Result<(), EngineError> {
        req.validate(false)?;
        let metadata_json = req.metadata_json()?;

        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| EngineError::Database(e.to_string()))?;
        let stored = match tx.query_row(
            &format!("SELECT {ROW_COLUMNS} FROM credentials WHERE id = ?1"),
            params![req.id],
            StoredRow::from_sql,
        ) {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(EngineError::CredentialNotFound(req.id.clone()));
            }
            Err(e) => return Err(EngineError::Database(e.to_string())),
        };

        let binding_unchanged = stored.cred_type == req.cred_type && stored.metadata == metadata_json;
        if req.value.is_empty() && binding_unchanged {
            tx.execute(
                "UPDATE credentials SET name = ?2 WHERE id = ?1",
                params![req.id, req.name],
            )
            .map_err(|e| EngineError::Database(e.to_string()))?;
        } else {
            let plaintext = if req.value.is_empty() {
                open_row(&self.cipher, &stored).map_err(|_| {
                    EngineError::CredentialInvalid(
                        "the stored secret cannot be read, so enter a new value".to_string(),
                    )
                })?
            } else {
                Zeroizing::new(req.value.as_bytes().to_vec())
            };
            let aad = v1_aad(&req.id, &req.cred_type, metadata_json.as_deref());
            let (encrypted, nonce_bytes) = seal(&self.cipher, plaintext.as_slice(), &aad)?;
            tx.execute(
                "UPDATE credentials
                 SET name = ?2, cred_type = ?3, metadata = ?4, value_enc = ?5, nonce = ?6, enc_version = ?7
                 WHERE id = ?1",
                params![
                    req.id, req.name, req.cred_type, metadata_json,
                    encrypted, nonce_bytes.to_vec(), ENC_VERSION_CURRENT,
                ],
            )
            .map_err(|e| EngineError::Database(e.to_string()))?;
        }
        tx.commit().map_err(|e| EngineError::Database(e.to_string()))
    }

    /// Returns the decrypted secret. A credential stored in the original
    /// format is rewritten in the current one after it decrypts.
    pub fn retrieve(&self, id: &str) -> Result<Option<SecretString>, EngineError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let row = match conn.query_row(
            &format!("SELECT {ROW_COLUMNS} FROM credentials WHERE id = ?1"),
            params![id],
            StoredRow::from_sql,
        ) {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(EngineError::Database(e.to_string())),
        };
        let plaintext = open_row(&self.cipher, &row)?;
        let secret = std::str::from_utf8(plaintext.as_slice())
            .map_err(|e| EngineError::Encryption(format!("credential '{id}' is not valid UTF-8: {e}")))?;
        let secret = Zeroizing::new(secret.to_owned());
        if row.version != ENC_VERSION_CURRENT {
            if let Err(e) = self.rewrite_current_format(&conn, &row, plaintext.as_slice()) {
                if self.first_rewrite_failure(id) {
                    tracing::warn!(credential = %id, "could not rewrite credential in the current storage format: {e}");
                }
            }
        }
        Ok(Some(secret))
    }

    fn first_rewrite_failure(&self, id: &str) -> bool {
        self.rewrite_failed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string())
    }

    fn rewrite_current_format(
        &self,
        conn: &Connection,
        row: &StoredRow,
        plaintext: &[u8],
    ) -> Result<(), EngineError> {
        let aad = v1_aad(&row.id, &row.cred_type, row.metadata.as_deref());
        let (encrypted, nonce_bytes) = seal(&self.cipher, plaintext, &aad)?;
        conn.execute(
            "UPDATE credentials SET value_enc = ?2, nonce = ?3, enc_version = ?4
             WHERE id = ?1 AND enc_version = ?5 AND value_enc = ?6",
            params![row.id, encrypted, nonce_bytes.to_vec(), ENC_VERSION_CURRENT, row.version, row.enc],
        )
        .map_err(|e| EngineError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<CredentialEntry>, EngineError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
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

    /// Returns non-secret metadata (provider/model/base_url) for a credential.
    /// `Ok(None)` for: id not found, or a credential saved before this field
    /// existed (metadata column is NULL).
    pub fn get_metadata(&self, id: &str) -> Result<Option<CredentialMetadata>, EngineError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let result: Result<Option<String>, rusqlite::Error> = conn.query_row(
            "SELECT metadata FROM credentials WHERE id = ?1",
            params![id],
            |row| row.get(0),
        );
        match result {
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(EngineError::Database(e.to_string())),
            Ok(None) => Ok(None),
            Ok(Some(json_str)) => serde_json::from_str(&json_str)
                .map(Some)
                .map_err(|e| EngineError::Database(format!("corrupt credential metadata for '{id}': {e}"))),
        }
    }

    pub fn delete(&self, id: &str) -> Result<(), EngineError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let removed = conn.execute("DELETE FROM credentials WHERE id = ?1", params![id])
            .map_err(|e| EngineError::Database(e.to_string()))?;
        if removed == 0 {
            return Err(EngineError::CredentialNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Creates the table and adds any column an older database lacks, inside
    /// one write transaction so two processes opening the same legacy
    /// database cannot both attempt the same `ALTER TABLE`.
    fn migrate_schema(conn: &mut Connection) -> Result<(), EngineError> {
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| EngineError::Database(e.to_string()))?;
        tx.execute_batch("
            CREATE TABLE IF NOT EXISTS credentials (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                cred_type   TEXT NOT NULL DEFAULT 'api_key',
                value_enc   BLOB NOT NULL,
                nonce       BLOB NOT NULL,
                created_at  TEXT NOT NULL,
                metadata    TEXT,
                enc_version INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS credential_meta (
                name      TEXT PRIMARY KEY,
                value_enc BLOB NOT NULL,
                nonce     BLOB NOT NULL
            );
        ").map_err(|e| EngineError::Database(e.to_string()))?;
        Self::ensure_column(&tx, "cred_type", "TEXT NOT NULL DEFAULT 'api_key'")?;
        Self::ensure_column(&tx, "metadata", "TEXT")?;
        Self::ensure_column(&tx, "enc_version", "INTEGER NOT NULL DEFAULT 0")?;
        tx.commit().map_err(|e| EngineError::Database(e.to_string()))
    }
    /// Adds `column` to `credentials` when an older database lacks it. Any
    /// failure other than the column already existing is returned, not hidden.
    fn ensure_column(conn: &Connection, column: &str, definition: &str) -> Result<(), EngineError> {
        let present: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('credentials') WHERE name = ?1)",
                params![column],
                |row| row.get(0),
            )
            .map_err(|e| EngineError::Database(e.to_string()))?;
        if !present {
            conn.execute(
                &format!("ALTER TABLE credentials ADD COLUMN {column} {definition}"),
                [],
            )
            .map_err(|e| EngineError::Database(e.to_string()))?;
        }
        Ok(())
    }

    // ── Key loading ───────────────────────────────────────────────────────────

    fn load_key(
        source: KeySource,
        has_credentials: bool,
        probe: Option<&KeyProbe>,
    ) -> Result<Zeroizing<Vec<u8>>, EngineError> {
        match source {
            KeySource::File(path) => Self::key_from_file(&path, has_credentials),
            KeySource::OsKeychain { fallback } => {
                let user = keychain_user(&fallback);
                let scoped = match keyring::Entry::new(KEYCHAIN_SERVICE, &user) {
                    Ok(entry) => entry,
                    Err(e) => return Self::key_from_file_after_keychain_failure(
                        &fallback, has_credentials, &format!("OS keychain init failed: {e}"), false,
                    ),
                };
                let legacy = keyring::Entry::new(KEYCHAIN_SERVICE, LEGACY_KEYCHAIN_USER).ok();
                Self::key_from_slots(
                    &scoped,
                    legacy.as_ref().map(|e| e as &dyn KeychainSlot),
                    &fallback,
                    has_credentials,
                    probe,
                )
            }
        }
    }

    /// Load the AES key from the OS keychain.
    ///
    /// The keychain entry is specific to the directory holding `fallback`, so
    /// separate data directories (and the desktop app next to a server) never
    /// share or overwrite one key. Every step that can create or move a key
    /// runs under an exclusive lock file next to `fallback`.
    ///
    /// Decision tree:
    /// 1. This directory's entry exists → decode it. A fallback file holding
    ///    the same key is removed; one holding a different key is refused when
    ///    credentials exist.
    /// 2. No entry, fallback file exists → copy its key into the keychain,
    ///    read it back, delete the file only once the read-back matches.
    /// 3. No entry, no file, credentials exist, and the pre-scoping shared
    ///    entry holds a key that decrypts them → copy that key into this
    ///    directory's entry. The shared entry is left in place.
    /// 4. No entry, no file, credentials exist otherwise → `KeyUnavailable`.
    /// 5. No entry, no file, no credentials → generate, write, and use
    ///    whatever the keychain then returns. If the keychain cannot serve the
    ///    value back, the same key is saved to the fallback file instead.
    /// 6. Any other keychain failure (access denied, locked, backend down, or
    ///    a failed write in step 5) → use the fallback file if present,
    ///    generate one only when there are no saved credentials, otherwise
    ///    `KeyUnavailable`.
    fn key_from_slots(
        scoped: &dyn KeychainSlot,
        legacy: Option<&dyn KeychainSlot>,
        fallback: &Path,
        has_credentials: bool,
        probe: Option<&KeyProbe>,
    ) -> Result<Zeroizing<Vec<u8>>, EngineError> {
        match scoped.read() {
            Ok(Some(encoded)) => Self::reconciled_keychain_key(&encoded, fallback, has_credentials),
            Ok(None) => {
                let _guard = lock_beside(fallback).map_err(lock_error)?;
                match scoped.read() {
                    Ok(Some(encoded)) => {
                        Self::reconciled_keychain_key(&encoded, fallback, has_credentials)
                    }
                    Ok(None) => Self::provision_keychain_key(
                        scoped, legacy, fallback, has_credentials, probe,
                    ),
                    Err(e) => Self::key_from_file_after_keychain_failure(
                        fallback, has_credentials, &format!("OS keychain read failed: {e}"), true,
                    ),
                }
            }
            Err(e) => Self::key_from_file_after_keychain_failure(
                fallback, has_credentials, &format!("OS keychain read failed: {e}"), false,
            ),
        }
    }

    /// Runs with the lock held and this directory's keychain entry empty.
    fn provision_keychain_key(
        scoped: &dyn KeychainSlot,
        legacy: Option<&dyn KeychainSlot>,
        fallback: &Path,
        has_credentials: bool,
        probe: Option<&KeyProbe>,
    ) -> Result<Zeroizing<Vec<u8>>, EngineError> {
        if fallback.exists() {
            let key_bytes = Self::key_from_file_with_lock(fallback, has_credentials, true)?;
            let encoded = Zeroizing::new(B64.encode(key_bytes.as_slice()));
            match Self::write_keychain_key(scoped, &encoded) {
                Ok(()) => match wipe_and_remove(fallback) {
                    Ok(()) => eprintln!("[store] Encryption key migrated from file to OS keychain."),
                    Err(e) => eprintln!(
                        "[store] Encryption key copied to the OS keychain, but the key file {} \
                         could not be removed: {e}. Remove it manually.",
                        fallback.display()
                    ),
                },
                Err(e) => eprintln!(
                    "[store] Keychain write failed during migration: {e}. Key remains in file."
                ),
            }
            return Ok(key_bytes);
        }

        if has_credentials {
            let Some(shared) = Self::shared_keychain_key(legacy) else {
                return Err(EngineError::KeyUnavailable(
                    "saved credentials exist but the OS keychain has no encryption key \
                     for this data directory and no backup key file was found"
                        .to_string(),
                ));
            };
            if !probe.is_none_or(|p| p.accepts(&shared)) {
                return Err(EngineError::KeyUnavailable(
                    "saved credentials exist but the OS keychain has no encryption key \
                     for this data directory, and the older shared keychain key does not \
                     decrypt them"
                        .to_string(),
                ));
            }
            let encoded = Zeroizing::new(B64.encode(shared.as_slice()));
            match Self::write_keychain_key(scoped, &encoded) {
                Ok(()) => eprintln!(
                    "[store] Encryption key copied from the shared OS keychain entry to this \
                     data directory's entry."
                ),
                Err(e) => eprintln!(
                    "[store] Could not copy the shared keychain key to this data directory's \
                     entry: {e}. Using the shared key for this run."
                ),
            }
            return Ok(shared);
        }

        let mut key = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(&mut *key);
        let encoded = Zeroizing::new(B64.encode(&key[..]));
        match scoped.write(&encoded) {
            Ok(()) => match scoped.read() {
                Ok(Some(stored)) => Self::decode_keychain_key(&stored),
                Ok(None) | Err(_) => {
                    publish_secret_file(fallback, encoded.as_bytes())
                        .map_err(|e| EngineError::Encryption(e.to_string()))?;
                    eprintln!(
                        "[store] OS keychain write could not be read back. Key saved to {}.",
                        fallback.display()
                    );
                    Ok(Zeroizing::new(key.to_vec()))
                }
            },
            Err(e) => Self::key_from_file_after_keychain_failure(
                fallback, has_credentials, &format!("OS keychain unavailable: {e}"), true,
            ),
        }
    }

    fn shared_keychain_key(legacy: Option<&dyn KeychainSlot>) -> Option<Zeroizing<Vec<u8>>> {
        let encoded = legacy?.read().ok().flatten()?;
        Self::decode_keychain_key(&encoded).ok()
    }

    /// Decodes the keychain's key and settles any fallback file beside it. A
    /// file holding the same key is a leftover and is removed. A file holding a
    /// different key means the two stores disagree; that is an error once
    /// credentials exist, because either key may be the one that encrypted
    /// them.
    fn reconciled_keychain_key(
        encoded: &str,
        fallback: &Path,
        has_credentials: bool,
    ) -> Result<Zeroizing<Vec<u8>>, EngineError> {
        let key = Self::decode_keychain_key(encoded)?;
        if !fallback.exists() {
            return Ok(key);
        }
        match Self::read_key_file(fallback) {
            Ok(file_key) if bool::from(file_key.ct_eq(&key)) => {
                match wipe_and_remove(fallback) {
                    Ok(()) => eprintln!(
                        "[store] Removed leftover key file {}: the OS keychain holds the same key.",
                        fallback.display()
                    ),
                    Err(e) => eprintln!(
                        "[store] Key file {} duplicates the OS keychain key and could not be \
                         removed: {e}.",
                        fallback.display()
                    ),
                }
            }
            Ok(_) if has_credentials => {
                return Err(EngineError::KeyUnavailable(format!(
                    "the OS keychain key and the key file {} differ, and saved credentials \
                     exist. Remove the key file if the keychain key is the right one, or \
                     remove the keychain entry if the file is",
                    fallback.display()
                )));
            }
            Ok(_) => eprintln!(
                "[store] Key file {} differs from the OS keychain key. Using the keychain key.",
                fallback.display()
            ),
            Err(e) => eprintln!(
                "[store] Ignoring unreadable key file {}: {e}",
                fallback.display()
            ),
        }
        Ok(key)
    }

    fn decode_keychain_key(encoded: &str) -> Result<Zeroizing<Vec<u8>>, EngineError> {
        let key_bytes = Zeroizing::new(B64.decode(encoded.trim()).map_err(|e| {
            EngineError::Encryption(format!(
                "keychain key is corrupt (base64 decode failed): {e}"
            ))
        })?);
        if key_bytes.len() != 32 {
            return Err(EngineError::Encryption(format!(
                "keychain key is corrupt: expected 32 bytes, got {}",
                key_bytes.len()
            )));
        }
        Ok(key_bytes)
    }

    /// Stores `encoded` in the keychain and confirms the keychain returns the
    /// same value, so a backend that accepts a write but cannot serve it back
    /// is caught before the only other copy of the key is deleted.
    fn write_keychain_key(slot: &dyn KeychainSlot, encoded: &str) -> Result<(), String> {
        slot.write(encoded)?;
        match slot.read().map_err(|e| format!("read-back failed: {e}"))? {
            Some(stored) if stored.trim() == encoded => Ok(()),
            Some(_) => Err("keychain returned a different value than was written".to_string()),
            None => Err("read-back failed: the entry is empty".to_string()),
        }
    }

    fn key_from_file_after_keychain_failure(
        fallback: &Path,
        has_credentials: bool,
        reason: &str,
        lock_held: bool,
    ) -> Result<Zeroizing<Vec<u8>>, EngineError> {
        eprintln!(
            "[store] {reason}. Falling back to file at {}.",
            fallback.display()
        );
        Self::key_from_file_with_lock(fallback, has_credentials, lock_held).map_err(|e| match e {
            EngineError::KeyUnavailable(_) => EngineError::KeyUnavailable(format!(
                "{reason}, and no backup key file was found"
            )),
            other => other,
        })
    }

    fn key_from_file(key_path: &Path, has_credentials: bool) -> Result<Zeroizing<Vec<u8>>, EngineError> {
        Self::key_from_file_with_lock(key_path, has_credentials, false)
    }

    /// `lock_held` is true when the caller already holds the lock beside
    /// `key_path`; taking it again from the same process would deadlock.
    fn key_from_file_with_lock(
        key_path: &Path,
        has_credentials: bool,
        lock_held: bool,
    ) -> Result<Zeroizing<Vec<u8>>, EngineError> {
        if key_path.exists() {
            return Self::read_key_file(key_path);
        }
        if has_credentials {
            return Err(EngineError::KeyUnavailable(format!(
                "saved credentials exist but the key file {} is missing",
                key_path.display()
            )));
        }
        let _guard = if lock_held {
            None
        } else {
            lock_beside(key_path).map_err(lock_error)?
        };
        if key_path.exists() {
            return Self::read_key_file(key_path);
        }

        let mut key = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(&mut *key);
        let encoded = Zeroizing::new(B64.encode(&key[..]));
        publish_secret_file(key_path, encoded.as_bytes())
            .map_err(|e| EngineError::Encryption(e.to_string()))?;
        Ok(Zeroizing::new(key.to_vec()))
    }

    fn read_key_file(key_path: &Path) -> Result<Zeroizing<Vec<u8>>, EngineError> {
        tighten_key_file_permissions(key_path);
        let encoded = Zeroizing::new(
            std::fs::read_to_string(key_path)
                .map_err(|e| EngineError::Encryption(e.to_string()))?,
        );
        let key_bytes = Zeroizing::new(
            B64.decode(encoded.trim()).map_err(|e| EngineError::Encryption(e.to_string()))?,
        );
        if key_bytes.len() != 32 {
            return Err(EngineError::Encryption(format!(
                "Key file is corrupt: expected 32 bytes after base64 decode, got {}",
                key_bytes.len()
            )));
        }
        Ok(key_bytes)
    }
}

/// keyring falls back to a process-lifetime in-memory store on any target
/// without one of the native backends enabled in Cargo.toml. A key kept there
/// is gone at exit, so such a build keeps the key in the file instead.
const KEYCHAIN_PERSISTENT: bool = cfg!(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
));

fn keychain_downgraded(source: &KeySource, keychain_persistent: bool) -> bool {
    matches!(source, KeySource::OsKeychain { .. }) && !keychain_persistent
}

fn key_dir(source: &KeySource) -> &Path {
    let (KeySource::File(path) | KeySource::OsKeychain { fallback: path }) = source;
    path.parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Age a leftover temporary key file must reach before it is removed; a live
/// `publish_secret_file` holds its file for milliseconds.
const STALE_KEY_TEMP_AGE: Duration = Duration::from_secs(600);
/// A published key is a short base64 string, so only small files qualify.
const KEY_TEMP_MAX_BYTES: u64 = 256;

/// Removes `.tmp*` files left in `dir` by a `publish_secret_file` that was
/// interrupted before its rename. Only small regular files at least
/// `min_age` old are touched. Best effort: failures are logged.
fn sweep_stale_key_temp_files(dir: &Path, min_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(".tmp") {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        let stale = kind.is_file()
            && meta.len() <= KEY_TEMP_MAX_BYTES
            && meta
                .modified()
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age >= min_age);
        if !stale {
            continue;
        }
        match wipe_and_remove(&entry.path()) {
            Ok(()) => removed += 1,
            Err(e) => eprintln!(
                "[store] Could not remove leftover temporary file {}: {e}",
                entry.path().display()
            ),
        }
    }
    removed
}

fn persistent_source(source: KeySource, keychain_persistent: bool) -> KeySource {
    match source {
        KeySource::OsKeychain { fallback } if !keychain_persistent => KeySource::File(fallback),
        other => other,
    }
}

/// Ciphertext and nonce of the stored key-check value.
type KeyCheckRow = (Vec<u8>, Vec<u8>);

const KEYCHAIN_SERVICE: &str = "aerini";
const LEGACY_KEYCHAIN_USER: &str = "encryption_key";
const KEY_LOCK_TIMEOUT: Duration = Duration::from_secs(60);

/// One keychain entry, reduced to what key provisioning needs. `Ok(None)`
/// means the entry does not exist.
trait KeychainSlot {
    fn read(&self) -> Result<Option<Zeroizing<String>>, String>;
    fn write(&self, encoded: &str) -> Result<(), String>;
}

impl KeychainSlot for keyring::Entry {
    fn read(&self) -> Result<Option<Zeroizing<String>>, String> {
        match self.get_password() {
            Ok(encoded) => Ok(Some(Zeroizing::new(encoded))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn write(&self, encoded: &str) -> Result<(), String> {
        self.set_password(encoded).map_err(|e| e.to_string())
    }
}

/// One stored ciphertext, used to check that a candidate key belongs to this
/// database before it is adopted. The key-check value is preferred; a
/// credential row serves a store that has none yet.
struct KeyProbe {
    enc:   Vec<u8>,
    nonce: [u8; 12],
    aad:   Vec<u8>,
}

impl KeyProbe {
    fn load(conn: &Connection) -> Option<Self> {
        Self::from_key_check(conn).or_else(|| Self::from_row(conn))
    }

    fn from_key_check(conn: &Connection) -> Option<Self> {
        let (enc, nonce): (Vec<u8>, Vec<u8>) = conn
            .query_row(
                "SELECT value_enc, nonce FROM credential_meta WHERE name = ?1",
                params![KEY_CHECK_NAME],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok()?;
        Some(Self { enc, nonce: <[u8; 12]>::try_from(nonce).ok()?, aad: KEY_CHECK_AAD.to_vec() })
    }

    fn from_row(conn: &Connection) -> Option<Self> {
        let row = conn
            .query_row(
                &format!(
                    "SELECT {ROW_COLUMNS} FROM credentials
                     WHERE length(nonce) = 12 AND length(value_enc) >= 16 LIMIT 1"
                ),
                [],
                StoredRow::from_sql,
            )
            .ok()?;
        let aad = row.aad()?;
        Some(Self { nonce: <[u8; 12]>::try_from(row.nonce).ok()?, enc: row.enc, aad })
    }

    fn accepts(&self, key: &[u8]) -> bool {
        let Ok(key) = Key::<Aes256Gcm>::try_from(key) else { return false };
        Aes256Gcm::new(&key)
            .decrypt(&Nonce::from(self.nonce), Payload { msg: &self.enc, aad: &self.aad })
            .map(|mut plain| plain.zeroize())
            .is_ok()
    }
}

fn key_check_accepts(cipher: &Aes256Gcm, enc: &[u8], nonce: &[u8]) -> bool {
    let Ok(nonce) = <[u8; 12]>::try_from(nonce) else { return false };
    cipher
        .decrypt(&Nonce::from(nonce), Payload { msg: enc, aad: KEY_CHECK_AAD })
        .is_ok_and(|plain| bool::from(plain.as_slice().ct_eq(KEY_CHECK_PLAINTEXT)))
}

/// The keychain account name for the data directory holding `fallback`: the
/// canonical directory path, hashed, so two directories never share an entry.
fn keychain_user(fallback: &Path) -> String {
    let dir = fallback
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let _ = create_private_dir_all(dir);
    let canonical = dir
        .canonicalize()
        .or_else(|_| std::path::absolute(dir))
        .unwrap_or_else(|_| dir.to_path_buf());
    let digest = blake3::hash(canonical.as_os_str().as_encoded_bytes());
    format!("{LEGACY_KEYCHAIN_USER}:{}", &digest.to_hex()[..16])
}

/// Exclusive advisory lock on `<path>.lock`, released on drop.
pub struct KeyLock {
    _file: std::fs::File,
}

/// Takes the lock beside `path`, waiting up to a minute for another process.
/// `Ok(None)` means locking is unavailable here (the lock file cannot be
/// created or the filesystem does not support locks); the caller proceeds
/// unlocked.
pub fn lock_beside(path: &Path) -> std::io::Result<Option<KeyLock>> {
    let mut lock_name = path.as_os_str().to_owned();
    lock_name.push(".lock");
    let lock_path = PathBuf::from(lock_name);
    if let Some(parent) = lock_path.parent() {
        let _ = create_private_dir_all(parent);
    }

    let mut opts = std::fs::OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = match opts.open(&lock_path) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("[store] Cannot create lock file {}: {e}. Continuing without it.", lock_path.display());
            return Ok(None);
        }
    };

    let deadline = Instant::now() + KEY_LOCK_TIMEOUT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(KeyLock { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("timed out waiting for {}", lock_path.display()),
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                eprintln!("[store] Cannot lock {}: {e}. Continuing without it.", lock_path.display());
                return Ok(None);
            }
        }
    }
}

fn lock_error(e: std::io::Error) -> EngineError {
    EngineError::Encryption(format!("could not lock the key file: {e}"))
}

/// Creates `dir` and any missing parents, owner-only (0o700) on Unix from the
/// moment each exists.
pub fn create_private_dir_all(dir: &Path) -> std::io::Result<()> {
    if dir.as_os_str().is_empty() || dir.is_dir() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Writes `contents` to `path` so the file is never visible empty or partial:
/// an owner-only temporary file in the same directory is written and synced,
/// then renamed over `path`, and the directory is synced where the platform
/// allows. Callers hold the lock from `lock_beside` and have checked that
/// `path` does not exist.
pub fn publish_secret_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    create_private_dir_all(dir)?;

    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    #[cfg(windows)]
    set_owner_only_acl(tmp.path()).map_err(std::io::Error::other)?;
    tmp.write_all(contents)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;

    #[cfg(unix)]
    if let Err(e) = std::fs::File::open(dir).and_then(|d| d.sync_all()) {
        eprintln!("[store] Could not sync directory {}: {e}", dir.display());
    }
    Ok(())
}

/// Overwrites `path` with zeros before unlinking it. Best effort: journaling
/// filesystems and SSDs may keep older copies of the blocks.
fn wipe_and_remove(path: &Path) -> std::io::Result<()> {
    use std::io::Write;

    if let Ok(meta) = std::fs::metadata(path) {
        if let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(path) {
            let zeros = vec![0u8; meta.len().min(4096) as usize];
            let _ = file.write_all(&zeros).and_then(|()| file.sync_all());
        }
    }
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Restricts an existing key file to its owner. Best effort: a file this
/// process does not own cannot be changed, so a failure is logged rather than
/// fatal.
pub fn tighten_key_file_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Ok(meta) = std::fs::metadata(path) else { return };
        if meta.permissions().mode() & 0o077 == 0 {
            return;
        }
        match std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            Ok(()) => eprintln!(
                "[store] Key file {} was readable by other users; restricted it to its owner.",
                path.display()
            ),
            Err(e) => eprintln!(
                "[store] Key file {} is readable by other users and could not be restricted: {e}",
                path.display()
            ),
        }
    }
    #[cfg(windows)]
    if let Err(e) = set_owner_only_acl(path) {
        eprintln!("[store] Could not restrict key file {} to its owner: {e}", path.display());
    }
}

/// Creates the credential database file owner-only (0o600) on Unix before
/// SQLite opens it, so it never exists at the process umask. SQLite gives its
/// journal and WAL files the same permissions as the database file.
fn create_db_file_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path);
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Restricts an existing credential database to its owner. Best effort,
/// logged on failure.
fn restrict_db_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            eprintln!("[store] Could not restrict {} to its owner: {e}", path.display());
        }
    }
    #[cfg(windows)]
    if let Err(e) = set_owner_only_acl(path) {
        eprintln!("[store] Could not restrict {} to its owner: {e}", path.display());
    }
}
/// Sets a DACL on `path` that grants full control to the file owner only,
/// mirroring Unix `chmod 0600`. Only compiled on Windows targets.
#[cfg(windows)]
fn set_owner_only_acl(path: &std::path::Path) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Authorization::{SetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        ACL, InitializeAcl, AddAccessAllowedAce, GetTokenInformation,
        TOKEN_USER, TokenUser, ACL_REVISION, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, CloseHandle};

    // Encode path as a null-terminated wide string.
    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0u16))
        .collect();

    unsafe {
        // Open the current process token to get the owner SID.
        let mut token: HANDLE = INVALID_HANDLE_VALUE;
        if windows_sys::Win32::System::Threading::OpenProcessToken(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            windows_sys::Win32::Security::TOKEN_QUERY,
            &mut token,
        ) == 0 {
            return Err(format!(
                "OpenProcessToken failed: {}",
                windows_sys::Win32::Foundation::GetLastError()
            ));
        }

        // Query token for the user SID.
        let mut needed: u32 = 0;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
        if needed == 0 {
            CloseHandle(token);
            return Err("GetTokenInformation returned zero size — unexpected security configuration".to_string());
        }
        // Vec<u64>, not Vec<u8>: TOKEN_USER holds a pointer, so the buffer it is
        // read through must be 8-byte aligned.
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        if GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr() as *mut _,
            needed,
            &mut needed,
        ) == 0 {
            CloseHandle(token);
            return Err(format!(
                "GetTokenInformation failed: {}",
                windows_sys::Win32::Foundation::GetLastError()
            ));
        }
        CloseHandle(token);

        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let sid = user.User.Sid;

        // Build a minimal ACL containing one allow-all ACE for the owner SID.
        // Typical SID + ACE overhead is well under 256 bytes.
        // Use Vec<u32> (not Vec<u8>) to guarantee 4-byte alignment for the ACL header.
        const ACL_BUF_BYTES: usize = 256;
        let mut acl_buf = vec![0u32; ACL_BUF_BYTES / 4];
        if InitializeAcl(acl_buf.as_mut_ptr() as *mut ACL, ACL_BUF_BYTES as u32, ACL_REVISION) == 0 {
            return Err(format!(
                "InitializeAcl failed: {}",
                windows_sys::Win32::Foundation::GetLastError()
            ));
        }
        if AddAccessAllowedAce(
            acl_buf.as_mut_ptr() as *mut ACL,
            ACL_REVISION,
            FILE_ALL_ACCESS,
            sid,
        ) == 0 {
            return Err(format!(
                "AddAccessAllowedAce failed: {}",
                windows_sys::Win32::Foundation::GetLastError()
            ));
        }

        // Apply the ACL to the file. PROTECTED_DACL_SECURITY_INFORMATION clears
        // inherited ACEs so only the explicit owner ACE remains.
        let rc = SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl_buf.as_mut_ptr() as *mut ACL,
            std::ptr::null_mut(),
        );
        if rc != 0 {
            return Err(format!("SetNamedSecurityInfoW failed: {}", rc));
        }
    }
    Ok(())
}

pub struct StoreCredentialResolver {
    pub store: Arc<CredentialStore>,
}

#[async_trait::async_trait]
impl CredentialResolver for StoreCredentialResolver {
    async fn resolve(&self, credential_id: &str) -> Result<SecretString, CredentialResolveError> {
        // `CredentialStore::retrieve` is a synchronous fn — it
        // acquires a blocking `std::sync::Mutex<rusqlite::Connection>` and
        // runs the query + AES-256-GCM decrypt inline. `resolve` is called
        // on every node execution that references a credential, so running
        // that synchronously here would hold the calling tokio worker
        // thread for the duration of the lock + query on every such call.
        // Offload to the blocking thread pool instead.
        let store         = Arc::clone(&self.store);
        let credential_id = credential_id.to_string();
        let joined = tokio::task::spawn_blocking(move || store.retrieve(&credential_id)).await;

        match joined {
            Ok(Ok(Some(secret))) => Ok(secret),
            Ok(Ok(None)) => Err(CredentialResolveError::NotFound),
            Ok(Err(EngineError::Encryption(msg))) => Err(CredentialResolveError::Unreadable(msg)),
            Ok(Err(EngineError::Database(msg))) => Err(CredentialResolveError::StoreError(msg)),
            Ok(Err(other)) => Err(CredentialResolveError::StoreError(other.to_string())),
            Err(join_err) => Err(CredentialResolveError::StoreError(join_err.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_source_becomes_the_key_file_only_where_keyring_is_in_memory() {
        let fallback = PathBuf::from("/data/aerini.key");
        let keychain = || KeySource::OsKeychain { fallback: fallback.clone() };
        assert!(matches!(persistent_source(keychain(), true), KeySource::OsKeychain { .. }));
        match persistent_source(keychain(), false) {
            KeySource::File(path) => assert_eq!(path, fallback),
            KeySource::OsKeychain { .. } => panic!("an in-memory keychain must not hold the key"),
        }
        assert!(matches!(persistent_source(KeySource::File(fallback.clone()), false), KeySource::File(_)));
        assert!(keychain_downgraded(&keychain(), false));
        assert!(!keychain_downgraded(&keychain(), true));
        assert!(!keychain_downgraded(&KeySource::File(fallback.clone()), false));
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        const { assert!(KEYCHAIN_PERSISTENT) };
    }

    #[test]
    fn leftover_temporary_key_files_are_removed_only_when_old_small_and_named_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let leftover = dir.path().join(".tmpAbC123");
        let large = dir.path().join(".tmpLarge");
        let key = dir.path().join("aerini.key");
        std::fs::write(&leftover, b"c2VjcmV0").unwrap();
        std::fs::write(&large, vec![b'x'; 1024]).unwrap();
        std::fs::write(&key, b"c2VjcmV0").unwrap();

        assert_eq!(sweep_stale_key_temp_files(dir.path(), Duration::from_secs(3600)), 0);
        assert!(leftover.exists(), "a fresh temporary file may belong to a live writer");

        assert_eq!(sweep_stale_key_temp_files(dir.path(), Duration::ZERO), 1);
        assert!(!leftover.exists());
        assert!(large.exists() && key.exists());
    }

    #[test]
    fn key_from_file_generates_and_reloads_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key.b64");

        let generated = CredentialStore::key_from_file(&key_path, false).unwrap();
        assert_eq!(generated.len(), 32, "generated key must be 32 bytes");

        let reloaded = CredentialStore::key_from_file(&key_path, false).unwrap();
        assert_eq!(
            generated, reloaded,
            "a second call against the same path must return the same key, not regenerate"
        );
    }

    #[test]
    fn key_from_file_refuses_to_generate_when_credentials_exist() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key.b64");

        let result = CredentialStore::key_from_file(&key_path, true);

        assert!(matches!(result, Err(EngineError::KeyUnavailable(_))));
        assert!(!key_path.exists(), "no key file may be created on refusal");
    }

    #[test]
    fn key_from_file_reuses_existing_key_when_credentials_exist() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key.b64");
        let original = CredentialStore::key_from_file(&key_path, false).unwrap();

        let reused = CredentialStore::key_from_file(&key_path, true).unwrap();

        assert_eq!(original, reused);
    }

    // The file holding this key must never be readable by anyone but the
    // owner, from the moment it exists on disk. Checking the mode bits
    // after creation can't directly prove there's no window where a
    // separate write-then-chmod sequence would leave it briefly world/group
    // readable (that requires a concurrent-read race harness this
    // environment can't run), but it does lock in the end state that
    // create-with-mode guarantees, so a future edit back to a two-step
    // write+chmod would still leave this test passing on final permission
    // bits alone — it isn't a substitute for a real TOCTOU race test.
    #[cfg(unix)]
    #[test]
    fn key_from_file_creates_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key.b64");

        CredentialStore::key_from_file(&key_path, false).unwrap();

        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
        // Mask to the permission bits only (mode() also carries file-type bits).
        assert_eq!(
            mode & 0o777,
            0o600,
            "key file must be created with exactly owner read/write (0o600), got {:o}",
            mode & 0o777
        );
    }

    #[cfg(unix)]
    #[test]
    fn existing_key_file_readable_by_others_is_restricted_to_owner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key.b64");
        let original = CredentialStore::key_from_file(&key_path, false).unwrap();
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let reloaded = CredentialStore::key_from_file(&key_path, true).unwrap();

        assert_eq!(original, reloaded, "tightening permissions must not change the key");
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "got {:o}", mode & 0o777);
    }

    #[cfg(unix)]
    #[test]
    fn credential_database_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("creds.sqlite");
        let key_path = dir.path().join("key.b64");

        let _store = CredentialStore::open(&db_path, KeySource::File(key_path)).unwrap();

        let mode = std::fs::metadata(&db_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "got {:o}", mode & 0o777);
    }

    #[test]
    fn legacy_table_without_newer_columns_is_upgraded_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("creds.sqlite");
        let key_path = dir.path().join("key.b64");
        CredentialStore::key_from_file(&key_path, false).unwrap();
        {
            let raw = Connection::open(&db_path).unwrap();
            raw.execute_batch(
                "CREATE TABLE credentials (
                     id TEXT PRIMARY KEY, name TEXT NOT NULL,
                     value_enc BLOB NOT NULL, nonce BLOB NOT NULL, created_at TEXT NOT NULL
                 );
                 INSERT INTO credentials VALUES ('old', 'Old', x'00', x'00', '2024-01-01T00:00:00Z');",
            ).unwrap();
        }

        let store = CredentialStore::open(&db_path, KeySource::File(key_path)).unwrap();

        let entries = store.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cred_type, "api_key");
        assert!(store.get_metadata("old").unwrap().is_none());
    }

    #[test]
    fn retrieve_with_corrupt_nonce_returns_error_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("creds.sqlite");
        let key_path = dir.path().join("key.b64");

        let store = CredentialStore::open(&db_path, KeySource::File(key_path)).unwrap();
        store.create(&CreateCredentialRequest {
            id: "cred1".to_string(),
            name: "Test Cred".to_string(),
            value: "secret-value".to_string(),
            cred_type: "api_key".to_string(),
            provider: None,
            model: None,
            base_url: None,
        }).unwrap();

        // Corrupt the stored nonce to the wrong length, simulating a corrupted
        // or tampered DB row — must not reach Nonce::from_slice, which panics
        // on a length mismatch instead of returning an error.
        let raw = Connection::open(&db_path).unwrap();
        raw.execute(
            "UPDATE credentials SET nonce = ?1 WHERE id = 'cred1'",
            params![vec![0u8; 5]],
        ).unwrap();
        drop(raw);

        let result = store.retrieve("cred1");
        assert!(result.is_err(), "corrupt nonce must return Err, not panic");
        assert!(
            result.unwrap_err().to_string().contains("corrupt nonce"),
            "error must identify the nonce as the corrupt field"
        );
    }

    #[test]
    fn exported_key_matches_the_key_file_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("creds.sqlite");
        let key_path = dir.path().join("key.b64");

        let store = CredentialStore::open(&db_path, KeySource::File(key_path.clone())).unwrap();
        let exported = store.export_key_base64();

        let on_disk = std::fs::read_to_string(&key_path).unwrap();
        assert_eq!(
            exported.trim(), on_disk.trim(),
            "export_key_base64 must return exactly what key_from_file wrote to disk"
        );
    }

    /// The key-backup recovery scenario, reproduced end-to-end: a
    /// credential is stored, its key is exported, the original key file is
    /// deleted (simulating a lost keychain entry with no surviving fallback
    /// file), the exported value is written back to that same path, and a
    /// fresh `CredentialStore::open` against it must decrypt the original
    /// credential. Backup-only coverage (asserting the string looks right)
    /// wouldn't catch a wrong byte order, encoding mismatch, or wrong field
    /// being exported — only actually restoring from it does.
    #[test]
    fn key_exported_before_loss_restores_access_after_loss() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("creds.sqlite");
        let key_path = dir.path().join("key.b64");

        let store = CredentialStore::open(&db_path, KeySource::File(key_path.clone())).unwrap();
        store.create(&CreateCredentialRequest {
            id: "cred1".to_string(),
            name: "Test Cred".to_string(),
            value: "irreplaceable-secret".to_string(),
            cred_type: "api_key".to_string(),
            provider: None,
            model: None,
            base_url: None,
        }).unwrap();
        let backed_up_key = store.export_key_base64();
        drop(store);

        // Simulate total key loss: the key file (and, in the real desktop
        // path, the OS keychain entry) is gone.
        std::fs::remove_file(&key_path).unwrap();
        let refused = CredentialStore::open(&db_path, KeySource::File(key_path.clone()));
        assert!(
            matches!(refused.err(), Some(EngineError::KeyUnavailable(_))),
            "opening a store that holds credentials without its key must be refused, \
             not answered with a freshly generated key"
        );
        assert!(
            !key_path.exists(),
            "a refused open must not leave a replacement key file behind"
        );

        // Restore: write the backed-up key back to the expected path.
        std::fs::write(&key_path, backed_up_key.as_bytes()).unwrap();

        let restored = CredentialStore::open(&db_path, KeySource::File(key_path)).unwrap();
        assert_eq!(
            stored_secret(&restored, "cred1"),
            Some("irreplaceable-secret".to_string()),
            "restoring the exported key must recover the original credential"
        );
    }


    #[tokio::test]
    async fn store_resolver_returns_secret_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("creds.sqlite");
        let key_path = dir.path().join("key.b64");
        let store = Arc::new(CredentialStore::open(&db_path, KeySource::File(key_path)).unwrap());
        store.create(&CreateCredentialRequest {
            id: "cred1".to_string(),
            name: "Test Cred".to_string(),
            value: "secret-value".to_string(),
            cred_type: "api_key".to_string(),
            provider: None,
            model: None,
            base_url: None,
        }).unwrap();

        let resolver = StoreCredentialResolver { store };
        assert_eq!(resolver.resolve("cred1").await.unwrap().as_str(), "secret-value");
    }

    #[tokio::test]
    async fn store_resolver_returns_not_found_for_missing_id() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("creds.sqlite");
        let key_path = dir.path().join("key.b64");
        let store = Arc::new(CredentialStore::open(&db_path, KeySource::File(key_path)).unwrap());

        let resolver = StoreCredentialResolver { store };
        assert!(matches!(
            resolver.resolve("does-not-exist").await,
            Err(CredentialResolveError::NotFound)
        ));
    }

    #[tokio::test]
    async fn store_resolver_returns_unreadable_for_corrupt_entry() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("creds.sqlite");
        let key_path = dir.path().join("key.b64");
        let store = Arc::new(CredentialStore::open(&db_path, KeySource::File(key_path)).unwrap());
        store.create(&CreateCredentialRequest {
            id: "cred1".to_string(),
            name: "Test Cred".to_string(),
            value: "secret-value".to_string(),
            cred_type: "api_key".to_string(),
            provider: None,
            model: None,
            base_url: None,
        }).unwrap();

        let raw = Connection::open(&db_path).unwrap();
        raw.execute(
            "UPDATE credentials SET nonce = ?1 WHERE id = 'cred1'",
            params![vec![0u8; 5]],
        ).unwrap();
        drop(raw);

        let resolver = StoreCredentialResolver { store };
        assert!(matches!(
            resolver.resolve("cred1").await,
            Err(CredentialResolveError::Unreadable(_))
        ));
    }

    #[test]
    fn concurrent_first_starts_agree_on_one_key_and_leave_no_temporary_files() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key.b64");

        let keys: Vec<Zeroizing<Vec<u8>>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| CredentialStore::key_from_file(&key_path, false).unwrap()))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        assert!(keys.iter().all(|k| k == &keys[0]), "every start must see the same key");
        assert_eq!(std::fs::read_to_string(&key_path).unwrap().trim().len(), 44);
        let extra: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "key.b64" && name != "key.b64.lock")
            .collect();
        assert!(extra.is_empty(), "unexpected files left behind: {extra:?}");
    }

    #[cfg(unix)]
    #[test]
    fn directories_created_for_the_key_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("a").join("b").join("key.b64");

        CredentialStore::key_from_file(&key_path, false).unwrap();

        for sub in [dir.path().join("a"), dir.path().join("a").join("b")] {
            let mode = std::fs::metadata(&sub).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{} is {:o}", sub.display(), mode & 0o777);
        }
    }

    struct FakeSlot {
        value:         Mutex<Option<String>>,
        serves_reads:  bool,
    }

    impl FakeSlot {
        fn empty() -> Self {
            Self { value: Mutex::new(None), serves_reads: true }
        }
        fn holding(key: &[u8]) -> Self {
            Self { value: Mutex::new(Some(B64.encode(key))), serves_reads: true }
        }
        fn stored(&self) -> Option<String> {
            self.value.lock().unwrap().clone()
        }
    }

    impl KeychainSlot for FakeSlot {
        fn read(&self) -> Result<Option<Zeroizing<String>>, String> {
            if !self.serves_reads {
                return Ok(None);
            }
            Ok(self.value.lock().unwrap().clone().map(Zeroizing::new))
        }
        fn write(&self, encoded: &str) -> Result<(), String> {
            *self.value.lock().unwrap() = Some(encoded.to_string());
            Ok(())
        }
    }

    fn probe_encrypted_with(key: &[u8; 32]) -> KeyProbe {
        let nonce = [7u8; 12];
        let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(*key));
        let enc = cipher.encrypt(&Nonce::from(nonce), b"secret".as_ref()).unwrap();
        KeyProbe { enc, nonce, aad: Vec::new() }
    }

    #[test]
    fn concurrent_first_starts_share_one_keychain_key() {
        let dir = tempfile::tempdir().unwrap();
        let fallback = dir.path().join(".cred.key");
        let slot = FakeSlot::empty();

        let keys: Vec<Zeroizing<Vec<u8>>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        CredentialStore::key_from_slots(&slot, None, &fallback, false, None).unwrap()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        assert!(keys.iter().all(|k| k == &keys[0]));
        assert_eq!(slot.stored().unwrap(), B64.encode(&keys[0]), "the keychain must hold the key every start used");
        assert!(!fallback.exists());
    }

    #[test]
    fn shared_keychain_key_is_adopted_only_when_it_decrypts_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let fallback = dir.path().join(".cred.key");
        let (key_a, key_b) = ([1u8; 32], [2u8; 32]);

        let legacy = FakeSlot::holding(&key_a);
        let scoped = FakeSlot::empty();
        let adopted = CredentialStore::key_from_slots(
            &scoped, Some(&legacy), &fallback, true, Some(&probe_encrypted_with(&key_a)),
        ).unwrap();
        assert_eq!(adopted.as_slice(), key_a.as_slice());
        assert_eq!(scoped.stored(), Some(B64.encode(key_a)));
        assert_eq!(legacy.stored(), Some(B64.encode(key_a)), "the shared entry must stay in place");

        let wrong_legacy = FakeSlot::holding(&key_b);
        let untouched = FakeSlot::empty();
        let refused = CredentialStore::key_from_slots(
            &untouched, Some(&wrong_legacy), &fallback, true, Some(&probe_encrypted_with(&key_a)),
        );
        assert!(matches!(refused, Err(EngineError::KeyUnavailable(_))));
        assert_eq!(untouched.stored(), None);
    }

    #[test]
    fn empty_store_gets_its_own_key_even_when_a_shared_key_exists() {
        let dir = tempfile::tempdir().unwrap();
        let fallback = dir.path().join(".cred.key");
        let legacy = FakeSlot::holding(&[1u8; 32]);
        let scoped = FakeSlot::empty();

        let key = CredentialStore::key_from_slots(&scoped, Some(&legacy), &fallback, false, None).unwrap();

        assert_ne!(key.as_slice(), [1u8; 32].as_slice());
        assert_eq!(scoped.stored(), Some(B64.encode(&key)));
    }

    #[test]
    fn file_key_moves_into_an_empty_keychain_and_the_file_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let fallback = dir.path().join(".cred.key");
        publish_secret_file(&fallback, B64.encode([3u8; 32]).as_bytes()).unwrap();
        let scoped = FakeSlot::empty();

        let key = CredentialStore::key_from_slots(&scoped, None, &fallback, true, None).unwrap();

        assert_eq!(key.as_slice(), [3u8; 32].as_slice());
        assert_eq!(scoped.stored(), Some(B64.encode([3u8; 32])));
        assert!(!fallback.exists());
    }

    #[test]
    fn key_file_beside_a_working_keychain_is_removed_when_equal_and_refused_when_different() {
        let dir = tempfile::tempdir().unwrap();
        let fallback = dir.path().join(".cred.key");
        let slot = FakeSlot::holding(&[1u8; 32]);

        publish_secret_file(&fallback, B64.encode([1u8; 32]).as_bytes()).unwrap();
        CredentialStore::key_from_slots(&slot, None, &fallback, true, None).unwrap();
        assert!(!fallback.exists(), "a duplicate of the keychain key is a leftover");

        publish_secret_file(&fallback, B64.encode([2u8; 32]).as_bytes()).unwrap();
        let refused = CredentialStore::key_from_slots(&slot, None, &fallback, true, None);
        assert!(matches!(refused, Err(EngineError::KeyUnavailable(_))));
        assert!(fallback.exists(), "a differing key file must not be deleted");

        let key = CredentialStore::key_from_slots(&slot, None, &fallback, false, None).unwrap();
        assert_eq!(key.as_slice(), [1u8; 32].as_slice(), "with nothing encrypted, the keychain key wins");
    }

    #[test]
    fn keychain_that_cannot_serve_a_written_key_back_gets_the_same_key_in_the_fallback_file() {
        let dir = tempfile::tempdir().unwrap();
        let fallback = dir.path().join(".cred.key");
        let slot = FakeSlot { value: Mutex::new(None), serves_reads: false };

        let key = CredentialStore::key_from_slots(&slot, None, &fallback, false, None).unwrap();

        assert_eq!(CredentialStore::read_key_file(&fallback).unwrap(), key);
        assert_eq!(slot.stored(), Some(B64.encode(&key)), "the entry and the file must hold one key");
    }

    #[test]
    fn keychain_user_is_stable_per_directory_and_differs_between_directories() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one");
        let two = dir.path().join("two");

        let first = keychain_user(&one.join(".cred.key"));
        assert_eq!(first, keychain_user(&one.join("other-name.key")));
        assert_ne!(first, keychain_user(&two.join(".cred.key")));
        assert!(first.starts_with("encryption_key:"));
    }

    #[test]
    fn retrieving_a_legacy_row_with_a_placeholder_nonce_reports_it_as_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("creds.sqlite");
        let key_path = dir.path().join("key.b64");
        CredentialStore::key_from_file(&key_path, false).unwrap();
        {
            let raw = Connection::open(&db_path).unwrap();
            raw.execute_batch(
                "CREATE TABLE credentials (
                     id TEXT PRIMARY KEY, name TEXT NOT NULL,
                     value_enc BLOB NOT NULL, nonce BLOB NOT NULL, created_at TEXT NOT NULL
                 );
                 INSERT INTO credentials VALUES ('old', 'Old', x'00', x'00', '2024-01-01T00:00:00Z');",
            ).unwrap();
        }
        let store = CredentialStore::open(&db_path, KeySource::File(key_path)).unwrap();

        let err = store.retrieve("old").unwrap_err();

        assert!(err.to_string().contains("corrupt nonce"), "{err}");
    }

    #[test]
    fn concurrent_opens_of_a_legacy_database_all_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("creds.sqlite");
        let key_path = dir.path().join("key.b64");
        CredentialStore::key_from_file(&key_path, false).unwrap();
        {
            let raw = Connection::open(&db_path).unwrap();
            raw.execute_batch(
                "CREATE TABLE credentials (
                     id TEXT PRIMARY KEY, name TEXT NOT NULL,
                     value_enc BLOB NOT NULL, nonce BLOB NOT NULL, created_at TEXT NOT NULL
                 );
                 INSERT INTO credentials VALUES ('old', 'Old', x'00', x'00', '2024-01-01T00:00:00Z');",
            ).unwrap();
        }

        let results: Vec<Result<CredentialStore, EngineError>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..6)
                .map(|_| {
                    scope.spawn(|| CredentialStore::open(&db_path, KeySource::File(key_path.clone())))
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        for result in results {
            assert!(result.is_ok(), "{:?}", result.err());
        }
    }

    fn stored_secret(store: &CredentialStore, id: &str) -> Option<String> {
        store.retrieve(id).unwrap().map(|s| s.as_str().to_owned())
    }

    fn open_scratch_store() -> (tempfile::TempDir, std::path::PathBuf, CredentialStore) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("creds.sqlite");
        let key_path = dir.path().join("key.b64");
        let store = CredentialStore::open(&db_path, KeySource::File(key_path)).unwrap();
        (dir, db_path, store)
    }

    fn request(id: &str, value: &str) -> CreateCredentialRequest {
        CreateCredentialRequest {
            id: id.to_string(),
            name: "Key".to_string(),
            value: value.to_string(),
            cred_type: "api_key".to_string(),
            provider: None,
            model: None,
            base_url: None,
        }
    }

    #[test]
    fn out_of_bounds_input_is_rejected_and_nothing_is_stored() {
        let (_dir, _db, store) = open_scratch_store();
        let mut bad: Vec<CreateCredentialRequest> = Vec::new();
        let too_long_id = "a".repeat(MAX_ID_LEN + 1);
        for id in ["", "a.b", "has space", too_long_id.as_str()] {
            bad.push(request(id, "v"));
        }
        bad.push(request("k", ""));
        bad.push(request("k", &"v".repeat(MAX_VALUE_BYTES + 1)));
        let mut r = request("k", "v");
        r.name = "bad\nname".to_string();
        bad.push(r);
        let mut r = request("k", "v");
        r.cred_type = "password".to_string();
        bad.push(r);
        for url in ["ftp://host/x", "javascript:alert(1)", "not a url", "https://user:pw@host/x"] {
            let mut r = request("k", "v");
            r.base_url = Some(url.to_string());
            bad.push(r);
        }
        let mut r = request("k", "v");
        r.model = Some("m".repeat(MAX_MODEL_CHARS + 1));
        bad.push(r);

        for req in &bad {
            assert!(
                matches!(store.create(req), Err(EngineError::CredentialInvalid(_))),
                "accepted: id={:?} cred_type={:?} base_url={:?}", req.id, req.cred_type, req.base_url
            );
        }
        assert!(store.list().unwrap().is_empty());

        let mut ok = request(&"a".repeat(MAX_ID_LEN), &"v".repeat(MAX_VALUE_BYTES));
        ok.base_url = Some("https://api.example.com/v1".to_string());
        store.create(&ok).unwrap();

        let mut local = request("local_model", "");
        local.provider = Some("local".to_string());
        store.create(&local).unwrap();
        assert_eq!(stored_secret(&store, "local_model").as_deref(), Some(""));
    }

    #[test]
    fn create_never_overwrites_and_update_keeps_created_at_and_a_blank_value_keeps_the_secret() {
        let (_dir, db_path, store) = open_scratch_store();
        store.create(&request("k", "first")).unwrap();
        let created_at = |db: &std::path::Path| -> String {
            Connection::open(db)
                .unwrap()
                .query_row("SELECT created_at FROM credentials WHERE id = 'k'", [], |r| r.get(0))
                .unwrap()
        };
        let stamp = created_at(&db_path);

        assert!(matches!(store.create(&request("k", "second")), Err(EngineError::CredentialExists(_))));
        assert_eq!(stored_secret(&store, "k").as_deref(), Some("first"));

        let mut renamed = request("k", "");
        renamed.name = "Renamed".to_string();
        store.update(&renamed).unwrap();
        assert_eq!(stored_secret(&store, "k").as_deref(), Some("first"));
        assert_eq!(store.list().unwrap()[0].name, "Renamed");

        store.update(&request("k", "rotated")).unwrap();
        assert_eq!(stored_secret(&store, "k").as_deref(), Some("rotated"));
        assert_eq!(created_at(&db_path), stamp);

        assert!(matches!(store.update(&request("missing", "v")), Err(EngineError::CredentialNotFound(_))));
    }

    #[test]
    fn unknown_id_is_not_found_on_delete_and_an_older_id_stays_updatable() {
        let (_dir, db_path, store) = open_scratch_store();
        assert!(matches!(store.delete("nope"), Err(EngineError::CredentialNotFound(_))));

        Connection::open(&db_path).unwrap().execute(
            "INSERT INTO credentials (id, name, cred_type, value_enc, nonce, created_at)
             VALUES ('old.id', 'Old', 'api_key', x'00', x'00', '2024-01-01T00:00:00Z')",
            [],
        ).unwrap();
        store.update(&request("old.id", "")).unwrap();
        store.delete("old.id").unwrap();
    }

    #[test]
    fn secret_cannot_be_moved_to_another_credential_or_retyped() {
        let (_dir, db_path, store) = open_scratch_store();
        store.create(&request("a", "secret-a")).unwrap();
        store.create(&request("b", "secret-b")).unwrap();
        let raw = Connection::open(&db_path).unwrap();
        raw.execute(
            "UPDATE credentials
             SET value_enc = (SELECT value_enc FROM credentials WHERE id = 'a'),
                 nonce = (SELECT nonce FROM credentials WHERE id = 'a')
             WHERE id = 'b'",
            [],
        ).unwrap();

        assert!(matches!(store.retrieve("b"), Err(EngineError::Encryption(_))));
        assert_eq!(stored_secret(&store, "a").as_deref(), Some("secret-a"));

        raw.execute("UPDATE credentials SET cred_type = 'bearer' WHERE id = 'a'", []).unwrap();
        assert!(matches!(store.retrieve("a"), Err(EngineError::Encryption(_))));
    }

    #[test]
    fn legacy_row_is_readable_and_rewritten_in_the_current_format_on_first_read() {
        let (_dir, db_path, store) = open_scratch_store();
        let (enc, nonce) = seal(&store.cipher, b"legacy-secret", b"").unwrap();
        Connection::open(&db_path).unwrap().execute(
            "INSERT INTO credentials (id, name, cred_type, value_enc, nonce, created_at)
             VALUES ('old', 'Old', 'api_key', ?1, ?2, '2024-01-01T00:00:00Z')",
            params![enc, nonce.to_vec()],
        ).unwrap();
        let version = || -> i64 {
            Connection::open(&db_path)
                .unwrap()
                .query_row("SELECT enc_version FROM credentials WHERE id = 'old'", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(version(), 0);

        assert_eq!(stored_secret(&store, "old").as_deref(), Some("legacy-secret"));
        assert_eq!(version(), ENC_VERSION_CURRENT);
        assert_eq!(stored_secret(&store, "old").as_deref(), Some("legacy-secret"));
    }

    #[test]
    fn read_only_database_still_serves_a_legacy_row_and_reports_its_failed_rewrite_once() {
        let (_dir, db_path, store) = open_scratch_store();
        let (enc, nonce) = seal(&store.cipher, b"legacy-secret", b"").unwrap();
        Connection::open(&db_path).unwrap().execute(
            "INSERT INTO credentials (id, name, cred_type, value_enc, nonce, created_at)
             VALUES ('old', 'Old', 'api_key', ?1, ?2, '2024-01-01T00:00:00Z')",
            params![enc, nonce.to_vec()],
        ).unwrap();
        store.conn.lock().unwrap().execute_batch("PRAGMA query_only = ON").unwrap();

        assert_eq!(stored_secret(&store, "old").as_deref(), Some("legacy-secret"));
        assert_eq!(stored_secret(&store, "old").as_deref(), Some("legacy-secret"));

        assert!(!store.first_rewrite_failure("old"), "the failure must already be on record");
        assert!(store.first_rewrite_failure("another"));
    }

    #[test]
    fn update_that_changes_the_type_keeps_the_secret_readable() {
        let (_dir, _db, store) = open_scratch_store();
        store.create(&request("k", "first")).unwrap();
        let mut retyped = request("k", "");
        retyped.cred_type = "bearer".to_string();
        retyped.model = Some("m1".to_string());

        store.update(&retyped).unwrap();

        assert_eq!(stored_secret(&store, "k").as_deref(), Some("first"));
    }

    #[test]
    fn store_opened_with_another_key_is_refused_and_a_damaged_check_value_is_repaired() {
        let (dir, db_path, store) = open_scratch_store();
        store.create(&request("k", "v")).unwrap();
        drop(store);
        let key_path = dir.path().join("key.b64");
        let other_key = dir.path().join("other.key");
        std::fs::write(&other_key, B64.encode([9u8; 32])).unwrap();

        let refused = CredentialStore::open(&db_path, KeySource::File(other_key));
        assert!(matches!(refused.err(), Some(EngineError::KeyMismatch(_))));

        Connection::open(&db_path)
            .unwrap()
            .execute("UPDATE credential_meta SET nonce = zeroblob(12)", [])
            .unwrap();
        let reopened = CredentialStore::open(&db_path, KeySource::File(key_path.clone())).unwrap();
        assert_eq!(stored_secret(&reopened, "k").as_deref(), Some("v"));
        drop(reopened);
        assert!(CredentialStore::open(&db_path, KeySource::File(key_path)).is_ok());
    }

    #[test]
    fn restore_replaces_the_key_only_when_it_decrypts_the_store() {
        let (dir, db_path, store) = open_scratch_store();
        store.create(&request("k", "v")).unwrap();
        let backup = store.export_key_base64();
        drop(store);
        let key_path = dir.path().join("key.b64");
        let wrong = B64.encode([9u8; 32]);

        let before = std::fs::read(&key_path).unwrap();
        let refused = CredentialStore::restore_key_base64(&db_path, KeySource::File(key_path.clone()), &wrong);
        assert!(matches!(refused, Err(EngineError::KeyMismatch(_))));
        assert_eq!(std::fs::read(&key_path).unwrap(), before);

        std::fs::write(&key_path, &wrong).unwrap();
        assert!(CredentialStore::open(&db_path, KeySource::File(key_path.clone())).is_err());
        CredentialStore::restore_key_base64(&db_path, KeySource::File(key_path.clone()), &backup).unwrap();
        let reopened = CredentialStore::open(&db_path, KeySource::File(key_path)).unwrap();
        assert_eq!(stored_secret(&reopened, "k").as_deref(), Some("v"));
    }

    #[test]
    fn restore_into_the_keychain_replaces_the_entry_and_removes_the_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let fallback = dir.path().join(".cred.key");
        std::fs::write(&fallback, b"stale").unwrap();
        let slot = FakeSlot::holding(&[1u8; 32]);
        let restored = B64.encode([2u8; 32]);

        CredentialStore::restore_into_slot(&slot, &fallback, &restored).unwrap();

        assert_eq!(slot.stored(), Some(restored));
        assert!(!fallback.exists());
    }

    #[test]
    fn request_debug_output_omits_the_secret_value() {
        let shown = format!("{:?}", request("k", "sekret-value-xyz"));
        assert!(!shown.contains("sekret-value-xyz"), "{shown}");
    }
}
