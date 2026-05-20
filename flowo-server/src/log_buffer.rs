use chrono::Utc;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level:     String,
    pub node_id:   Option<String>,
    pub message:   String,
}

#[derive(Clone)]
pub struct LogBuffer {
    inner:    Arc<RwLock<VecDeque<LogEntry>>>,
    capacity: usize,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner:    Arc::new(RwLock::new(VecDeque::with_capacity(capacity))),
            capacity,
        }
    }

    pub fn push(&self, level: &str, node_id: Option<&str>, message: impl Into<String>) {
        let entry = LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            level:     level.to_string(),
            node_id:   node_id.map(|s| s.to_string()),
            message:   message.into(),
        };
        let mut buf = self.inner.write().expect("log_buffer RwLock poisoned");
        if buf.len() >= self.capacity {
            buf.pop_front();
        }
        buf.push_back(entry);
    }

    /// Return the last `n` entries in chronological order.
    pub fn last_n(&self, n: usize) -> Vec<LogEntry> {
        let buf = self.inner.read().expect("log_buffer RwLock poisoned");
        let skip = buf.len().saturating_sub(n);
        buf.iter().skip(skip).cloned().collect()
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.inner.read().expect("log_buffer RwLock poisoned").len()
    }
}
