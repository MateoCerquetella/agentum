//! Shared domain types for agentum.
//!
//! Kept dependency-light so every crate can pull this in without inheriting
//! a database, runtime, or HTTP stack.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("invalid session status: {0}")]
    InvalidStatus(String),
    #[error("invalid session name: {0}")]
    InvalidName(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Idle,
    Running,
    Stopped,
    Crashed,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Idle => "idle",
            Status::Running => "running",
            Status::Stopped => "stopped",
            Status::Crashed => "crashed",
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Status {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "idle" => Ok(Status::Idle),
            "running" => Ok(Status::Running),
            "stopped" => Ok(Status::Stopped),
            "crashed" => Ok(Status::Crashed),
            other => Err(CoreError::InvalidStatus(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: Uuid,
    pub name: String,
    pub workdir: String,
    pub tool: String,
    pub model: Option<String>,
    pub flags: Vec<String>,
    pub status: Status,
    pub tmux_target: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_activity_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewSession {
    pub name: String,
    pub workdir: String,
    pub tool: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub flags: Vec<String>,
}

/// Event published on the broadcast bus and persisted to the `events` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Dotted kind, e.g. `watchdog.compact`, `session.crashed`, `session.started`.
    pub kind: String,
    pub session_id: Option<Uuid>,
    pub session_name: Option<String>,
    /// Optional structured detail. Empty object when no payload.
    pub payload: serde_json::Value,
    #[serde(with = "time::serde::rfc3339")]
    pub ts: OffsetDateTime,
}

impl Event {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            session_id: None,
            session_name: None,
            payload: serde_json::Value::Object(Default::default()),
            ts: OffsetDateTime::now_utc(),
        }
    }

    pub fn with_session(mut self, id: Uuid, name: impl Into<String>) -> Self {
        self.session_id = Some(id);
        self.session_name = Some(name.into());
        self
    }

    pub fn with_payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = payload;
        self
    }
}

// ---------- board items (Phase 7) ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardItem {
    pub id: i64,
    /// Human-friendly identifier, e.g. `AG-7`. Derived from `id` at insert.
    pub key: String,
    pub title: String,
    pub body: Option<String>,
    /// Free-form column name. `todo` | `doing` | `done` are the conventional
    /// columns; users can introduce custom ones.
    pub status: String,
    /// Identifier of whoever currently holds the item. Free-form so both
    /// agent sessions (a session id) and web actors (`web-…`) can claim.
    pub claimed_by: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewBoardItem {
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BoardPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub body: Option<Option<String>>,
    #[serde(default)]
    pub status: Option<String>,
}

/// Distinguishes "field omitted" from "field set to null" so a PATCH can
/// explicitly clear `body`.
fn deserialize_optional_field<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::<T>::deserialize(de)?))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimRequest {
    /// Free-form actor identifier. Server stores it in `claimed_by` as-is.
    pub claimed_by: String,
}

// ---------- notes (Phase 8) ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewNote {
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotePatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

// ---------- channels + messages (Phase 8) ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: i64,
    pub a_session: Uuid,
    pub b_session: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewChannel {
    pub a_session: Uuid,
    pub b_session: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub channel_id: i64,
    pub sender: String,
    pub body: String,
    #[serde(with = "time::serde::rfc3339")]
    pub ts: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMessage {
    pub sender: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Validate a username: lowercase alphanumeric, dash, underscore, dot. 1..=64 chars.
pub fn validate_username(name: &str) -> Result<(), CoreError> {
    if name.is_empty() || name.len() > 64 {
        return Err(CoreError::InvalidName(name.to_string()));
    }
    let ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
    if !ok {
        return Err(CoreError::InvalidName(name.to_string()));
    }
    Ok(())
}

/// Validate a session name: lowercase alphanumeric, dash, underscore. 1..=64 chars.
pub fn validate_name(name: &str) -> Result<(), CoreError> {
    if name.is_empty() || name.len() > 64 {
        return Err(CoreError::InvalidName(name.to_string()));
    }
    let ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        return Err(CoreError::InvalidName(name.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_roundtrip() {
        for s in ["idle", "running", "stopped", "crashed"] {
            let parsed: Status = s.parse().unwrap();
            assert_eq!(parsed.as_str(), s);
        }
    }

    #[test]
    fn name_validation() {
        assert!(validate_name("alpha-1").is_ok());
        assert!(validate_name("ALPHA").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("has space").is_err());
        assert!(validate_name("dot.dot").is_err());
    }
}
