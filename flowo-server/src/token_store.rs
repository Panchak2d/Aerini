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
use rand::RngCore;
use base64::Engine;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Mutex};
use subtle::ConstantTimeEq;
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
    pub expires_at: Option<String>,
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
    pub expires_at: Option<String>,
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
            );
            -- Per-workflow SSE ACL (P3-2).
            -- Row presence = token is restricted to that workflow's events.
            -- Tokens with NO rows in this table see ALL events (backward compat).
            -- Admin-scoped tokens always see all events regardless of this table.
            -- Note: no ON DELETE CASCADE — foreign_keys pragma is off in this db.
            -- ACL rows for deleted tokens become orphaned but are harmless (never matched
            -- against an active token). A future migration can clean them up.
            CREATE TABLE IF NOT EXISTS token_workflow_acl (
                token_id    TEXT NOT NULL,
                workflow_id TEXT NOT NULL,
                granted_at  TEXT NOT NULL,
                PRIMARY KEY (token_id, workflow_id)
            );",
        )?;
        // Schema migration: add expires_at column for existing databases that predate M-1 fix.
        let has_expires: bool = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('tokens') WHERE name='expires_at'",
            [],
            |row| row.get::<_, i64>(0),
        ).map(|c| c > 0).unwrap_or(false);
        if !has_expires {
            conn.execute("ALTER TABLE tokens ADD COLUMN expires_at TEXT", [])?;
        }
        Ok(Self { conn: Mutex::new(conn), key })
    }

    /// Create a new token. Returns the raw (unhashed) token string — shown once.
    /// `expires_in_secs`: optional TTL in seconds. None = non-expiring.
    pub fn create_token(&self, label: &str, scopes: &[&str], expires_in_secs: Option<u64>) -> rusqlite::Result<String> {
        // 32 bytes from OsRng → 256 bits of entropy, URL-safe base64 encoded.
        // Replaces UUID v4 which had only 122 bits due to fixed version/variant bits.
        let mut raw_bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut raw_bytes);
        let raw      = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw_bytes);
        let token_id = Uuid::new_v4().to_string();
        let hash     = hash_token(&self.key, &raw);
        let scopes_j = serde_json::to_string(scopes).unwrap_or_else(|_| "[]".to_string());
        let now      = Utc::now();
        let expires_at: Option<String> = expires_in_secs.map(|secs| {
            let secs_i64 = i64::try_from(secs).unwrap_or(i64::MAX);
            (now + chrono::Duration::seconds(secs_i64)).to_rfc3339()
        });
        let conn     = self.conn.lock().expect("token store mutex poisoned");
        conn.execute(
            "INSERT INTO tokens (token_id, token_hash, label, scopes, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![token_id, hash, label, scopes_j, now.to_rfc3339(), expires_at],
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

    /// Verify a raw bearer token. Returns `None` if not found, revoked, or expired.
    pub fn verify_token(&self, raw: &str) -> Option<TokenRecord> {
        let provided_hash = hash_token(&self.key, raw);
        let conn = self.conn.lock().expect("token store mutex poisoned");
        // Use the indexed WHERE clause so the lookup remains O(log n).
        // Then re-verify with constant-time byte comparison in-process for defense-in-depth:
        // SQLite string equality is not guaranteed constant-time, and the hash is sensitive.
        // If the hashes don't match in-process after the DB lookup, treat as not found.
        conn.query_row(
            "SELECT token_id, token_hash, label, scopes, created_at, expires_at
             FROM tokens
             WHERE token_hash = ?1
               AND revoked_at IS NULL",
            params![provided_hash],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,           // token_id
                    row.get::<_, String>(1)?,           // token_hash (for ct_eq)
                    row.get::<_, String>(2)?,           // label
                    row.get::<_, String>(3)?,           // scopes
                    row.get::<_, String>(4)?,           // created_at
                    row.get::<_, Option<String>>(5)?,   // expires_at
                ))
            },
        ).ok().and_then(|(token_id, db_hash, label, scopes_j, created_at, expires_at)| {
            // In-process constant-time confirmation — guards against any SQLite
            // comparison short-circuit or timing side-channel.
            let matched: bool = db_hash.as_bytes()
                .ct_eq(provided_hash.as_bytes())
                .into();
            if !matched { return None; }
            let scopes: Vec<String> = serde_json::from_str(&scopes_j).unwrap_or_default();
            let record = TokenRecord { token_id, label, scopes, created_at, expires_at };
            if let Some(ref exp_str) = record.expires_at {
                if let Ok(exp) = chrono::DateTime::parse_from_rfc3339(exp_str) {
                    if Utc::now() >= exp.with_timezone(&Utc) {
                        return None;
                    }
                }
            }
            Some(record)
        })
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
            "SELECT token_id, label, scopes, created_at, revoked_at, expires_at
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
                expires_at: row.get(5)?,
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

    // ── Per-workflow SSE ACL (P3-2) ──────────────────────────────────────────

    /// Grant a token access to a specific workflow's SSE events.
    /// Once ANY ACL row exists for a token, it is restricted to those workflows only.
    /// No-op if already granted.
    pub fn acl_grant(&self, token_id: &str, workflow_id: &str) -> rusqlite::Result<()> {
        let now  = Utc::now().to_rfc3339();
        let conn = self.conn.lock().expect("token store mutex poisoned");
        conn.execute(
            "INSERT OR IGNORE INTO token_workflow_acl (token_id, workflow_id, granted_at)
             VALUES (?1, ?2, ?3)",
            params![token_id, workflow_id, now],
        )?;
        Ok(())
    }

    /// Revoke a token's access to a specific workflow's SSE events.
    /// No-op if not present.
    pub fn acl_revoke(&self, token_id: &str, workflow_id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("token store mutex poisoned");
        conn.execute(
            "DELETE FROM token_workflow_acl WHERE token_id = ?1 AND workflow_id = ?2",
            params![token_id, workflow_id],
        )?;
        Ok(())
    }

    /// List all workflow IDs a token has been granted access to.
    /// Returns an empty Vec for tokens with no ACL rows (unrestricted).
    pub fn acl_list(&self, token_id: &str) -> rusqlite::Result<Vec<String>> {
        let conn = self.conn.lock().expect("token store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT workflow_id FROM token_workflow_acl
             WHERE token_id = ?1
             ORDER BY granted_at ASC",
        )?;
        let rows = stmt.query_map(params![token_id], |row| row.get::<_, String>(0))?;
        rows.collect()
    }

    /// Determine which workflow IDs a token may observe via SSE.
    ///
    /// Returns `None`  → no filter (see all workflows).
    /// Returns `Some(set)` → only those workflow IDs are visible.
    ///
    /// Rules:
    /// - Admin tokens: always unrestricted (None).
    /// - Tokens with no ACL rows: unrestricted (None) — backward compat.
    /// - Tokens with ≥1 ACL row: restricted to that set (Some).
    pub fn acl_filter(&self, record: &TokenRecord) -> rusqlite::Result<Option<std::collections::HashSet<String>>> {
        if record.has_scope("admin") {
            return Ok(None);
        }
        let ids = self.acl_list(&record.token_id)?;
        if ids.is_empty() {
            Ok(None)
        } else {
            Ok(Some(ids.into_iter().collect()))
        }
    }
}
