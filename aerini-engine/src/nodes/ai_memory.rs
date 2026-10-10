use async_trait::async_trait;
use base64::Engine as _;
use once_cell::sync::OnceCell;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Connection, TransactionBehavior};
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};
use super::util::{cfg_bool_opt, cfg_u64_opt, decode_file_data};

use super::ai_prompt::{extract_port_attachments, SUPPORTED_ATTACHMENT_MIMES};

// Sizes are decoded bytes and sit under the smallest provider request ceilings
// (base64 adds a third): Anthropic rejects an image over 5 MB encoded, and
// Gemini's documented inline request limit is 20 MB in total.
const MAX_FILE_BYTES: usize = 3 * 1024 * 1024;
const RECALL_BYTE_BUDGET: usize = 12 * 1024 * 1024;
const MAX_SESSION_FILE_BYTES: i64 = 32 * 1024 * 1024;
const DEFAULT_MAX_STORED_FILES: u64 = 20;
const DEFAULT_RECALL_FILES: u64 = 5;
const MAX_RECALL_FILES: u64 = 20;
const MAX_READ_MESSAGES: u64 = 100_000;
const MAX_CONTENT_BYTES: usize = 4 * 1024 * 1024;

struct StoredFile {
    filename:  String,
    mime_type: String,
    data:      String,
    size:      usize,
    hash:      String,
}

/// AI Memory node — stores and retrieves conversation history in a local SQLite database.
/// Enables multi-turn AI conversations where context is preserved across workflow runs.
///
/// Pool is initialised lazily on first execute() call, matching the original behaviour
/// of deferring open/create errors to execution time rather than panicking at startup.
pub struct AiMemoryNode {
    pub db_path: std::path::PathBuf,
    pool: OnceCell<Pool<SqliteConnectionManager>>,
}

impl AiMemoryNode {
    pub fn new(db_path: std::path::PathBuf) -> Self {
        Self { db_path, pool: OnceCell::new() }
    }

    fn get_pool(&self) -> Result<&Pool<SqliteConnectionManager>, String> {
        self.pool.get_or_try_init(|| {
            let manager = SqliteConnectionManager::file(&self.db_path)
                .with_init(|conn| conn.execute_batch(
                    "PRAGMA busy_timeout=5000;
                     PRAGMA journal_mode=WAL;"
                ));
            // r2d2 re-enforces min_idle as a floor on every checkout, not just
            // at pool construction: establish_idle_connections runs inside
            // try_get_inner right after popping an idle connection, before
            // that connection is returned to the caller. With
            // min_idle(Some(1)), checking out the sole warmed connection
            // below immediately schedules a background replacement
            // connection; if that finishes before this connection is dropped
            // — e.g. while the schema-init statements below run — the pool
            // ends up with two connections instead of one. min_idle(Some(0))
            // keeps that floor at zero, so the replenishment call is always a
            // no-op: a connection is only ever created in direct response to
            // real demand.
            let pool = Pool::builder()
                .max_size(4)
                .min_idle(Some(0))
                .build(manager)
                .map_err(|e| format!("AI memory: could not create pool: {}", e))?;
            {
                let conn = pool.get().map_err(|e| e.to_string())?;
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS ai_memory (
                        session_id TEXT NOT NULL,
                        role       TEXT NOT NULL,
                        content    TEXT NOT NULL,
                        created_at TEXT NOT NULL,
                        seq        INTEGER NOT NULL
                    );
                    CREATE INDEX IF NOT EXISTS ai_memory_session
                        ON ai_memory(session_id, seq);
                    CREATE TABLE IF NOT EXISTS ai_memory_files (
                        session_id TEXT    NOT NULL,
                        seq        INTEGER NOT NULL,
                        ord        INTEGER NOT NULL,
                        filename   TEXT    NOT NULL,
                        mime_type  TEXT    NOT NULL,
                        data       TEXT    NOT NULL,
                        size       INTEGER NOT NULL,
                        hash       TEXT    NOT NULL,
                        created_at TEXT    NOT NULL
                    );
                    CREATE INDEX IF NOT EXISTS ai_memory_files_session
                        ON ai_memory_files(session_id, seq);"
                ).map_err(|e| e.to_string())?;
            }
            Ok(pool)
        })
    }
}

