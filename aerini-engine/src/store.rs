//! Credential store — AES-256-GCM encrypted values in SQLite.
//!
//! Moved from commands/credentials.rs so both the Tauri app and the server
//! binary can use it without any Tauri dependency.

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand_core::RngCore;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use zeroize::Zeroize;

use crate::error::EngineError;
use crate::executor::CredentialResolver;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    conn:    Mutex<Connection>,
    cipher:  Aes256Gcm,
    raw_key: RawKey,
}

/// Holds the same 32-byte AES-256 key `cipher` was built from, purely so
/// `export_key_base64` can hand it back out for backup — nothing else reads
/// this field. Wiped on drop: this is the single value that decrypts every
/// credential in the store, so it gets the same care as the private key
/// material `cipher` itself already holds internally.
struct RawKey(Vec<u8>);

impl Drop for RawKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl CredentialStore {
    pub fn open(db_path: &Path, key_source: KeySource) -> Result<Self, EngineError> {
        let key_bytes = Self::load_key(key_source)?;
        let key    = Key::<Aes256Gcm>::from_slice(&key_bytes);
        let cipher = Aes256Gcm::new(key);
        let raw_key = RawKey(key_bytes);

        let conn = Connection::open(db_path)
            .map_err(|e| EngineError::Database(e.to_string()))?;

        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS credentials (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                cred_type   TEXT NOT NULL DEFAULT 'api_key',
                value_enc   BLOB NOT NULL,
                nonce       BLOB NOT NULL,
                created_at  TEXT NOT NULL,
                metadata    TEXT
            );
        ").map_err(|e| EngineError::Database(e.to_string()))?;

        // Add cred_type column to existing databases that predate this field.
        // SQLite does not support ALTER TABLE ADD COLUMN IF NOT EXISTS, so we
        // attempt the migration and silently ignore the "duplicate column name" error.
        let _ = conn.execute(
            "ALTER TABLE credentials ADD COLUMN cred_type TEXT NOT NULL DEFAULT 'api_key'",
            [],
        );

        // Same pattern for the metadata column (added after cred_type).
        let _ = conn.execute(
            "ALTER TABLE credentials ADD COLUMN metadata TEXT",
            [],
        );

        Ok(Self { conn: Mutex::new(conn), cipher, raw_key })
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
    /// To restore from a backup: write this exact string to the key file
    /// this store was (or will be) opened with — `KeySource::File`'s path,
    /// or `KeySource::OsKeychain`'s `fallback` path — before the app next
    /// starts. `key_from_keychain`'s existing migration path picks up a
    /// fallback-file key and moves it into the OS keychain automatically;
    /// no separate import method is needed.
    ///
    /// Anyone holding this value can decrypt every credential in this store.
    /// Treat it with at least as much care as the credentials themselves.
    pub fn export_key_base64(&self) -> String {
        B64.encode(&self.raw_key.0)
    }

    pub fn store(&self, req: &CreateCredentialRequest) -> Result<(), EngineError> {
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let encrypted = self.cipher
            .encrypt(nonce, req.value.as_bytes())
            .map_err(|e| EngineError::Encryption(e.to_string()))?;

        let meta = CredentialMetadata {
            provider: req.provider.clone(),
            model:    req.model.clone(),
            base_url: req.base_url.clone(),
        };
        let metadata_json: Option<String> = if meta.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&meta)
                .map_err(|e| EngineError::Database(format!("failed to serialize credential metadata: {e}")))?)
        };

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT OR REPLACE INTO credentials (id, name, cred_type, value_enc, nonce, created_at, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                req.id, req.name, req.cred_type, encrypted, nonce_bytes.to_vec(),
                chrono::Utc::now().to_rfc3339(), metadata_json,
            ],
        ).map_err(|e| EngineError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn retrieve(&self, id: &str) -> Result<Option<String>, EngineError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
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
                if nonce_bytes.len() != 12 {
                    return Err(EngineError::Encryption(format!(
                        "credential '{id}' has a corrupt nonce: expected 12 bytes, got {}",
                        nonce_bytes.len()
                    )));
                }
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

        const SERVICE: &str = "aerini";
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

            // Create the key file with owner-only (0o600) permissions atomically
            // on Unix, via the file-creation mode itself rather than a separate
            // fs::write + set_permissions pair. The two-call version left a
            // window — however brief — where this file (holding the AES-256 key
            // that decrypts every stored credential) existed on disk at the
            // default create mode (typically 0o644 after umask, world/group
            // readable) before the follow-up chmod call landed.
            #[cfg(unix)]
            {
                use std::io::Write;
                use std::os::unix::fs::OpenOptionsExt;
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(key_path)
                    .map_err(|e| EngineError::Encryption(e.to_string()))?;
                f.write_all(encoded.as_bytes())
                    .map_err(|e| EngineError::Encryption(e.to_string()))?;
            }

            #[cfg(not(unix))]
            {
                std::fs::write(key_path, &encoded)
                    .map_err(|e| EngineError::Encryption(e.to_string()))?;
            }

            // On Windows, set a DACL that grants access only to the current user,
            // mirroring the Unix 0o600 intent.
            #[cfg(windows)]
            set_owner_only_acl(key_path)
                .map_err(|e| EngineError::Encryption(e.to_string()))?;

            Ok(key.to_vec())
        }
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
        let mut buf = vec![0u8; needed as usize];
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
        if InitializeAcl(acl_buf.as_mut_ptr() as *mut ACL, ACL_BUF_BYTES as u32, ACL_REVISION as u32) == 0 {
            return Err(format!(
                "InitializeAcl failed: {}",
                windows_sys::Win32::Foundation::GetLastError()
            ));
        }
        if AddAccessAllowedAce(
            acl_buf.as_mut_ptr() as *mut ACL,
            ACL_REVISION as u32,
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
    async fn resolve(&self, credential_id: &str) -> Option<String> {
        // `CredentialStore::retrieve` is a synchronous fn — it
        // acquires a blocking `std::sync::Mutex<rusqlite::Connection>` and
        // runs the query + AES-256-GCM decrypt inline. `resolve` is called
        // on every node execution that references a credential, so running
        // that synchronously here would hold the calling tokio worker
        // thread for the duration of the lock + query on every such call.
        // Offload to the blocking thread pool instead. A `spawn_blocking`
        // panic degrades to `None` ("no credential"), matching this
        // method's own pre-existing `.ok().flatten()` behavior on a DB
        // error — both were already "credential unavailable" outcomes.
        let store         = Arc::clone(&self.store);
        let credential_id = credential_id.to_string();
        tokio::task::spawn_blocking(move || store.retrieve(&credential_id).ok().flatten())
            .await
            .unwrap_or(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_from_file_generates_and_reloads_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("key.b64");

        let generated = CredentialStore::key_from_file(&key_path).unwrap();
        assert_eq!(generated.len(), 32, "generated key must be 32 bytes");

        let reloaded = CredentialStore::key_from_file(&key_path).unwrap();
        assert_eq!(
            generated, reloaded,
            "a second call against the same path must return the same key, not regenerate"
        );
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

        CredentialStore::key_from_file(&key_path).unwrap();

        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
        // Mask to the permission bits only (mode() also carries file-type bits).
        assert_eq!(
            mode & 0o777,
            0o600,
            "key file must be created with exactly owner read/write (0o600), got {:o}",
            mode & 0o777
        );
    }

    #[test]
    fn retrieve_with_corrupt_nonce_returns_error_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("creds.sqlite");
        let key_path = dir.path().join("key.b64");

        let store = CredentialStore::open(&db_path, KeySource::File(key_path)).unwrap();
        store.store(&CreateCredentialRequest {
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

    /// This is the actual #7 recovery scenario, reproduced end-to-end: a
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
        store.store(&CreateCredentialRequest {
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
        assert!(
            CredentialStore::open(&db_path, KeySource::File(key_path.clone()))
                .unwrap()
                .retrieve("cred1")
                .is_err(),
            "sanity check: losing the key file must actually break decryption \
             (a fresh key was silently generated and now can't read the old rows), \
             otherwise this test would pass without the backup doing anything"
        );

        // Restore: write the backed-up key back to the expected path.
        std::fs::write(&key_path, &backed_up_key).unwrap();

        let restored = CredentialStore::open(&db_path, KeySource::File(key_path)).unwrap();
        assert_eq!(
            restored.retrieve("cred1").unwrap(),
            Some("irreplaceable-secret".to_string()),
            "restoring the exported key must recover the original credential"
        );
    }
}
