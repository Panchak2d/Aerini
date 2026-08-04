//! Chat session/message persistence methods for [`super::WorkflowDb`].

use super::{WorkflowDb, ChatSessionRecord, ChatMessageRecord};

fn row_to_message(row: &rusqlite::Row) -> rusqlite::Result<ChatMessageRecord> {
    // A malformed images_json degrades to `None` rather than failing the
    // whole row — same choice performance_reports.rs makes for history_json.
    // One corrupt attachment blob shouldn't hide the rest of the message.
    let images_json: Option<String> = row.get(3)?;
    Ok(ChatMessageRecord {
        id:        row.get(0)?,
        role:      row.get(1)?,
        text:      row.get(2)?,
        images:    images_json.and_then(|j| serde_json::from_str(&j).ok()),
        timestamp: row.get(4)?,
    })
}

impl WorkflowDb {
    /// All sessions for a workflow, newest-created first, each with its full
    /// message list in chronological order.
    pub fn list_chat_sessions(&self, workflow_id: &str) -> Result<Vec<ChatSessionRecord>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;

        let mut session_stmt = conn.prepare(
            "SELECT id, name, created_at FROM chat_sessions
             WHERE workflow_id = ?1 ORDER BY created_at DESC"
        ).map_err(|e| e.to_string())?;
        let sessions_meta = session_stmt.query_map(rusqlite::params![workflow_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
        }).map_err(|e| e.to_string())?
          .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;

        let mut message_stmt = conn.prepare(
            "SELECT id, role, text, images_json, timestamp FROM chat_messages
             WHERE session_id = ?1 ORDER BY timestamp ASC"
        ).map_err(|e| e.to_string())?;

        let mut sessions = Vec::with_capacity(sessions_meta.len());
        for (id, name, created_at) in sessions_meta {
            let messages = message_stmt.query_map(rusqlite::params![id], row_to_message)
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
            sessions.push(ChatSessionRecord {
                id, workflow_id: workflow_id.to_string(), name, created_at, messages,
            });
        }
        Ok(sessions)
    }

    /// Upserts the session row and replaces its full message list. The
    /// caller (ChatPanel.ts's persist()) always sends the complete current
    /// state of one session — same "caller owns the full state" shape
    /// persist() used for the whole store under localStorage — so replacing
    /// rather than diffing keeps this in step with that contract.
    pub fn save_chat_session(&self, session: &ChatSessionRecord) -> Result<(), String> {
        let mut conn = self.pool.get().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        tx.execute(
            "INSERT OR REPLACE INTO chat_sessions (id, workflow_id, name, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![session.id, session.workflow_id, session.name, session.created_at],
        ).map_err(|e| e.to_string())?;

        tx.execute("DELETE FROM chat_messages WHERE session_id = ?1", rusqlite::params![session.id])
            .map_err(|e| e.to_string())?;

        for m in &session.messages {
            let images_json = match &m.images {
                Some(imgs) => Some(serde_json::to_string(imgs).map_err(|e| e.to_string())?),
                None => None,
            };
            tx.execute(
                "INSERT INTO chat_messages (id, session_id, role, text, images_json, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![m.id, session.id, m.role, m.text, images_json, m.timestamp],
            ).map_err(|e| e.to_string())?;
        }

        tx.commit().map_err(|e| e.to_string())
    }

    /// Deletes a session. chat_messages for it cascade via the FK ON DELETE
    /// CASCADE (same mechanism workflow_versions uses off the workflows row).
    pub fn delete_chat_session(&self, id: &str) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM chat_sessions WHERE id = ?1", rusqlite::params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ChatImageFile;
    use std::path::PathBuf;

    fn temp_path(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("aerini_chat_db_test_{}.db", tag));
        p
    }

    fn cleanup(path: &PathBuf) {
        let _ = std::fs::remove_file(path);
        let mut wal = path.clone();
        wal.set_extension("db-wal");
        let _ = std::fs::remove_file(&wal);
        let mut shm = path.clone();
        shm.set_extension("db-shm");
        let _ = std::fs::remove_file(&shm);
    }

    fn sample_session(id: &str, workflow_id: &str, created_at: i64) -> ChatSessionRecord {
        ChatSessionRecord {
            id: id.to_string(),
            workflow_id: workflow_id.to_string(),
            name: "Session".to_string(),
            created_at,
            messages: vec![
                ChatMessageRecord {
                    id: format!("{id}-m1"), role: "user".to_string(),
                    text: Some("hi".to_string()), images: None, timestamp: created_at + 1,
                },
                ChatMessageRecord {
                    id: format!("{id}-m2"), role: "ai".to_string(),
                    text: None,
                    images: Some(vec![ChatImageFile {
                        filename: "a.png".to_string(), data: "YQ==".to_string(), mime_type: "image/png".to_string(),
                    }]),
                    timestamp: created_at + 2,
                },
            ],
        }
    }

    // Normal case: save then list — session metadata and every message field,
    // including the images_json round trip, come back intact and in order.
    #[test]
    fn save_and_list_chat_session_round_trips_messages_and_images() {
        let path = temp_path("roundtrip");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");

        db.save_chat_session(&sample_session("s1", "wf-1", 1_000)).expect("save failed");

        let sessions = db.list_chat_sessions("wf-1").expect("list failed");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].messages.len(), 2);
        assert_eq!(sessions[0].messages[0].text.as_deref(), Some("hi"));
        assert_eq!(sessions[0].messages[1].images.as_ref().unwrap()[0].filename, "a.png");

        cleanup(&path);
    }

    // Edge case: re-saving the same session id with a shorter message list
    // replaces rather than appends — no stale rows left over from the prior save.
    #[test]
    fn save_chat_session_replaces_message_list_on_resave() {
        let path = temp_path("resave");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");

        db.save_chat_session(&sample_session("s1", "wf-1", 1_000)).expect("save failed");
        let mut second = sample_session("s1", "wf-1", 1_000);
        second.messages.truncate(1);
        db.save_chat_session(&second).expect("resave failed");

        let sessions = db.list_chat_sessions("wf-1").expect("list failed");
        assert_eq!(sessions.len(), 1, "resave must not create a duplicate session row");
        assert_eq!(sessions[0].messages.len(), 1, "old messages must not survive a resave");

        cleanup(&path);
    }

    #[test]
    fn list_chat_sessions_orders_newest_first_and_scopes_by_workflow() {
        let path = temp_path("ordering");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");

        db.save_chat_session(&sample_session("s1", "wf-1", 1_000)).unwrap();
        db.save_chat_session(&sample_session("s2", "wf-1", 2_000)).unwrap();
        db.save_chat_session(&sample_session("s3", "wf-2", 3_000)).unwrap();

        let wf1 = db.list_chat_sessions("wf-1").unwrap();
        assert_eq!(wf1.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["s2", "s1"]);
        assert_eq!(db.list_chat_sessions("wf-2").unwrap().len(), 1);

        cleanup(&path);
    }

    #[test]
    fn delete_chat_session_cascades_its_messages() {
        let path = temp_path("delete");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");

        db.save_chat_session(&sample_session("s1", "wf-1", 1_000)).unwrap();
        db.delete_chat_session("s1").expect("delete failed");

        assert!(db.list_chat_sessions("wf-1").unwrap().is_empty());
        let conn = db.pool.get().unwrap();
        let leftover: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages WHERE session_id = 's1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(leftover, 0, "messages must cascade-delete with their session");

        cleanup(&path);
    }
}