#[async_trait]
impl Node for AiMemoryNode {
    fn type_id(&self) -> &'static str { "ai_memory" }
    fn display_name(&self) -> &'static str { "AI Memory" }
    fn node_type(&self) -> NodeType { NodeType::Ai }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Read from or write to persistent AI memory scoped per session, so the AI can remember past conversations — and, optionally, the files shared in them." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["operation", "session_id"],
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["read", "write", "append", "clear", "forget_files"],
                    "description": "read: get history as messages array. write: replace history. append: add a message. clear: delete all messages and files. forget_files: delete the session's remembered files but keep its messages."
                },
                "session_id": {
                    "type": "string",
                    "description": "Unique ID for this conversation thread, e.g. 'user_123' or 'support_chat'"
                },
                "role": {
                    "type": "string",
                    "enum": ["user", "assistant", "system"],
                    "description": "Message role (required for write/append)"
                },
                "content": {
                    "type": "string",
                    "description": "Message content (required for write/append)"
                },
                "files": {
                    "type": "string",
                    "description": "Files to remember with this message (write/append only). An expression that resolves to a files array, e.g. {{Webhook.output.files}}. Leave blank to store no files. Stored files stay on this device until the session is cleared, they are pruned, or you use forget_files."
                },
                "include_files": {
                    "type": "boolean",
                    "description": "Return the session's remembered files in the output's 'files' array (read/append/write). Wire it to an AI node's Files port to show the model those files again. Off by default. The AI provider receives them again on every run that uses them."
                },
                "max_files": {
                    "type": "number",
                    "description": "Most files returned when include_files is on, newest first (default: 5, maximum: 20). Only files attached to messages inside the max_messages window come back, up to 12 MiB in total."
                },
                "max_messages": {
                    "type": "number",
                    "description": "Maximum messages to return on read (default: 20, newest first)"
                },
                "max_stored": {
                    "type": "number",
                    "description": "Maximum messages to retain in storage per session. Oldest rows are pruned after append if exceeded (default: 1000, minimum: 1)."
                },
                "max_stored_files": {
                    "type": "number",
                    "description": "Maximum files to retain per session. Oldest files are pruned after append if exceeded (default: 20, minimum: 1). A session never keeps more than 32 MiB of files, and each file is limited to 3 MiB."
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "messages":      { "type": "array",  "description": "Conversation history as [{role, content}] array" },
                "count":         { "type": "number", "description": "Number of messages in this session" },
                "session_id":    { "type": "string" },
                "files":         { "type": "array",  "description": "Remembered files as [{filename, mime_type, data}] — present only when include_files is on; wire it to a Files port" },
                "files_omitted": { "type": "number", "description": "Remembered files in the message window that did not fit max_files or the size limit — present only when include_files is on" },
                "files_removed": { "type": "number", "description": "Files deleted — forget_files only" }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs:  vec![PortDefinition { id: "input".to_string(),  label: "In".to_string(),  position: PortPosition::Left , port_type: None, arity: PortArity::Single }],
            outputs: vec![PortDefinition { id: "output".to_string(), label: "Out".to_string(), position: PortPosition::Right, port_type: None, arity: PortArity::Single }],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let operation = match input.input["operation"].as_str() {
            Some(op) => op.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_OP", "operation is required")),
        };
        let session_id = match input.input["session_id"].as_str() {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_SESSION", "session_id is required")),
        };
        let max_messages = match cfg_u64_opt(&input.input["max_messages"], "max_messages") {
            Ok(v) => v.unwrap_or(20).min(MAX_READ_MESSAGES) as usize,
            Err(e) => return NodeOutput::failure(e),
        };
        let max_stored = match cfg_u64_opt(&input.input["max_stored"], "max_stored") {
            Ok(v) => v.unwrap_or(1000).clamp(1, i64::MAX as u64) as i64,
            Err(e) => return NodeOutput::failure(e),
        };
        let max_stored_files = match cfg_u64_opt(&input.input["max_stored_files"], "max_stored_files") {
            Ok(v) => v.unwrap_or(DEFAULT_MAX_STORED_FILES).clamp(1, 1000) as i64,
            Err(e) => return NodeOutput::failure(e),
        };
        let max_files = match cfg_u64_opt(&input.input["max_files"], "max_files") {
            Ok(v) => v.unwrap_or(DEFAULT_RECALL_FILES).clamp(1, MAX_RECALL_FILES) as usize,
            Err(e) => return NodeOutput::failure(e),
        };
        let include_files = match cfg_bool_opt(&input.input["include_files"], "include_files") {
            Ok(v) => v.unwrap_or(false),
            Err(e) => return NodeOutput::failure(e),
        };

        let pool = match self.get_pool() {
            Ok(p)  => p.clone(),
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable("DB_ERROR", e)),
        };

        let files_raw = if matches!(operation.as_str(), "append" | "write") {
            extract_port_attachments(&input.input["files"])
        } else {
            Vec::new()
        };

        let job = Job {
            operation,
            session_id,
            role:      input.input["role"].as_str().map(str::to_string),
            content:   input.input["content"].as_str().map(str::to_string),
            files_raw,
            max_messages,
            max_stored,
            max_stored_files,
            max_files,
            include_files,
        };

        match tokio::task::spawn_blocking(move || run_job(pool, job)).await {
            Ok(out) => out,
            Err(_)  => NodeOutput::failure(NodeError::unrecoverable("DB_TASK_PANIC", "AI memory worker stopped unexpectedly")),
        }
    }
}

/// Everything one operation needs, owned so it can run on a blocking thread.
struct Job {
    operation:        String,
    session_id:       String,
    role:             Option<String>,
    content:          Option<String>,
    files_raw:        Vec<Value>,
    max_messages:     usize,
    max_stored:       i64,
    max_stored_files: i64,
    max_files:        usize,
    include_files:    bool,
}

/// The role a message is stored under: one of the three the chat APIs accept,
/// in lower case. Anything else would be stored and then rejected by the AI
/// provider on a later read.
fn normalize_role(role: &str) -> Option<&'static str> {
    match role.trim().to_ascii_lowercase().as_str() {
        "user"      => Some("user"),
        "assistant" => Some("assistant"),
        "system"    => Some("system"),
        _           => None,
    }
}

/// The role and content of an append or write, validated.
fn message_fields<'a>(job: &'a Job, op: &str) -> Result<(&'static str, &'a str), NodeOutput> {
    let role = match job.role.as_deref() {
        Some(r) => r,
        None => return Err(NodeOutput::failure(NodeError::unrecoverable("MISSING_ROLE", format!("role is required for {op}")))),
    };
    let role = match normalize_role(role) {
        Some(r) => r,
        None => return Err(NodeOutput::failure(NodeError::unrecoverable(
            "INVALID_ROLE",
            format!("role must be user, assistant or system, not '{}'", role),
        ))),
    };
    let content = match job.content.as_deref() {
        Some(c) if !c.is_empty() => c,
        _ => return Err(NodeOutput::failure(NodeError::unrecoverable("MISSING_CONTENT", format!("content is required for {op}")))),
    };
    if content.len() > MAX_CONTENT_BYTES {
        return Err(NodeOutput::failure(NodeError::unrecoverable(
            "CONTENT_TOO_LARGE",
            format!("content is {} KiB; the limit for one message is {} KiB", content.len() / 1024, MAX_CONTENT_BYTES / 1024),
        )));
    }
    Ok((role, content))
}

fn run_job(pool: Pool<SqliteConnectionManager>, job: Job) -> NodeOutput {
    let mut conn = match pool.get() {
        Ok(c)  => c,
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable("DB_ERROR", e.to_string())),
    };
    let session_id = job.session_id.as_str();

    match job.operation.as_str() {
        "read" => {
            match read_messages(&conn, session_id, job.max_messages) {
                Ok(msgs) => {
                    let count = msgs.len();
                    let mut out  = json!({ "messages": msgs, "count": count, "session_id": session_id });
                    let mut logs = vec![format!("Read {} messages from session '{}'", count, session_id)];
                    if job.include_files {
                        if let Err(e) = recall_into(&conn, session_id, job.max_messages, job.max_files, &mut out, &mut logs) {
                            return NodeOutput::failure(NodeError::unrecoverable("READ_ERROR", e));
                        }
                    }
                    NodeOutput::success_with_logs(out, logs)
                }
                Err(e) => NodeOutput::failure(NodeError::unrecoverable("READ_ERROR", e)),
            }
        }

        "append" => {
            let (role, content) = match message_fields(&job, "append") {
                Ok(v)  => v,
                Err(out) => return out,
            };
            let (files, mut logs) = prepare_files(&job.files_raw);
            if let Err(e) = append_message(&mut conn, session_id, role, content, &files, job.max_stored, job.max_stored_files) {
                return NodeOutput::failure(NodeError::unrecoverable("WRITE_ERROR", e));
            }
            let msgs  = read_messages(&conn, session_id, job.max_messages).unwrap_or_default();
            let count = msgs.len();
            let mut out = json!({ "messages": msgs, "count": count, "session_id": session_id });
            logs.push(format!("Appended {} message to session '{}'", role, session_id));
            if !files.is_empty() {
                logs.push(format!("Stored {} file(s) with the message", files.len()));
            }
            if job.include_files {
                recall_after_commit(&conn, session_id, job.max_messages, job.max_files, &mut out, &mut logs);
            }
            NodeOutput::success_with_logs(out, logs)
        }

        "write" => {
            let (role, content) = match message_fields(&job, "write") {
                Ok(v)  => v,
                Err(out) => return out,
            };
            let (files, mut logs) = prepare_files(&job.files_raw);
            if let Err(e) = replace_history(&mut conn, session_id, role, content, &files, job.max_stored_files) {
                return NodeOutput::failure(NodeError::unrecoverable("WRITE_ERROR", e));
            }
            let mut out = json!({ "messages": [{"role": role, "content": content}], "count": 1, "session_id": session_id });
            logs.push(format!("Wrote 1 message to session '{}'", session_id));
            if !files.is_empty() {
                logs.push(format!("Stored {} file(s) with the message", files.len()));
            }
            if job.include_files {
                recall_after_commit(&conn, session_id, job.max_messages, job.max_files, &mut out, &mut logs);
            }
            NodeOutput::success_with_logs(out, logs)
        }

        "clear" => {
            match clear_session(&mut conn, session_id) {
                Ok((messages, files)) => NodeOutput::success_with_logs(
                    json!({ "messages": [], "count": 0, "session_id": session_id }),
                    vec![format!("Cleared {} messages and {} files from session '{}'", messages, files, session_id)],
                ),
                Err(e) => NodeOutput::failure(NodeError::unrecoverable("CLEAR_ERROR", e)),
            }
        }

        "forget_files" => {
            match conn.execute("DELETE FROM ai_memory_files WHERE session_id = ?1", params![session_id]) {
                Ok(n) => NodeOutput::success_with_logs(
                    json!({ "session_id": session_id, "files_removed": n }),
                    vec![format!("Removed {} files from session '{}'", n, session_id)],
                ),
                Err(e) => NodeOutput::failure(NodeError::unrecoverable("FORGET_ERROR", e.to_string())),
            }
        }

        other => NodeOutput::failure(NodeError::unrecoverable(
            "INVALID_OP",
            format!("Unknown operation '{}'. Use: read, write, append, clear, forget_files", other),
        )),
    }
}

