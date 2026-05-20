//! SQLite-backed token registry for API mode.
//!
//! Each token is stored as a BLAKE3 keyed hash — the raw token is shown once at
//! creation time and never persisted in plaintext. The 32-byte key lives in
//! `data_dir/tokens.key` and is generated on first run.
//!
//! Scopes: `read`, `write`, `admin`.
//! - `read`  → GET endpoints
//! - `write` → POST / DELETE on workflows, credentials, scheduler
//! - `admin` → all of the above + token management (`/api/tokens`)
//!
//! An `admin`-scoped token satisfies any scope check.

use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Mutex};
use uuid::Uuid;

fn hash_token(key: &[u8; 32], raw: &str) -> String {
    blake3::keyed_hash(key, raw.as_bytes()).to_hex().to_string()
}

/// A token record returned from lookups. Does NOT contain the raw token or hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRecord {
    pub token_id:   String,
    pub label:      String,
    pub scopes:     Vec<String>,
    pub created_at: String,
}

impl TokenRecord {
    pub fn has_scope(&self, required: &str) -> bool {
        self.scopes.iter().any(|s| s == required || s == "admin")
    }
}

/// A token entry for listing — includes revocation status, still no hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub token_id:   String,
    pub label:      String,
    pub scopes:     Vec<String>,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

pub struct TokenStore {
    conn: Mutex<Connection>,
    key:  [u8; 32],
}

impl TokenStore {
    pub fn open(path: &Path, key: [u8; 32]) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tokens (
                token_id   TEXT PRIMARY KEY,
                token_hash TEXT NOT NULL UNIQUE,
                label      TEXT NOT NULL,
                scopes     TEXT NOT NULL,
                created_at TEXT NOT NULL,
                revoked_at TEXT
            );",
        )?;
        Ok(Self { conn: Mutex::new(conn), key })
    }

    /// Create a new token. Returns the raw (unhashed) token string — shown once.
    pub fn create_token(&self, label: &str, scopes: &[&str]) -> rusqlite::Result<String> {
        let raw      = Uuid::new_v4().to_string().replace('-', "");
        let token_id = Uuid::new_v4().to_string();
        let hash     = hash_token(&self.key, &raw);
        let scopes_j = serde_json::to_string(scopes).unwrap_or_else(|_| "[]".to_string());
        let now      = Utc::now().to_rfc3339();
        let conn     = self.conn.lock().expect("token store mutex poisoned");
        conn.execute(
            "INSERT INTO tokens (token_id, token_hash, label, scopes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![token_id, hash, label, scopes_j, now],
        )?;
        Ok(raw)
    }

    /// Import an existing raw token (e.g. from `FLOWO_TOKEN` migration).
    /// If token is already present and active, no-op.
    pub fn import_token(&self, raw: &str, label: &str, scopes: &[&str]) -> rusqlite::Result<()> {
        let hash = hash_token(&self.key, raw);
        let conn = self.conn.lock().expect("token store mutex poisoned");
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tokens WHERE token_hash = ?1",
            params![hash],
            |row| row.get(0),
        )?;
        if count > 0 {
            return Ok(());
        }
        let token_id = Uuid::new_v4().to_string();
        let scopes_j = serde_json::to_string(scopes).unwrap_or_else(|_| "[]".to_string());
        let now      = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO tokens (token_id, token_hash, label, scopes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![token_id, hash, label, scopes_j, now],
        )?;
        Ok(())
    }

    /// Verify a raw bearer token. Returns `None` if not found or revoked.
    pub fn verify_token(&self, raw: &str) -> Option<TokenRecord> {
        let hash = hash_token(&self.key, raw);
        let conn = self.conn.lock().expect("token store mutex poisoned");
        conn.query_row(
            "SELECT token_id, label, scopes, created_at
             FROM tokens
             WHERE token_hash = ?1 AND revoked_at IS NULL",
            params![hash],
            |row| {
                let scopes_j: String = row.get(2)?;
                let scopes: Vec<String> =
                    serde_json::from_str(&scopes_j).unwrap_or_default();
                Ok(TokenRecord {
                    token_id:   row.get(0)?,
                    label:      row.get(1)?,
                    scopes,
                    created_at: row.get(3)?,
                })
            },
        ).ok()
    }

    /// Revoke a token by its `token_id`. No-op if already revoked or not found.
    pub fn revoke_token(&self, token_id: &str) -> rusqlite::Result<()> {
        let now  = Utc::now().to_rfc3339();
        let conn = self.conn.lock().expect("token store mutex poisoned");
        conn.execute(
            "UPDATE tokens SET revoked_at = ?1 WHERE token_id = ?2 AND revoked_at IS NULL",
            params![now, token_id],
        )?;
        Ok(())
    }

    /// List all tokens (active and revoked), ordered by creation time.
    pub fn list_tokens(&self) -> rusqlite::Result<Vec<TokenInfo>> {
        let conn = self.conn.lock().expect("token store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT token_id, label, scopes, created_at, revoked_at
             FROM tokens
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let scopes_j: String = row.get(2)?;
            let scopes: Vec<String> = serde_json::from_str(&scopes_j).unwrap_or_default();
            Ok(TokenInfo {
                token_id:   row.get(0)?,
                label:      row.get(1)?,
                scopes,
                created_at: row.get(3)?,
                revoked_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Returns `true` if the store has no tokens at all (first-run detection).
    pub fn is_empty(&self) -> bool {
        let conn = self.conn.lock().expect("token store mutex poisoned");
        conn.query_row(
            "SELECT COUNT(*) FROM tokens",
            [],
            |row| row.get::<_, i64>(0),
        ).map(|c| c == 0)
         .unwrap_or(true)
    }
}
