//! Chat session/message persistence methods for [`super::WorkflowDb`].

use std::collections::HashSet;

use super::{WorkflowDb, ChatSessionRecord, ChatSessionMeta, ChatMessageRecord};

fn row_to_message(row: &rusqlite::Row) -> rusqlite::Result<ChatMessageRecord> {
    // A malformed images_json or attachments_json degrades to `None` rather
    // than failing the whole row — same choice performance_reports.rs makes
    // for history_json. One corrupt blob shouldn't hide the rest of the message.
    let images_json:      Option<String> = row.get(3)?;
    let attachments_json: Option<String> = row.get(4)?;
    Ok(ChatMessageRecord {
        id:          row.get(0)?,
        role:        row.get(1)?,
        text:        row.get(2)?,
        images:      images_json.and_then(|j| serde_json::from_str(&j).ok()),
        attachments: attachments_json.and_then(|j| serde_json::from_str(&j).ok()),
        timestamp:   row.get(5)?,
    })
}

/// Upsert, never INSERT OR REPLACE: REPLACE deletes the old row first, which
/// cascades through the foreign key to every chat_messages row.
fn upsert_session_meta(conn: &rusqlite::Connection, s: &ChatSessionMeta) -> Result<(), String> {
    conn.execute(
        "INSERT INTO chat_sessions (id, workflow_id, name, created_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
             workflow_id = excluded.workflow_id,
             name        = excluded.name,
             created_at  = excluded.created_at",
        rusqlite::params![s.id, s.workflow_id, s.name, s.created_at],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

/// The `images_json` and `attachments_json` column values for a message.
fn message_blobs(m: &ChatMessageRecord) -> Result<(Option<String>, Option<String>), String> {
    let images = match &m.images {
        Some(imgs) => Some(serde_json::to_string(imgs).map_err(|e| e.to_string())?),
        None => None,
    };
    let attachments = match &m.attachments {
        Some(atts) => Some(serde_json::to_string(atts).map_err(|e| e.to_string())?),
        None => None,
    };
    Ok((images, attachments))
}

impl WorkflowDb {
    /// Sessions for a workflow, newest-created first, without their messages.
    pub fn list_chat_session_meta(&self, workflow_id: &str) -> Result<Vec<ChatSessionMeta>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, name, created_at FROM chat_sessions
             WHERE workflow_id = ?1 ORDER BY created_at DESC"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![workflow_id], |row| {
            Ok(ChatSessionMeta {
                id:          row.get(0)?,
                workflow_id: workflow_id.to_string(),
                name:        row.get(1)?,
                created_at:  row.get(2)?,
            })
        }).map_err(|e| e.to_string())?
          .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
        Ok(rows)
    }

    /// One session's messages in chronological order. An unknown session id
    /// yields an empty list.
    pub fn load_chat_messages(&self, session_id: &str) -> Result<Vec<ChatMessageRecord>, String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, role, text, images_json, attachments_json, timestamp FROM chat_messages
             WHERE session_id = ?1 ORDER BY timestamp ASC"
        ).map_err(|e| e.to_string())?;
        let rows = stmt.query_map(rusqlite::params![session_id], row_to_message)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
        Ok(rows)
    }

    /// Upserts a session's metadata only; its messages are left alone.
    pub fn save_chat_session_meta(&self, session: &ChatSessionMeta) -> Result<(), String> {
        let conn = self.pool.get().map_err(|e| e.to_string())?;
        upsert_session_meta(&conn, session)
    }

    /// Adds one message to a session, creating or updating the session row in
    /// the same transaction so the call needs no earlier setup. Idempotent by
    /// message id: repeating a call rewrites the same row instead of adding a
    /// second one. An id already owned by a different session fails and rolls
    /// back the whole call, session row included.
    pub fn append_chat_message(&self, session: &ChatSessionMeta, message: &ChatMessageRecord) -> Result<(), String> {
        let mut conn = self.pool.get().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        upsert_session_meta(&tx, session)?;

        let (images_json, attachments_json) = message_blobs(message)?;
        let changed = tx.execute(
            "INSERT INTO chat_messages (id, session_id, role, text, images_json, attachments_json, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                 role             = excluded.role,
                 text             = excluded.text,
                 images_json      = excluded.images_json,
                 attachments_json = excluded.attachments_json,
                 timestamp        = excluded.timestamp
             WHERE chat_messages.session_id = excluded.session_id",
            rusqlite::params![message.id, session.id, message.role, message.text,
                              images_json, attachments_json, message.timestamp],
        ).map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("chat message id {} belongs to another session", message.id));
        }
        tx.commit().map_err(|e| e.to_string())
    }

    /// Upserts the session row and syncs its message list to the caller's
    /// full current state, so the caller must send every message the session
    /// has. Only new ids are inserted, only rows whose content differs are
    /// rewritten, and ids the caller no longer sends are deleted. To add one
    /// message to a session, use [`Self::append_chat_message`].
    pub fn save_chat_session(&self, session: &ChatSessionRecord) -> Result<(), String> {
        let mut conn = self.pool.get().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        upsert_session_meta(&tx, &ChatSessionMeta {
            id: session.id.clone(), workflow_id: session.workflow_id.clone(),
            name: session.name.clone(), created_at: session.created_at,
        })?;

        let existing: HashSet<String> = {
            let mut stmt = tx.prepare("SELECT id FROM chat_messages WHERE session_id = ?1")
                .map_err(|e| e.to_string())?;
            let ids = stmt.query_map(rusqlite::params![session.id], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<HashSet<_>, _>>().map_err(|e| e.to_string())?;
            ids
        };

        let incoming: HashSet<&str> = session.messages.iter().map(|m| m.id.as_str()).collect();
        for stale in existing.iter().filter(|id| !incoming.contains(id.as_str())) {
            tx.execute("DELETE FROM chat_messages WHERE id = ?1 AND session_id = ?2",
                       rusqlite::params![stale, session.id])
                .map_err(|e| e.to_string())?;
        }

        for m in &session.messages {
            let (images_json, attachments_json) = message_blobs(m)?;
            if existing.contains(&m.id) {
                tx.execute(
                    "UPDATE chat_messages
                     SET role = ?3, text = ?4, images_json = ?5, attachments_json = ?6, timestamp = ?7
                     WHERE id = ?1 AND session_id = ?2
                       AND (role IS NOT ?3 OR text IS NOT ?4 OR images_json IS NOT ?5
                            OR attachments_json IS NOT ?6 OR timestamp IS NOT ?7)",
                    rusqlite::params![m.id, session.id, m.role, m.text, images_json, attachments_json, m.timestamp],
                ).map_err(|e| e.to_string())?;
            } else {
                tx.execute(
                    "INSERT INTO chat_messages (id, session_id, role, text, images_json, attachments_json, timestamp)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![m.id, session.id, m.role, m.text, images_json, attachments_json, m.timestamp],
                ).map_err(|e| e.to_string())?;
            }
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
    use crate::db::{ChatAttachment, ChatImageFile};
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

    fn list_full(db: &WorkflowDb, workflow_id: &str) -> Vec<ChatSessionRecord> {
        db.list_chat_session_meta(workflow_id).unwrap().into_iter().map(|m| ChatSessionRecord {
            messages: db.load_chat_messages(&m.id).unwrap(),
            id: m.id, workflow_id: m.workflow_id, name: m.name, created_at: m.created_at,
        }).collect()
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
                    text: Some("hi".to_string()), images: None,
                    attachments: Some(vec![ChatAttachment {
                        filename: "notes.pdf".to_string(), data: "Yg==".to_string(), mime_type: "application/pdf".to_string(),
                    }]),
                    timestamp: created_at + 1,
                },
                ChatMessageRecord {
                    id: format!("{id}-m2"), role: "ai".to_string(),
                    text: None,
                    images: Some(vec![ChatImageFile {
                        filename: "a.png".to_string(), data: "YQ==".to_string(), mime_type: "image/png".to_string(),
                    }]),
                    attachments: None,
                    timestamp: created_at + 2,
                },
            ],
        }
    }

    // Normal case: save then list — session metadata and every message field,
    // including the images_json and attachments_json round trips, come back
    // intact and in order; a message with no attachments stays `None`.
    #[test]
    fn save_and_list_chat_session_round_trips_messages_images_and_attachments() {
        let path = temp_path("roundtrip");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");

        db.save_chat_session(&sample_session("s1", "wf-1", 1_000)).expect("save failed");

        let sessions = list_full(&db, "wf-1");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].messages.len(), 2);
        assert_eq!(sessions[0].messages[0].text.as_deref(), Some("hi"));
        assert_eq!(sessions[0].messages[1].images.as_ref().unwrap()[0].filename, "a.png");
        let atts = sessions[0].messages[0].attachments.as_ref().expect("attachments must round-trip");
        assert_eq!((atts[0].filename.as_str(), atts[0].data.as_str(), atts[0].mime_type.as_str()),
                   ("notes.pdf", "Yg==", "application/pdf"));
        assert!(sessions[0].messages[1].attachments.is_none());

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

        let sessions = list_full(&db, "wf-1");
        assert_eq!(sessions.len(), 1, "resave must not create a duplicate session row");
        assert_eq!(sessions[0].messages.len(), 1, "old messages must not survive a resave");

        cleanup(&path);
    }

    // Counts every insert/update/delete on chat_messages so the test can tell
    // "row left alone" from "row rewritten with identical content", which a
    // read-back alone cannot.
    fn install_write_log(db: &WorkflowDb) {
        let conn = db.pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE write_log (op TEXT NOT NULL);
             CREATE TRIGGER wl_i AFTER INSERT ON chat_messages BEGIN INSERT INTO write_log VALUES ('insert'); END;
             CREATE TRIGGER wl_u AFTER UPDATE ON chat_messages BEGIN INSERT INTO write_log VALUES ('update'); END;
             CREATE TRIGGER wl_d AFTER DELETE ON chat_messages BEGIN INSERT INTO write_log VALUES ('delete'); END;",
        ).unwrap();
    }

    fn drain_write_log(db: &WorkflowDb) -> Vec<String> {
        let conn = db.pool.get().unwrap();
        let ops = {
            let mut stmt = conn.prepare("SELECT op FROM write_log ORDER BY rowid").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap()
                .collect::<Result<Vec<_>, _>>().unwrap();
            rows
        };
        conn.execute("DELETE FROM write_log", []).unwrap();
        ops
    }

    // Normal + edge: an unchanged re-save writes nothing; an edit rewrites
    // exactly the edited row; an append inserts exactly one row; a removal
    // deletes exactly one row. The session row upsert must not cascade-wipe
    // messages (an unchanged re-save logging a delete/insert would show it).
    #[test]
    fn save_chat_session_writes_only_changed_messages_on_resave() {
        let path = temp_path("incremental");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");
        let mut session = sample_session("s1", "wf-1", 1_000);
        db.save_chat_session(&session).unwrap();
        install_write_log(&db);

        db.save_chat_session(&session).unwrap();
        assert!(drain_write_log(&db).is_empty(), "identical re-save must not write any message row");

        session.messages[0].text = Some("edited".to_string());
        db.save_chat_session(&session).unwrap();
        assert_eq!(drain_write_log(&db), vec!["update"]);

        session.messages.push(ChatMessageRecord {
            id: "s1-m3".to_string(), role: "user".to_string(), text: Some("more".to_string()),
            images: None, attachments: None, timestamp: 1_003,
        });
        db.save_chat_session(&session).unwrap();
        assert_eq!(drain_write_log(&db), vec!["insert"]);

        session.messages.remove(1);
        db.save_chat_session(&session).unwrap();
        assert_eq!(drain_write_log(&db), vec!["delete"]);

        let stored = list_full(&db, "wf-1");
        let texts: Vec<_> = stored[0].messages.iter().map(|m| (m.id.as_str(), m.text.as_deref())).collect();
        assert_eq!(texts, vec![("s1-m1", Some("edited")), ("s1-m3", Some("more"))]);

        cleanup(&path);
    }

    // Edge: a message id already owned by another session fails the save and
    // the whole transaction rolls back; neither session's stored state changes.
    #[test]
    fn save_chat_session_rejects_message_id_owned_by_another_session() {
        let path = temp_path("id_collision");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");
        db.save_chat_session(&sample_session("s1", "wf-1", 1_000)).unwrap();
        db.save_chat_session(&sample_session("s2", "wf-1", 2_000)).unwrap();

        let mut clash = sample_session("s2", "wf-1", 2_000);
        clash.name = "renamed".to_string();
        clash.messages[0].id = "s1-m1".to_string();
        assert!(db.save_chat_session(&clash).is_err());

        let sessions = list_full(&db, "wf-1");
        let s1 = sessions.iter().find(|s| s.id == "s1").unwrap();
        let s2 = sessions.iter().find(|s| s.id == "s2").unwrap();
        assert_eq!(s1.messages.len(), 2);
        assert_eq!((s2.name.as_str(), s2.messages[0].id.as_str()), ("Session", "s2-m1"));

        cleanup(&path);
    }

    #[test]
    fn delete_chat_session_cascades_its_messages() {
        let path = temp_path("delete");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");

        db.save_chat_session(&sample_session("s1", "wf-1", 1_000)).unwrap();
        db.delete_chat_session("s1").expect("delete failed");

        assert!(list_full(&db, "wf-1").is_empty());
        let conn = db.pool.get().unwrap();
        let leftover: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages WHERE session_id = 's1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(leftover, 0, "messages must cascade-delete with their session");

        cleanup(&path);
    }

    fn sample_message(id: &str, timestamp: i64, text: &str) -> ChatMessageRecord {
        ChatMessageRecord {
            id: id.to_string(), role: "user".to_string(), text: Some(text.to_string()),
            images: None, attachments: None, timestamp,
        }
    }

    fn meta_of(s: &ChatSessionRecord) -> ChatSessionMeta {
        ChatSessionMeta {
            id: s.id.clone(), workflow_id: s.workflow_id.clone(),
            name: s.name.clone(), created_at: s.created_at,
        }
    }

    // Normal + edge: appending to a session that has no row creates it; a
    // repeated call with the same message id (a retry) rewrites that row
    // instead of adding a second one; messages come back in timestamp order.
    #[test]
    fn append_chat_message_creates_session_and_is_idempotent_on_retry() {
        let path = temp_path("append_idempotent");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");
        let meta = meta_of(&sample_session("s1", "wf-1", 1_000));

        db.append_chat_message(&meta, &sample_message("m2", 1_002, "second")).unwrap();
        db.append_chat_message(&meta, &sample_message("m1", 1_001, "first")).unwrap();
        db.append_chat_message(&meta, &sample_message("m1", 1_001, "first")).unwrap();

        let metas = db.list_chat_session_meta("wf-1").unwrap();
        assert_eq!(metas.len(), 1);
        let msgs = db.load_chat_messages("s1").unwrap();
        assert_eq!(msgs.iter().map(|m| (m.id.as_str(), m.text.as_deref())).collect::<Vec<_>>(),
                   vec![("m1", Some("first")), ("m2", Some("second"))]);

        cleanup(&path);
    }

    // Edge: a message id owned by another session fails the append and rolls
    // back the whole call; the rejected call's session rename does not stick
    // and neither session's stored messages change.
    #[test]
    fn append_chat_message_rejects_id_owned_by_another_session_and_rolls_back() {
        let path = temp_path("append_collision");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");
        db.save_chat_session(&sample_session("s1", "wf-1", 1_000)).unwrap();
        db.save_chat_session(&sample_session("s2", "wf-1", 2_000)).unwrap();

        let mut meta = meta_of(&sample_session("s2", "wf-1", 2_000));
        meta.name = "renamed".to_string();
        assert!(db.append_chat_message(&meta, &sample_message("s1-m1", 3_000, "stolen")).is_err());

        let metas = db.list_chat_session_meta("wf-1").unwrap();
        assert_eq!(metas.iter().find(|m| m.id == "s2").unwrap().name, "Session");
        let s1 = db.load_chat_messages("s1").unwrap();
        assert_eq!(s1.iter().find(|m| m.id == "s1-m1").unwrap().text.as_deref(), Some("hi"));
        assert_eq!(db.load_chat_messages("s2").unwrap().len(), 2);

        cleanup(&path);
    }

    // Normal + edge: the metadata save creates an empty session, renames an
    // existing one, and never touches its messages (an INSERT OR REPLACE
    // would cascade-delete them).
    #[test]
    fn save_chat_session_meta_keeps_existing_messages() {
        let path = temp_path("meta_only");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");

        let empty = ChatSessionMeta {
            id: "empty".to_string(), workflow_id: "wf-1".to_string(),
            name: "Empty".to_string(), created_at: 500,
        };
        db.save_chat_session_meta(&empty).unwrap();
        assert!(db.load_chat_messages("empty").unwrap().is_empty());

        db.save_chat_session(&sample_session("s1", "wf-1", 1_000)).unwrap();
        let mut renamed = meta_of(&sample_session("s1", "wf-1", 1_000));
        renamed.name = "Renamed".to_string();
        db.save_chat_session_meta(&renamed).unwrap();

        assert_eq!(db.load_chat_messages("s1").unwrap().len(), 2);
        let metas = db.list_chat_session_meta("wf-1").unwrap();
        assert_eq!(metas.iter().map(|m| (m.id.as_str(), m.name.as_str())).collect::<Vec<_>>(),
                   vec![("s1", "Renamed"), ("empty", "Empty")]);

        cleanup(&path);
    }

    // Normal + edge: the metadata listing is newest-first and scoped to the
    // workflow, and loading one session returns only that session's messages.
    #[test]
    fn list_chat_session_meta_and_load_chat_messages_are_scoped() {
        let path = temp_path("meta_scope");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");
        db.save_chat_session(&sample_session("s1", "wf-1", 1_000)).unwrap();
        db.save_chat_session(&sample_session("s2", "wf-1", 2_000)).unwrap();
        db.save_chat_session(&sample_session("s3", "wf-2", 3_000)).unwrap();

        let ids: Vec<_> = db.list_chat_session_meta("wf-1").unwrap().into_iter().map(|m| m.id).collect();
        assert_eq!(ids, vec!["s2", "s1"]);
        assert_eq!(db.list_chat_session_meta("wf-2").unwrap().len(), 1);
        assert!(db.list_chat_session_meta("no-such-workflow").unwrap().is_empty());

        let s1 = db.load_chat_messages("s1").unwrap();
        assert_eq!(s1.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["s1-m1", "s1-m2"]);
        assert!(db.load_chat_messages("no-such-session").unwrap().is_empty());

        cleanup(&path);
    }

    // Measurement: JSON bytes for 20 messages of ~200 KB each — the metadata
    // listing versus metadata plus every session's messages. Run with
    // --nocapture to print the numbers.
    #[test]
    fn chat_session_listing_is_metadata_sized_not_message_sized() {
        let path = temp_path("listing_bytes");
        cleanup(&path);
        let db = WorkflowDb::open(&path, 8).expect("open failed");
        let blob = "A".repeat(200 * 1024);
        let mut session = sample_session("big", "wf-1", 1_000);
        session.messages = (0..20).map(|i| ChatMessageRecord {
            id: format!("big-m{i}"), role: "ai".to_string(), text: None,
            images: Some(vec![ChatImageFile {
                filename: format!("{i}.png"), data: blob.clone(), mime_type: "image/png".to_string(),
            }]),
            attachments: None, timestamp: 1_001 + i,
        }).collect();
        db.save_chat_session(&session).unwrap();

        let full = serde_json::to_vec(&list_full(&db, "wf-1")).unwrap().len();
        let meta = serde_json::to_vec(&db.list_chat_session_meta("wf-1").unwrap()).unwrap().len();
        let one  = serde_json::to_vec(&db.load_chat_messages("big").unwrap()).unwrap().len();
        eprintln!("chat listing bytes: full={full} meta={meta} one-session-load={one}");

        assert!(full > 4_000_000, "fixture must actually carry ~4 MB of blobs, got {full}");
        assert!(meta < 500, "metadata listing must not scale with message size, got {meta}");
        assert!(one > 4_000_000 && one < full, "one-session load carries the blobs but not the list wrapper");

        cleanup(&path);
    }
}