fn read_messages(conn: &Connection, session_id: &str, limit: usize) -> Result<Vec<Value>, String> {
    // Select the newest `limit` rows (DESC + LIMIT), then reverse so the returned
    // Vec is in chronological order for the caller — the *window* selected is the
    // newest N messages, matching the schema's documented "newest first" contract,
    // while the messages *within* that window still read oldest-to-newest.
    let mut stmt = conn.prepare(
        "SELECT role, content FROM ai_memory WHERE session_id = ?1 ORDER BY seq DESC LIMIT ?2"
    ).map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![session_id, limit as i64], |row| {
        Ok(json!({ "role": row.get::<_, String>(0)?, "content": row.get::<_, String>(1)? }))
    }).map_err(|e| e.to_string())?;
    let mut msgs = rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    msgs.reverse();
    Ok(msgs)
}

fn get_next_seq(conn: &Connection, session_id: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(MAX(seq) + 1, 0) FROM ai_memory WHERE session_id = ?1",
        params![session_id],
        |row| row.get(0),
    ).map_err(|e| e.to_string())
}

/// Validates the raw `files` items the same way `process_attachments` will
/// read them back. A bad item is skipped with a log line and never blocks the
/// message itself, so a conversation turn is not lost over one attachment.
fn prepare_files(raw: &[Value]) -> (Vec<StoredFile>, Vec<String>) {
    let mut files = Vec::new();
    let mut logs  = Vec::new();
    for att in raw {
        let filename = att["filename"].as_str().unwrap_or("file").to_string();
        let mime     = att["mime_type"].as_str().unwrap_or("");
        let data     = att["data"].as_str().unwrap_or("");

        if data.is_empty() {
            logs.push(format!("Skipped file '{}': missing data", filename));
            continue;
        }
        if mime.is_empty() {
            logs.push(format!("Skipped file '{}': missing mime_type", filename));
            continue;
        }
        if !SUPPORTED_ATTACHMENT_MIMES.contains(&mime) {
            logs.push(format!("Skipped file '{}': unsupported type '{}'", filename, mime));
            continue;
        }
        let bytes = match decode_file_data(data) {
            Ok(b)  => b,
            Err(_) => {
                logs.push(format!("Skipped file '{}': invalid base64 data", filename));
                continue;
            }
        };
        if bytes.len() > MAX_FILE_BYTES {
            logs.push(format!("Skipped file '{}': larger than {} MiB", filename, MAX_FILE_BYTES / (1024 * 1024)));
            continue;
        }
        if mime.starts_with("text/") && std::str::from_utf8(&bytes).is_err() {
            logs.push(format!("Skipped file '{}': not valid UTF-8 text", filename));
            continue;
        }
        files.push(StoredFile {
            filename,
            mime_type: mime.to_string(),
            data:      base64::engine::general_purpose::STANDARD.encode(&bytes),
            size:      bytes.len(),
            hash:      blake3::hash(&bytes).to_hex().to_string(),
        });
    }
    (files, logs)
}

/// The message and its files are written in one write transaction so a
/// concurrent append cannot take the same `seq`, which would attach files to
/// the wrong message.
fn append_message(
    conn: &mut Connection,
    session_id: &str,
    role: &str,
    content: &str,
    files: &[StoredFile],
    max_stored: i64,
    max_stored_files: i64,
) -> Result<(), String> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
    let seq = get_next_seq(&tx, session_id)
        .map_err(|e| format!("failed to determine next sequence number: {}", e))?;
    let now = chrono::Utc::now().to_rfc3339();
    tx.execute(
        "INSERT INTO ai_memory (session_id, role, content, created_at, seq) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![session_id, role, content, now, seq],
    ).map_err(|e| e.to_string())?;
    store_files(&tx, session_id, seq, files, max_stored_files)?;
    // Prune oldest rows if session exceeds max_stored cap.
    tx.execute(
        "DELETE FROM ai_memory \
         WHERE session_id = ?1 \
           AND seq NOT IN ( \
             SELECT seq FROM ai_memory \
             WHERE session_id = ?1 \
             ORDER BY seq DESC LIMIT ?2 \
           )",
        params![session_id, max_stored],
    ).map_err(|e| format!("failed to prune old messages: {}", e))?;
    tx.execute(
        "DELETE FROM ai_memory_files \
         WHERE session_id = ?1 \
           AND seq NOT IN (SELECT seq FROM ai_memory WHERE session_id = ?1)",
        params![session_id],
    ).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

fn replace_history(
    conn: &mut Connection,
    session_id: &str,
    role: &str,
    content: &str,
    files: &[StoredFile],
    max_stored_files: i64,
) -> Result<(), String> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
    // These DELETEs' errors must propagate: discarding one would let a DB failure
    // (lock contention, I/O error) fall through to the INSERT, leaving old rows in
    // place alongside the new one while reporting success, silently breaking
    // "write replaces this session's history".
    tx.execute("DELETE FROM ai_memory WHERE session_id = ?1", params![session_id])
        .map_err(|e| format!("failed to clear previous messages: {}", e))?;
    tx.execute("DELETE FROM ai_memory_files WHERE session_id = ?1", params![session_id])
        .map_err(|e| format!("failed to clear previous files: {}", e))?;
    let now = chrono::Utc::now().to_rfc3339();
    tx.execute(
        "INSERT INTO ai_memory (session_id, role, content, created_at, seq) VALUES (?1, ?2, ?3, ?4, 0)",
        params![session_id, role, content, now],
    ).map_err(|e| e.to_string())?;
    store_files(&tx, session_id, 0, files, max_stored_files)?;
    tx.commit().map_err(|e| e.to_string())
}

