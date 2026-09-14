use async_trait::async_trait;
use once_cell::sync::OnceCell;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

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
                        ON ai_memory(session_id, seq);"
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
    fn description(&self) -> &'static str { "Read from or write to persistent AI memory scoped per session, so the AI can remember past conversations." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["operation", "session_id"],
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["read", "write", "append", "clear"],
                    "description": "read: get history as messages array. write: replace history. append: add a message. clear: delete all."
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
                "max_messages": {
                    "type": "number",
                    "description": "Maximum messages to return on read (default: 20, newest first)"
                },
                "max_stored": {
                    "type": "number",
                    "description": "Maximum messages to retain in storage per session. Oldest rows are pruned after append if exceeded (default: 1000, minimum: 1)."
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "messages":   { "type": "array",  "description": "Conversation history as [{role, content}] array" },
                "count":      { "type": "number", "description": "Number of messages in this session" },
                "session_id": { "type": "string" }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs:  vec![PortDefinition { id: "input".to_string(),  label: "In".to_string(),  position: PortPosition::Left , port_type: None }],
            outputs: vec![PortDefinition { id: "output".to_string(), label: "Out".to_string(), position: PortPosition::Right, port_type: None }],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let operation = match input.input["operation"].as_str() {
            Some(op) => op,
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_OP", "operation is required")),
        };
        let session_id = match input.input["session_id"].as_str() {
            Some(s) if !s.is_empty() => s,
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_SESSION", "session_id is required")),
        };
        let max_messages = input.input["max_messages"].as_u64().unwrap_or(20) as usize;
        let max_stored   = input.input["max_stored"].as_u64().unwrap_or(1000).max(1) as i64;

        let pool = match self.get_pool() {
            Ok(p)  => p,
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable("DB_ERROR", e)),
        };
        let conn = match pool.get() {
            Ok(c)  => c,
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable("DB_ERROR", e.to_string())),
        };

        match operation {
            "read" => {
                match read_messages(&conn, session_id, max_messages) {
                    Ok(msgs) => {
                        let count = msgs.len();
                        NodeOutput::success_with_logs(
                            json!({ "messages": msgs, "count": count, "session_id": session_id }),
                            vec![format!("Read {} messages from session '{}'", count, session_id)],
                        )
                    }
                    Err(e) => NodeOutput::failure(NodeError::unrecoverable("READ_ERROR", e)),
                }
            }

            "append" => {
                let role = match input.input["role"].as_str() {
                    Some(r) => r,
                    None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_ROLE", "role is required for append")),
                };
                let content = match input.input["content"].as_str() {
                    Some(c) if !c.is_empty() => c,
                    _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_CONTENT", "content is required for append")),
                };
                let seq = match get_next_seq(&conn, session_id) {
                    Ok(s) => s,
                    Err(e) => return NodeOutput::failure(NodeError::unrecoverable(
                        "WRITE_ERROR", format!("failed to determine next sequence number: {}", e),
                    )),
                };
                let now = chrono::Utc::now().to_rfc3339();
                match conn.execute(
                    "INSERT INTO ai_memory (session_id, role, content, created_at, seq) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![session_id, role, content, now, seq],
                ) {
                    Ok(_) => {
                        // Prune oldest rows if session exceeds max_stored cap.
                        let _ = conn.execute(
                            "DELETE FROM ai_memory \
                             WHERE session_id = ?1 \
                               AND seq NOT IN ( \
                                 SELECT seq FROM ai_memory \
                                 WHERE session_id = ?1 \
                                 ORDER BY seq DESC LIMIT ?2 \
                               )",
                            params![session_id, max_stored],
                        );
                        let msgs  = read_messages(&conn, session_id, max_messages).unwrap_or_default();
                        let count = msgs.len();
                        NodeOutput::success_with_logs(
                            json!({ "messages": msgs, "count": count, "session_id": session_id }),
                            vec![format!("Appended {} message to session '{}'", role, session_id)],
                        )
                    }
                    Err(e) => NodeOutput::failure(NodeError::unrecoverable("WRITE_ERROR", e.to_string())),
                }
            }

            "write" => {
                let role = match input.input["role"].as_str() {
                    Some(r) => r,
                    None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_ROLE", "role is required for write")),
                };
                let content = match input.input["content"].as_str() {
                    Some(c) if !c.is_empty() => c,
                    _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_CONTENT", "content is required for write")),
                };
                // This DELETE's error must propagate like the INSERT error below does —
                // discarding it would let a DB failure (lock contention, I/O error) fall
                // through to the INSERT, leaving old row(s) in place alongside the new one
                // while reporting success, silently breaking "write replaces this session's history".
                if let Err(e) = conn.execute("DELETE FROM ai_memory WHERE session_id = ?1", params![session_id]) {
                    return NodeOutput::failure(NodeError::unrecoverable(
                        "WRITE_ERROR", format!("failed to clear previous messages: {}", e),
                    ));
                }
                let now = chrono::Utc::now().to_rfc3339();
                match conn.execute(
                    "INSERT INTO ai_memory (session_id, role, content, created_at, seq) VALUES (?1, ?2, ?3, ?4, 0)",
                    params![session_id, role, content, now],
                ) {
                    Ok(_) => NodeOutput::success_with_logs(
                        json!({ "messages": [{"role": role, "content": content}], "count": 1, "session_id": session_id }),
                        vec![format!("Wrote 1 message to session '{}'", session_id)],
                    ),
                    Err(e) => NodeOutput::failure(NodeError::unrecoverable("WRITE_ERROR", e.to_string())),
                }
            }

            "clear" => {
                match conn.execute("DELETE FROM ai_memory WHERE session_id = ?1", params![session_id]) {
                    Ok(n) => NodeOutput::success_with_logs(
                        json!({ "messages": [], "count": 0, "session_id": session_id }),
                        vec![format!("Cleared {} messages from session '{}'", n, session_id)],
                    ),
                    Err(e) => NodeOutput::failure(NodeError::unrecoverable("CLEAR_ERROR", e.to_string())),
                }
            }

            other => NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_OP",
                format!("Unknown operation '{}'. Use: read, write, append, clear", other),
            )),
        }
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
}

