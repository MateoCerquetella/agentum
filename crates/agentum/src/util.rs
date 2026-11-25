//! Shared helpers: the error type every handler returns, a small TTL cache,
//! and the SQLite handle.
use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::{json, Value};
use std::{collections::HashMap, future::Future, sync::{Arc, Mutex}, time::{Duration, Instant}};

pub struct AppError(pub StatusCode, pub String);
pub type R<T> = Result<T, AppError>;

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        let e: anyhow::Error = e.into();
        tracing::warn!("{e:#}");
        AppError(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
    }
}
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}
pub fn bad(msg: impl Into<String>) -> AppError { AppError(StatusCode::BAD_REQUEST, msg.into()) }
pub fn not_found() -> AppError { AppError(StatusCode::NOT_FOUND, "not found".into()) }

/// One SQLite connection behind a mutex; calls run on the blocking pool.
#[derive(Clone)]
pub struct Db(Arc<Mutex<rusqlite::Connection>>);
impl Db {
    pub fn open(path: &str) -> anyhow::Result<Db> {
        let c = rusqlite::Connection::open(path)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        Ok(Db(Arc::new(Mutex::new(c))))
    }
    pub async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut rusqlite::Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let db = self.0.clone();
        Ok(tokio::task::spawn_blocking(move || {
            let mut c = db.lock().unwrap_or_else(|p| p.into_inner());
            f(&mut c)
        })
        .await??)
    }
    pub async fn kv_get(&self, k: &str) -> anyhow::Result<Option<Value>> {
        let k = k.to_string();
        self.call(move |c| {
            use rusqlite::OptionalExtension;
            c.query_row("SELECT v FROM kv WHERE k=?1", [k], |r| r.get::<_, String>(0)).optional()
        })
        .await
        .map(|o| o.and_then(|s| serde_json::from_str(&s).ok()))
    }
    pub async fn kv_set(&self, k: &str, v: &Value) -> anyhow::Result<()> {
        let (k, v) = (k.to_string(), v.to_string());
        self.call(move |c| c.execute("INSERT INTO kv(k,v) VALUES(?1,?2) ON CONFLICT(k) DO UPDATE SET v=excluded.v", [k, v]).map(|_| ())).await
    }
}

/// TTL cache for upstream fetches (feeds, quotes, weather).
#[derive(Clone, Default)]
pub struct Cache(Arc<Mutex<HashMap<String, (Instant, Value)>>>);
impl Cache {
    pub async fn get_or<F, Fut>(&self, key: &str, ttl: Duration, f: F) -> anyhow::Result<Value>
    where F: FnOnce() -> Fut, Fut: Future<Output = anyhow::Result<Value>> {
        if let Some((t, v)) = self.0.lock().unwrap().get(key) {
            if t.elapsed() < ttl { return Ok(v.clone()); }
        }
        match f().await {
            Ok(v) => {
                let mut m = self.0.lock().unwrap();
                if m.len() > 500 { m.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(3600)); }
                m.insert(key.to_string(), (Instant::now(), v.clone()));
                Ok(v)
            }
            // Serve stale data rather than an error when an upstream hiccups.
            Err(e) => match self.0.lock().unwrap().get(key) { Some((_, v)) => Ok(v.clone()), None => Err(e) },
        }
    }
    pub fn forget(&self, prefix: &str) { self.0.lock().unwrap().retain(|k, _| !k.starts_with(prefix)); }
}

pub fn now_ts() -> i64 { chrono::Utc::now().timestamp() }
pub fn s(v: &Value, k: &str) -> String { v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string() }