/// Messages and files go together: a half-cleared session would keep re-sending
/// files the user believes are gone.
fn clear_session(conn: &mut Connection, session_id: &str) -> Result<(usize, usize), String> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
    let messages = tx.execute("DELETE FROM ai_memory WHERE session_id = ?1", params![session_id])
        .map_err(|e| e.to_string())?;
    let files = tx.execute("DELETE FROM ai_memory_files WHERE session_id = ?1", params![session_id])
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok((messages, files))
}

/// A file whose bytes the session already holds moves to its newest message
/// instead of being stored twice, so re-attaching a file each turn costs no
/// extra space and an older message's pruning cannot take it away.
fn store_files(
    conn: &Connection,
    session_id: &str,
    seq: i64,
    files: &[StoredFile],
    max_stored_files: i64,
) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }
    let now = chrono::Utc::now().to_rfc3339();
    for (ord, f) in files.iter().enumerate() {
        conn.execute(
            "DELETE FROM ai_memory_files WHERE session_id = ?1 AND hash = ?2",
            params![session_id, f.hash],
        ).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO ai_memory_files (session_id, seq, ord, filename, mime_type, data, size, hash, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![session_id, seq, ord as i64, f.filename, f.mime_type, f.data, f.size as i64, f.hash, now],
        ).map_err(|e| e.to_string())?;
    }
    prune_files(conn, session_id, max_stored_files)
}

