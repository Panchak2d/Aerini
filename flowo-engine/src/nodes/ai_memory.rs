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
                .with_init(|conn| conn.execute_batch("PRAGMA journal_mode=WAL;"));
            let pool = Pool::builder()
                .max_size(4)
                .build(manager)
                .map_err(|e| format!("AI memory: could not create pool: {}", e))?;
            // Schema init on first connection
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
            inputs:  vec![PortDefinition { id: "input".to_string(),  label: "In".to_string(),  position: PortPosition::Left }],
            outputs: vec![PortDefinition { id: "output".to_string(), label: "Out".to_string(), position: PortPosition::Right }],
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
                let seq = get_next_seq(&conn, session_id).unwrap_or(0);
                let now = chrono::Utc::now().to_rfc3339();
                match conn.execute(
                    "INSERT INTO ai_memory (session_id, role, content, created_at, seq) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![session_id, role, content, now, seq],
                ) {
                    Ok(_) => {
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
                let content = input.input["content"].as_str().unwrap_or("");
                let _ = conn.execute("DELETE FROM ai_memory WHERE session_id = ?1", params![session_id]);
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
    let mut stmt = conn.prepare(
        "SELECT role, content FROM ai_memory WHERE session_id = ?1 ORDER BY seq ASC LIMIT ?2"
    ).map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![session_id, limit as i64], |row| {
        Ok(json!({ "role": row.get::<_, String>(0)?, "content": row.get::<_, String>(1)? }))
    }).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

fn get_next_seq(conn: &Connection, session_id: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(MAX(seq) + 1, 0) FROM ai_memory WHERE session_id = ?1",
        params![session_id],
        |row| row.get(0),
    ).map_err(|e| e.to_string())
}
