#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use aerini_mcp::{AerinMcp, Config};
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    response::Response,
    Router,
};
use rmcp::model::CallToolResult;
use serde_json::Value;

pub const TOKEN: &str = "test-token";

#[derive(Clone)]
pub struct Canned {
    pub method: &'static str,
    pub path: &'static str,
    pub status: u16,
    pub headers: Vec<(&'static str, &'static str)>,
    pub body: String,
    pub delay_ms: u64,
}

pub fn reply(method: &'static str, path: &'static str, status: u16, body: &str) -> Canned {
    Canned { method, path, status, headers: Vec::new(), body: body.to_string(), delay_ms: 0 }
}

impl Canned {
    pub fn header(mut self, name: &'static str, value: &'static str) -> Self {
        self.headers.push((name, value));
        self
    }

    pub fn delayed(mut self, ms: u64) -> Self {
        self.delay_ms = ms;
        self
    }
}

#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub query: String,
    pub authorization: Option<String>,
    pub if_match: Option<String>,
    pub body: String,
}

struct Inner {
    routes: Vec<Canned>,
    seen: Mutex<Vec<Seen>>,
}

pub struct Mock {
    pub base: String,
    inner: Arc<Inner>,
}

impl Mock {
    pub fn seen(&self) -> Vec<Seen> {
        self.inner.seen.lock().unwrap().clone()
    }
}

async fn handle(State(inner): State<Arc<Inner>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = to_bytes(body, 1 << 20).await.unwrap_or_default();
    let header = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let seen = Seen {
        method: parts.method.to_string(),
        path: parts.uri.path().to_string(),
        query: parts.uri.query().unwrap_or_default().to_string(),
        authorization: header("authorization"),
        if_match: header("if-match"),
        body: String::from_utf8_lossy(&bytes).to_string(),
    };
    let canned = inner
        .routes
        .iter()
        .find(|c| c.method == seen.method && c.path == seen.path)
        .cloned();
    inner.seen.lock().unwrap().push(seen);
    let Some(canned) = canned else {
        return Response::builder().status(404).body(Body::empty()).unwrap();
    };
    if canned.delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(canned.delay_ms)).await;
    }
    let mut builder = Response::builder().status(canned.status).header("content-type", "application/json");
    for (name, value) in &canned.headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::from(canned.body)).unwrap()
}

pub async fn serve(routes: Vec<Canned>) -> Mock {
    let inner = Arc::new(Inner { routes, seen: Mutex::new(Vec::new()) });
    let app = Router::new().fallback(handle).with_state(Arc::clone(&inner));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Mock { base, inner }
}

pub fn config(base: &str, allow_write: bool) -> Config {
    Config {
        url: Some(base.to_string()),
        token: Some(TOKEN.to_string()),
        allow_write,
        timeout: Duration::from_secs(5),
        max_output_bytes: 1024,
        out_dir: None,
    }
}

pub fn server(mock: &Mock, allow_write: bool) -> AerinMcp {
    AerinMcp::new(config(&mock.base, allow_write)).unwrap()
}

pub struct Outcome {
    pub is_error: bool,
    pub text: String,
}

impl Outcome {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.text))
    }
}

pub fn outcome(result: Result<CallToolResult, rmcp::ErrorData>) -> Outcome {
    let wire = serde_json::to_value(result.expect("tool call must not be a protocol error")).unwrap();
    Outcome {
        is_error: wire["isError"].as_bool().unwrap_or(false),
        text: wire["content"][0]["text"].as_str().unwrap_or_default().to_string(),
    }
}