/// Keeps the newest files within the count and byte caps. Once one file is
/// over a cap, every older file goes too, so what survives is always a
/// contiguous run of the most recent files.
fn prune_files(conn: &Connection, session_id: &str, max_stored_files: i64) -> Result<(), String> {
    let rows = {
        let mut stmt = conn.prepare(
            "SELECT rowid, size FROM ai_memory_files WHERE session_id = ?1 ORDER BY seq DESC, ord DESC"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![session_id], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    let mut kept  = 0_i64;
    let mut total = 0_i64;
    let mut full  = false;
    for (rowid, size) in rows {
        if !full && kept < max_stored_files && total + size <= MAX_SESSION_FILE_BYTES {
            kept  += 1;
            total += size;
        } else {
            full = true;
            conn.execute("DELETE FROM ai_memory_files WHERE rowid = ?1", params![rowid])
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Adds `files` and `files_omitted` to `out`. Only files attached to messages
/// inside the returned message window are considered, so a model never sees a
/// file whose conversation turn has dropped out of view. Selection runs over
/// metadata first, so file data is read only for the files actually returned.
fn recall_into(
    conn: &Connection,
    session_id: &str,
    max_messages: usize,
    max_files: usize,
    out: &mut Value,
    logs: &mut Vec<String>,
) -> Result<(), String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let floor: Option<i64> = tx.query_row(
        "SELECT MIN(seq) FROM (SELECT seq FROM ai_memory WHERE session_id = ?1 ORDER BY seq DESC LIMIT ?2)",
        params![session_id, max_messages as i64],
        |row| row.get(0),
    ).map_err(|e| e.to_string())?;

    let mut chosen:  Vec<i64> = Vec::new();
    let mut omitted: usize    = 0;
    let mut bytes:   usize    = 0;
    if let Some(floor) = floor {
        let mut stmt = tx.prepare(
            "SELECT rowid, size FROM ai_memory_files \
             WHERE session_id = ?1 AND seq >= ?2 ORDER BY seq DESC, ord DESC"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![session_id, floor], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? as usize))
        }).map_err(|e| e.to_string())?
          .collect::<Result<Vec<_>, _>>()
          .map_err(|e| e.to_string())?;
        let mut full = false;
        for (rowid, size) in rows {
            if !full && chosen.len() < max_files && bytes + size <= RECALL_BYTE_BUDGET {
                chosen.push(rowid);
                bytes += size;
            } else {
                full = true;
                omitted += 1;
            }
        }
    }

    chosen.reverse();
    let mut files = Vec::with_capacity(chosen.len());
    for rowid in &chosen {
        let file = tx.query_row(
            "SELECT filename, mime_type, data FROM ai_memory_files WHERE rowid = ?1",
            params![rowid],
            |row| Ok(json!({
                "filename":  row.get::<_, String>(0)?,
                "mime_type": row.get::<_, String>(1)?,
                "data":      row.get::<_, String>(2)?,
            })),
        ).map_err(|e| e.to_string())?;
        files.push(file);
    }

    if !files.is_empty() {
        logs.push(format!(
            "Recalled {} file(s), {} KiB, from session '{}'; the AI provider receives them each time a node sends them",
            files.len(), bytes / 1024, session_id,
        ));
    }
    if omitted > 0 {
        logs.push(format!("{} older file(s) did not fit the file limits and were left out", omitted));
    }
    out["files"]         = Value::Array(files);
    out["files_omitted"] = json!(omitted);
    Ok(())
}

/// After append or write has committed, a failed recall must not fail the
/// operation: the message is already stored, so a retry would duplicate it.
/// The output then carries no files and the log says why.
fn recall_after_commit(
    conn: &Connection,
    session_id: &str,
    max_messages: usize,
    max_files: usize,
    out: &mut Value,
    logs: &mut Vec<String>,
) {
    if let Err(e) = recall_into(conn, session_id, max_messages, max_files, out, logs) {
        out["files"]         = json!([]);
        out["files_omitted"] = json!(0);
        logs.push(format!("Could not recall files from session '{}': {}", session_id, e));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;

    fn temp_db_path(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("aerini_ai_memory_test_{}.db", tag));
        p
    }

    fn cleanup(path: &std::path::PathBuf) {
        let _ = std::fs::remove_file(path);
        let mut wal = path.clone();
        wal.set_extension("db-wal");
        let _ = std::fs::remove_file(&wal);
        let mut shm = path.clone();
        shm.set_extension("db-shm");
        let _ = std::fs::remove_file(&shm);
    }

    fn make_input(operation: &str, session_id: &str, role: Option<&str>, content: Option<&str>) -> NodeInput {
        let mut body = json!({ "operation": operation, "session_id": session_id });
        if let Some(r) = role    { body["role"]    = json!(r); }
        if let Some(c) = content { body["content"] = json!(c); }
        input_from_body(body)
    }

    fn input_from_body(body: Value) -> NodeInput {
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "test_node".to_string(),
            workflow_id:  String::new(),
            execution_id: "exec".to_string(),
            input:        body,
            context:      ExecutionContext::default(),
        }
    }

    /// Seeds two sessions' worth of rows, clears one via the same `"clear"`
    /// operation `clear_chat_session` (`src-tauri/src/commands/workflow.rs`)
    /// delegates to via the node registry, and confirms only the targeted
    /// session's rows are gone — the other session's history must survive
    /// untouched.
    #[tokio::test]
    async fn clear_removes_only_the_targeted_session() {
        let path = temp_db_path("clear_scoped");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        for (role, content) in [("user", "hi"), ("assistant", "hello"), ("user", "how are you")] {
            let out = node.execute(make_input("append", "session-a", Some(role), Some(content))).await;
            assert!(out.success, "seed session-a append failed: {:?}", out.error);
        }
        for (role, content) in [("user", "weather?"), ("assistant", "sunny")] {
            let out = node.execute(make_input("append", "session-b", Some(role), Some(content))).await;
            assert!(out.success, "seed session-b append failed: {:?}", out.error);
        }

        let clear_out = node.execute(make_input("clear", "session-a", None, None)).await;
        assert!(clear_out.success, "clear failed: {:?}", clear_out.error);

        let read_a = node.execute(make_input("read", "session-a", None, None)).await;
        assert!(read_a.success, "read session-a failed: {:?}", read_a.error);
        let a_out = read_a.output.unwrap();
        assert_eq!(a_out["count"], 0);
        assert_eq!(a_out["messages"], json!([]));

        let read_b = node.execute(make_input("read", "session-b", None, None)).await;
        assert!(read_b.success, "read session-b failed: {:?}", read_b.error);
        let b_out = read_b.output.unwrap();
        assert_eq!(b_out["count"], 2);
        assert_eq!(
            b_out["messages"],
            json!([
                { "role": "user",      "content": "weather?" },
                { "role": "assistant", "content": "sunny" }
            ]),
            "session-b must survive session-a's clear untouched"
        );

        cleanup(&path);
    }

    /// `read_messages` returns the newest N rows per the schema's documented
    /// "max_messages ... newest first" contract. Seed 25 messages (5 more
    /// than the default max_messages of 20) and assert the returned window
    /// is msg-5..msg-24 (the newest 20), not msg-0..msg-19 (the oldest 20).
    #[tokio::test]
    async fn read_returns_newest_messages_not_oldest() {
        let path = temp_db_path("read_newest");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        for i in 0..25 {
            let out = node.execute(make_input(
                "append", "session-x", Some("user"), Some(&format!("msg-{}", i)),
            )).await;
            assert!(out.success, "seed append {} failed: {:?}", i, out.error);
        }

        let read_out = node.execute(make_input("read", "session-x", None, None)).await;
        assert!(read_out.success, "read failed: {:?}", read_out.error);
        let out = read_out.output.unwrap();
        assert_eq!(out["count"], 20, "default max_messages is 20");
        let messages = out["messages"].as_array().expect("messages must be an array");
        assert_eq!(messages.len(), 20);
        assert_eq!(
            messages[0]["content"], "msg-5",
            "window must start at the newest-20 boundary (msg-5), not the oldest message (msg-0)"
        );
        assert_eq!(
            messages[19]["content"], "msg-24",
            "window must end at the most recently appended message"
        );

        cleanup(&path);
    }

    /// Clearing a session with no rows is a successful no-op, not an error —
    /// the Chat Panel "Clear" button must not surface a spurious failure for
    /// a brand-new, never-persisted session.
    #[tokio::test]
    async fn clear_nonexistent_session_is_a_successful_noop() {
        let path = temp_db_path("clear_noop");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        let out = node.execute(make_input("clear", "never-existed", None, None)).await;
        assert!(out.success, "clear of nonexistent session must succeed: {:?}", out.error);

        cleanup(&path);
    }

    /// Missing and explicitly-empty content on "write" must both be rejected the
    /// same way "append" already rejects them, per the shared "required for
    /// write/append" contract.
    #[tokio::test]
    async fn write_with_missing_or_empty_content_is_rejected() {
        let path = temp_db_path("write_missing_content");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        let missing = node.execute(make_input("write", "session-w", Some("user"), None)).await;
        assert!(!missing.success);
        assert_eq!(missing.error.unwrap().code, "MISSING_CONTENT");

        let empty = node.execute(make_input("write", "session-w", Some("user"), Some(""))).await;
        assert!(!empty.success);
        assert_eq!(empty.error.unwrap().code, "MISSING_CONTENT");

        // Neither rejected call should have written a row.
        let read = node.execute(make_input("read", "session-w", None, None)).await;
        assert_eq!(read.output.unwrap()["count"], 0);

        cleanup(&path);
    }

    #[tokio::test]
    async fn write_with_real_content_still_succeeds() {
        let path = temp_db_path("write_real_content");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        let out = node.execute(make_input("write", "session-w2", Some("user"), Some("hello there"))).await;
        assert!(out.success, "write failed: {:?}", out.error);
        let data = out.output.unwrap();
        assert_eq!(data["count"], 1);
        assert_eq!(data["messages"], json!([{ "role": "user", "content": "hello there" }]));

        cleanup(&path);
    }

    /// A DB failure on the DELETE-before-insert step must not let the INSERT
    /// proceed. Force the DELETE to fail via a trigger and assert the write call
    /// fails, with the session's prior content left untouched, not duplicated.
    #[tokio::test]
    async fn write_reports_failure_and_preserves_data_when_delete_fails() {
        let path = temp_db_path("write_delete_fails");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        // Seed one message via append — does not go through write's delete step.
        let seed = node.execute(make_input("append", "session-fail", Some("user"), Some("original"))).await;
        assert!(seed.success, "seed append failed: {:?}", seed.error);

        // Force every DELETE on ai_memory to fail from this point on.
        {
            let pool = node.get_pool().expect("pool init");
            let conn = pool.get().expect("get conn");
            conn.execute_batch(
                "CREATE TRIGGER block_delete BEFORE DELETE ON ai_memory
                 BEGIN SELECT RAISE(ABORT, 'delete blocked for test'); END;"
            ).expect("trigger install failed");
        }

        let write_out = node.execute(make_input("write", "session-fail", Some("user"), Some("replacement"))).await;
        assert!(!write_out.success, "write must fail when the delete step fails");
        assert_eq!(write_out.error.unwrap().code, "WRITE_ERROR");

        // Original content must survive untouched — no silent duplicate insert.
        let read_out = node.execute(make_input("read", "session-fail", None, None)).await;
        assert!(read_out.success, "read failed: {:?}", read_out.error);
        let data = read_out.output.unwrap();
        assert_eq!(data["count"], 1);
        assert_eq!(data["messages"], json!([{ "role": "user", "content": "original" }]));

        cleanup(&path);
    }

    /// Appending while `get_next_seq`'s seq lookup fails must return
    /// WRITE_ERROR and write nothing, never silently insert at a
    /// wrongly-defaulted seq.
    ///
    /// SQLite triggers don't fire on SELECT, so the trigger technique used by
    /// `write_reports_failure_and_preserves_data_when_delete_fails` above
    /// can't force get_next_seq's `SELECT MAX(seq)` to fail on its own while
    /// leaving the following INSERT able to succeed. Instead, this installs
    /// a `rusqlite` authorizer (`Connection::authorizer`, "hooks" feature,
    /// dev-only — see Cargo.toml) on the pool's one warmed connection that
    /// denies read access to `ai_memory.seq` specifically: that fails the
    /// `SELECT ... MAX(seq) ...` lookup (which reads that column), while the
    /// later `INSERT` — which writes `seq` but never reads it — is untouched
    /// by the denial and would still succeed if reached.
    ///
    /// That specificity is what makes this test discriminate a
    /// silently-defaulting seq lookup from one that fails cleanly under the
    /// exact same authorizer: code that swallows the SELECT failure would
    /// still execute the INSERT, succeeding with a wrongly-defaulted seq of
    /// 0; code that propagates the error never reaches the INSERT.
    #[tokio::test]
    async fn append_fails_cleanly_when_seq_lookup_errors() {
        let path = temp_db_path("append_seq_lookup_fails");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        // Force pool init (and the schema-creation statements it runs)
        // before installing the authorizer, so CREATE TABLE/INDEX aren't
        // denied — then install the authorizer on that same connection.
        {
            let pool = node.get_pool().expect("pool init failed");
            let conn = pool.get().expect("get conn failed");
            conn.authorizer(Some(|ctx: rusqlite::hooks::AuthContext<'_>| {
                use rusqlite::hooks::{AuthAction, Authorization};
                match ctx.action {
                    AuthAction::Read { table_name, column_name }
                        if table_name == "ai_memory" && column_name == "seq" =>
                    {
                        Authorization::Deny
                    }
                    _ => Authorization::Allow,
                }
            })).expect("authorizer install failed");
        }

        let out = node.execute(make_input(
            "append", "session-seqfail", Some("user"), Some("hello"),
        )).await;
        assert!(!out.success, "append must fail when get_next_seq's SELECT is denied, got: {:?}", out.output);
        assert_eq!(out.error.unwrap().code, "WRITE_ERROR");

        // Clear the authorizer before the verification read, so the read
        // (which only touches role/content, not seq) isn't itself denied.
        {
            let pool = node.get_pool().expect("pool init failed");
            let conn = pool.get().expect("get conn failed");
            conn.authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>)
                .expect("authorizer clear failed");
        }
        let read_out = node.execute(make_input("read", "session-seqfail", None, None)).await;
        assert!(read_out.success, "read failed: {:?}", read_out.error);
        assert_eq!(
            read_out.output.unwrap()["count"], 0,
            "no row must have been written when the seq lookup failed"
        );

        cleanup(&path);
    }

    #[test]
    fn fresh_pool_only_warms_one_connection() {
        let path = temp_db_path("ai_memory_warm_one");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        let pool = node.get_pool().expect("pool init failed");
        assert_eq!(pool.state().connections, 1, "only one connection should be eagerly warmed on first use");

        cleanup(&path);
    }

    #[test]
    fn pool_still_grows_to_max_size_under_concurrent_load() {
        let path = temp_db_path("ai_memory_grows_under_load");
        cleanup(&path);
        let node = std::sync::Arc::new(AiMemoryNode::new(path.clone()));
        node.get_pool().expect("pool init failed");

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let handles: Vec<_> = (0..4).map(|_| {
            let node = std::sync::Arc::clone(&node);
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                let pool = node.get_pool().expect("pool init failed");
                let conn = pool.get().expect("pool.get failed under concurrent load");
                barrier.wait();
                let _: i64 = conn.query_row("SELECT 1", [], |r| r.get(0)).expect("query failed");
            })
        }).collect();
        for h in handles {
            h.join().expect("worker thread panicked");
        }

        assert_eq!(node.get_pool().unwrap().state().connections, 4, "pool must still reach max_size once demand requires it");

        cleanup(&path);
    }

    fn input_with(operation: &str, session_id: &str, extra: Value) -> NodeInput {
        let mut body = json!({ "operation": operation, "session_id": session_id });
        for (k, v) in extra.as_object().expect("extra must be a JSON object") {
            body[k.as_str()] = v.clone();
        }
        input_from_body(body)
    }

    fn file_obj(name: &str, mime: &str, bytes: &[u8]) -> Value {
        use base64::Engine as _;
        json!({
            "filename":  name,
            "mime_type": mime,
            "data":      base64::engine::general_purpose::STANDARD.encode(bytes),
        })
    }

    async fn append_with_files(node: &AiMemoryNode, session_id: &str, content: &str, files: Vec<Value>, extra: Value) -> NodeOutput {
        let mut body = json!({ "role": "user", "content": content, "files": files });
        for (k, v) in extra.as_object().expect("extra must be a JSON object") {
            body[k.as_str()] = v.clone();
        }
        let out = node.execute(input_with("append", session_id, body)).await;
        assert!(out.success, "append '{}' failed: {:?}", content, out.error);
        out
    }

    async fn read_with_files(node: &AiMemoryNode, session_id: &str, extra: Value) -> Value {
        let mut body = json!({ "include_files": true });
        for (k, v) in extra.as_object().expect("extra must be a JSON object") {
            body[k.as_str()] = v.clone();
        }
        let out = node.execute(input_with("read", session_id, body)).await;
        assert!(out.success, "read failed: {:?}", out.error);
        out.output.unwrap()
    }

    fn file_names(out: &Value) -> Vec<String> {
        out["files"].as_array().expect("files must be an array")
            .iter().map(|f| f["filename"].as_str().unwrap_or("").to_string()).collect()
    }

    fn file_count(node: &AiMemoryNode, session_id: &str) -> i64 {
        let conn = node.get_pool().expect("pool init failed").get().expect("get conn failed");
        conn.query_row(
            "SELECT COUNT(*) FROM ai_memory_files WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        ).expect("count query failed")
    }

    #[tokio::test]
    async fn remembered_files_come_back_in_chronological_order_when_included() {
        let path = temp_db_path("files_roundtrip");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        append_with_files(&node, "s", "here is a", vec![file_obj("a.txt", "text/plain", b"alpha")], json!({})).await;
        // The executor hands a resolved expression to the node as a JSON string.
        let second = node.execute(input_with("append", "s", json!({
            "role": "user", "content": "here is b", "include_files": true,
            "files": json!([file_obj("b.txt", "text/plain", b"beta")]).to_string(),
        }))).await;
        assert!(second.success, "append with a string-form files value failed: {:?}", second.error);
        assert_eq!(
            file_names(&second.output.unwrap()), vec!["a.txt", "b.txt"],
            "append must return the files it just stored alongside the earlier ones"
        );

        let out = read_with_files(&node, "s", json!({})).await;
        assert_eq!(file_names(&out), vec!["a.txt", "b.txt"]);
        assert_eq!(out["files"][0]["mime_type"], "text/plain");
        assert_eq!(out["files"][0]["data"], file_obj("a.txt", "text/plain", b"alpha")["data"]);
        assert_eq!(out["files_omitted"], 0);
        assert_eq!(
            out["messages"][0], json!({ "role": "user", "content": "here is a" }),
            "messages must stay plain {{role, content}} pairs"
        );

        cleanup(&path);
    }

    #[tokio::test]
    async fn text_only_session_stores_no_files_and_default_read_shape_is_unchanged() {
        let path = temp_db_path("files_opt_in");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        let out = node.execute(make_input("append", "s", Some("user"), Some("hi"))).await;
        assert!(out.success, "append failed: {:?}", out.error);
        assert_eq!(file_count(&node, "s"), 0, "nothing is stored unless the files input is given");

        let plain = node.execute(make_input("read", "s", None, None)).await.output.unwrap();
        assert!(plain.get("files").is_none() && plain.get("files_omitted").is_none(),
            "files keys appear only when include_files is on, got: {}", plain);

        let with = read_with_files(&node, "s", json!({})).await;
        assert_eq!(with["files"], json!([]));
        assert_eq!(with["files_omitted"], 0);

        cleanup(&path);
    }

    #[tokio::test]
    async fn invalid_files_are_skipped_and_the_message_is_still_stored() {
        let path = temp_db_path("files_invalid");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        let too_big = vec![1u8; 3 * 1024 * 1024 + 1];
        let out = append_with_files(&node, "s", "turn", vec![
            file_obj("x.exe", "application/octet-stream", b"zz"),
            json!({ "filename": "bad.txt", "mime_type": "text/plain", "data": "not-valid-base64!!!" }),
            file_obj("big.png", "image/png", &too_big),
            file_obj("latin.txt", "text/plain", &[0xff, 0xfe, 0xfd]),
            json!({ "filename": "nodata.png", "mime_type": "image/png" }),
            file_obj("ok.txt", "text/plain", b"fine"),
        ], json!({})).await;

        assert_eq!(out.output.unwrap()["count"], 1, "the message must be stored");
        assert_eq!(file_count(&node, "s"), 1, "only the valid file is stored");
        assert_eq!(out.logs.iter().filter(|l| l.starts_with("Skipped file")).count(), 5);

        cleanup(&path);
    }

    #[tokio::test]
    async fn re_attaching_the_same_file_keeps_one_copy_at_its_newest_position() {
        let path = temp_db_path("files_dedupe");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        append_with_files(&node, "s", "m1", vec![file_obj("a.txt", "text/plain", b"alpha")], json!({})).await;
        append_with_files(&node, "s", "m2", vec![file_obj("b.txt", "text/plain", b"beta")], json!({})).await;
        append_with_files(&node, "s", "m3", vec![file_obj("a-again.txt", "text/plain", b"alpha")], json!({})).await;

        assert_eq!(file_count(&node, "s"), 2, "identical bytes must not be stored twice");
        let out = read_with_files(&node, "s", json!({})).await;
        assert_eq!(file_names(&out), vec!["b.txt", "a-again.txt"]);

        cleanup(&path);
    }

    #[tokio::test]
    async fn recall_returns_the_newest_max_files_and_reports_the_rest_as_omitted() {
        let path = temp_db_path("files_recall_count");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        for i in 0..7 {
            append_with_files(
                &node, "s", &format!("m{}", i),
                vec![file_obj(&format!("f{}.txt", i), "text/plain", format!("body-{}", i).as_bytes())],
                json!({}),
            ).await;
        }

        let default = read_with_files(&node, "s", json!({})).await;
        assert_eq!(file_names(&default), vec!["f2.txt", "f3.txt", "f4.txt", "f5.txt", "f6.txt"], "default max_files is 5");
        assert_eq!(default["files_omitted"], 2);

        let two = read_with_files(&node, "s", json!({ "max_files": 2 })).await;
        assert_eq!(file_names(&two), vec!["f5.txt", "f6.txt"]);
        assert_eq!(two["files_omitted"], 5);

        cleanup(&path);
    }

    #[tokio::test]
    async fn recall_stops_at_the_byte_budget_without_skipping_a_newer_file_for_an_older_one() {
        let path = temp_db_path("files_recall_bytes");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        for i in 0..5u8 {
            let bytes = vec![i; 3 * 1024 * 1024];
            append_with_files(&node, "s", &format!("m{}", i), vec![file_obj(&format!("f{}.png", i), "image/png", &bytes)], json!({})).await;
        }

        let out = read_with_files(&node, "s", json!({ "max_files": 20 })).await;
        assert_eq!(
            file_names(&out), vec!["f1.png", "f2.png", "f3.png", "f4.png"],
            "five 3 MiB files exceed the 12 MiB recall budget, so the oldest is left out"
        );
        assert_eq!(out["files_omitted"], 1);

        cleanup(&path);
    }

    #[tokio::test]
    async fn session_file_storage_never_exceeds_the_byte_cap() {
        let path = temp_db_path("files_session_cap");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        for i in 0..12u8 {
            let bytes = vec![i; 3 * 1024 * 1024];
            append_with_files(
                &node, "s", &format!("m{}", i),
                vec![file_obj(&format!("f{}.png", i), "image/png", &bytes)],
                json!({ "max_stored_files": 100 }),
            ).await;
        }

        assert_eq!(file_count(&node, "s"), 10, "twelve 3 MiB files exceed the 32 MiB cap, leaving the newest ten");
        let conn = node.get_pool().expect("pool init failed").get().expect("get conn failed");
        let oldest: String = conn.query_row(
            "SELECT filename FROM ai_memory_files WHERE session_id = 's' ORDER BY seq ASC LIMIT 1",
            [],
            |row| row.get(0),
        ).expect("oldest query failed");
        assert_eq!(oldest, "f2.png", "the cap drops the oldest files first");

        cleanup(&path);
    }

    #[tokio::test]
    async fn max_stored_files_prunes_the_oldest_files() {
        let path = temp_db_path("files_max_stored");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        for i in 0..3 {
            append_with_files(
                &node, "s", &format!("m{}", i),
                vec![file_obj(&format!("f{}.txt", i), "text/plain", format!("body-{}", i).as_bytes())],
                json!({ "max_stored_files": 2 }),
            ).await;
        }

        assert_eq!(file_count(&node, "s"), 2);
        let out = read_with_files(&node, "s", json!({})).await;
        assert_eq!(file_names(&out), vec!["f1.txt", "f2.txt"]);

        cleanup(&path);
    }

    #[tokio::test]
    async fn files_outside_the_message_window_are_not_returned() {
        let path = temp_db_path("files_window");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        append_with_files(&node, "s", "m0", vec![file_obj("old.txt", "text/plain", b"old")], json!({})).await;
        for i in 1..4 {
            let out = node.execute(make_input("append", "s", Some("user"), Some(&format!("m{}", i)))).await;
            assert!(out.success, "append failed: {:?}", out.error);
        }

        let narrow = read_with_files(&node, "s", json!({ "max_messages": 2 })).await;
        assert_eq!(narrow["files"], json!([]), "the file's message is outside the 2-message window");
        assert_eq!(narrow["files_omitted"], 0);

        let wide = read_with_files(&node, "s", json!({ "max_messages": 10 })).await;
        assert_eq!(file_names(&wide), vec!["old.txt"]);

        cleanup(&path);
    }

    #[tokio::test]
    async fn pruning_old_messages_removes_their_files() {
        let path = temp_db_path("files_message_prune");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        append_with_files(&node, "s", "m0", vec![file_obj("gone.txt", "text/plain", b"gone")], json!({ "max_stored": 2 })).await;
        append_with_files(&node, "s", "m1", vec![], json!({ "max_stored": 2 })).await;
        append_with_files(&node, "s", "m2", vec![file_obj("kept.txt", "text/plain", b"kept")], json!({ "max_stored": 2 })).await;

        assert_eq!(file_count(&node, "s"), 1);
        let out = read_with_files(&node, "s", json!({})).await;
        assert_eq!(out["count"], 2, "m0 was pruned");
        assert_eq!(file_names(&out), vec!["kept.txt"], "the pruned message's file goes with it");

        cleanup(&path);
    }

    #[tokio::test]
    async fn clear_removes_the_sessions_files_and_leaves_other_sessions_alone() {
        let path = temp_db_path("files_clear_scoped");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        append_with_files(&node, "session-a", "hi", vec![file_obj("a.txt", "text/plain", b"alpha")], json!({})).await;
        append_with_files(&node, "session-b", "hi", vec![file_obj("b.txt", "text/plain", b"beta")], json!({})).await;

        let cleared = node.execute(make_input("clear", "session-a", None, None)).await;
        assert!(cleared.success, "clear failed: {:?}", cleared.error);

        assert_eq!(file_count(&node, "session-a"), 0);
        let b = read_with_files(&node, "session-b", json!({})).await;
        assert_eq!(file_names(&b), vec!["b.txt"], "session-b's files must survive session-a's clear");

        cleanup(&path);
    }

    #[tokio::test]
    async fn clear_that_fails_on_files_leaves_the_messages_in_place() {
        let path = temp_db_path("files_clear_atomic");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        append_with_files(&node, "s", "hi", vec![file_obj("a.txt", "text/plain", b"alpha")], json!({})).await;
        {
            let conn = node.get_pool().expect("pool init failed").get().expect("get conn failed");
            conn.execute_batch(
                "CREATE TRIGGER block_file_delete BEFORE DELETE ON ai_memory_files
                 BEGIN SELECT RAISE(ABORT, 'file delete blocked for test'); END;"
            ).expect("trigger install failed");
        }

        let out = node.execute(make_input("clear", "s", None, None)).await;
        assert!(!out.success, "clear must fail when the file delete fails");
        assert_eq!(out.error.unwrap().code, "CLEAR_ERROR");

        let read = node.execute(make_input("read", "s", None, None)).await.output.unwrap();
        assert_eq!(read["count"], 1, "messages and files are cleared together or not at all");
        assert_eq!(file_count(&node, "s"), 1);

        cleanup(&path);
    }

    #[tokio::test]
    async fn write_replaces_files_along_with_history() {
        let path = temp_db_path("files_write_replaces");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        append_with_files(&node, "s", "old turn", vec![file_obj("old.txt", "text/plain", b"old")], json!({})).await;
        let written = node.execute(input_with("write", "s", json!({
            "role": "system", "content": "fresh start", "include_files": true,
            "files": [file_obj("new.txt", "text/plain", b"new")],
        }))).await;
        assert!(written.success, "write failed: {:?}", written.error);
        assert_eq!(file_names(&written.output.unwrap()), vec!["new.txt"]);

        let out = read_with_files(&node, "s", json!({})).await;
        assert_eq!(out["count"], 1);
        assert_eq!(file_names(&out), vec!["new.txt"], "write must drop the previous history's files");

        cleanup(&path);
    }

    #[tokio::test]
    async fn forget_files_removes_files_but_keeps_messages() {
        let path = temp_db_path("files_forget");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        append_with_files(&node, "s", "hi", vec![file_obj("a.txt", "text/plain", b"alpha")], json!({})).await;

        let out = node.execute(make_input("forget_files", "s", None, None)).await;
        assert!(out.success, "forget_files failed: {:?}", out.error);
        assert_eq!(out.output.unwrap()["files_removed"], 1);

        assert_eq!(file_count(&node, "s"), 0);
        let read = read_with_files(&node, "s", json!({})).await;
        assert_eq!(read["count"], 1, "the conversation itself must remain");
        assert_eq!(read["files"], json!([]));

        cleanup(&path);
    }

    #[tokio::test]
    async fn role_outside_user_assistant_system_is_rejected_and_case_is_normalised() {
        let path = temp_db_path("role_check");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());

        let bad = node.execute(make_input("append", "s", Some("tool"), Some("x"))).await;
        assert_eq!(bad.error.expect("error").code, "INVALID_ROLE");

        let ok = node.execute(make_input("append", "s", Some(" User "), Some("hi"))).await;
        assert!(ok.success, "{:?}", ok.error);
        let read = node.execute(make_input("read", "s", None, None)).await.output.unwrap();
        assert_eq!(read["messages"][0]["role"], "user");
        cleanup(&path);
    }

    #[tokio::test]
    async fn message_over_the_size_limit_is_refused_without_storing_it() {
        let path = temp_db_path("content_cap");
        cleanup(&path);
        let node = AiMemoryNode::new(path.clone());
        let big = "a".repeat(MAX_CONTENT_BYTES + 1);

        let out = node.execute(make_input("append", "s", Some("user"), Some(&big))).await;
        assert_eq!(out.error.expect("error").code, "CONTENT_TOO_LARGE");
        let read = node.execute(make_input("read", "s", None, None)).await.output.unwrap();
        assert_eq!(read["count"], 0);
        cleanup(&path);
    }

    #[test]
    fn data_uri_file_is_stored_as_plain_base64() {
        let raw = vec![json!({ "filename": "a.txt", "mime_type": "text/plain", "data": "data:text/plain;base64,aGk=" })];
        let (files, logs) = prepare_files(&raw);
        assert!(logs.is_empty(), "{logs:?}");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].data, "aGk=");
        assert_eq!(files[0].size, 2);
    }

    #[test]
    fn output_schema_declares_files_so_the_canvas_wires_them_to_a_files_port() {
        let node = AiMemoryNode::new(temp_db_path("files_schema"));
        assert!(
            node.output_schema()["properties"].get("files").is_some(),
            "Canvas injects {{{{Node.output.files}}}} for a Files port only when the source declares a files output"
        );
    }
}
