//! Shared domain types for agentum.
//!
//! Kept dependency-light so every crate can pull this in without inheriting
//! a database, runtime, or HTTP stack.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use time::OffsetDateTime;
use uuid::Uuid;

pub mod board_schema;
pub mod profiles;
pub mod transcript;

pub use board_schema::{
    RequiredField, TransitionCtx, required_fields_for, validate_against, validate_transition,
};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("invalid session status: {0}")]
    InvalidStatus(String),
    #[error("invalid session name: {0}")]
    InvalidName(String),
    #[error("invalid link kind: {0}")]
    ParseLinkKind(String),
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

    // Observability fields — populated by the watchdog as it monitors
    // the agent. Optional; the HTTP layer skips `None` to keep the wire
    // format compact.
    /// Cumulative tokens consumed by this session (input + output).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<i64>,
    /// USD spend, summed from per-tool pricing.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "cost")]
    pub cost_usd: Option<f64>,
    /// 0–100, percentage of context window remaining.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctx: Option<i32>,
    /// Tail of stdout — used as the FleetRow "last activity" cell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_log: Option<String>,
    /// Override for time-since-creation when the agent reports a more
    /// accurate "since this pane started" figure. The web client
    /// falls back to `(now - created_at)` when this is None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<i64>,
    /// Watchdog-aware lifecycle state (`live`/`idle`/`compact`/`crash`).
    /// Independent of `status` because /compact suspends a session
    /// without stopping its tmux pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<SessionState>,
    /// User-toggled "favorite" flag. Pinned sessions float to the top
    /// of every list view. Defaults to false; persisted to sqlite via
    /// migration 0009.
    #[serde(default)]
    pub pinned: bool,
    /// Board card this session is bound to. Set when the planner spawns
    /// a session for a goal card (session.card_id = goal.id); NULL for
    /// all other sessions. Added in migration 0015.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_id: Option<i64>,
}

/// Lifecycle state surfaced on the dashboard. Mirrors the TypeScript
/// `SessionState` so a server-pushed value flows directly to the
/// state-dot color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Live,
    Idle,
    Compact,
    Crash,
}

impl SessionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Idle => "idle",
            Self::Compact => "compact",
            Self::Crash => "crash",
        }
    }
}

impl std::str::FromStr for SessionState {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "live" => Ok(Self::Live),
            "idle" => Ok(Self::Idle),
            "compact" => Ok(Self::Compact),
            "crash" => Ok(Self::Crash),
            other => Err(CoreError::InvalidStatus(other.to_string())),
        }
    }
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
    /// Board card to bind this session to. Used by the planner spawn
    /// path (POST /api/board/goals) to record which goal card spawned
    /// this planning session. NULL for all other sessions.
    #[serde(default)]
    pub card_id: Option<i64>,
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

    /// Project a server `Event` into the dashboard's compact wire shape.
    /// Returns `None` for events the watchdog feed shouldn't display.
    pub fn to_watchdog(&self) -> Option<WatchdogEvent> {
        let kind = if let Some(rest) = self.kind.strip_prefix("watchdog.") {
            match rest {
                "compact" => WatchdogKind::Compact,
                "crash" => WatchdogKind::Crash,
                "warn" => WatchdogKind::Warn,
                _ => WatchdogKind::Ok,
            }
        } else if self.kind == "session.crashed" {
            WatchdogKind::Crash
        } else if self.kind.starts_with("session.") {
            WatchdogKind::Ok
        } else {
            return None;
        };
        // Prefer payload.label/msg when the watchdog publisher provides
        // them; otherwise synthesize from the dotted kind.
        let label = self
            .payload
            .get("label")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.kind.rsplit('.').next().unwrap_or("event").to_string());
        let msg = self
            .payload
            .get("msg")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| match (&self.session_name, &self.session_id) {
                (Some(n), _) => format!("{n} · {}", self.kind),
                (None, Some(id)) => format!("{id} · {}", self.kind),
                _ => self.kind.clone(),
            });
        Some(WatchdogEvent {
            ts: self.ts,
            kind,
            label,
            msg,
            ses: self.session_name.clone(),
        })
    }
}

/// Watchdog-feed event shape consumed by the dashboard. Compact subset
/// of the server `Event`. Mirrors the TS `WatchdogEvent`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchdogEvent {
    #[serde(with = "time::serde::rfc3339")]
    pub ts: OffsetDateTime,
    pub kind: WatchdogKind,
    pub label: String,
    pub msg: String,
    /// Originating session name (nullable for fleet-level events).
    pub ses: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WatchdogKind {
    Ok,
    Warn,
    Compact,
    Crash,
}

// ---------- board items ----------

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
    /// Ticket type — colors the foot label pill.
    /// One of: `bug` | `feat` | `chore` | `spike`. Free-form String so
    /// users can introduce custom labels without a schema change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lbl: Option<String>,
    /// Tool ecosystem — colors the foot dot.
    /// `claude` | `codex` | `gemini` | `hermes` are the recognized values;
    /// others fall back to neutral grey.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Working directory the agent should run in. Carried so a ticket
    /// can be spawned into a session without re-asking the user where
    /// the work lives. Free-form absolute path; the server doesn't
    /// validate that it exists (the agent itself will surface that).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Optional model hint (e.g. `claude-opus-4-7`, `gpt-5`). Passes
    /// through to the spawned session's `--model` flag when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// agentum session id that this ticket is being worked in. Set when
    /// the user spawns a session from the ticket; nullable so cards
    /// without an active session can still render the Start affordance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Manual ordering within a column. Lower priority floats to the top
    /// of the column; drag-to-reorder rewrites this on the affected
    /// column. Default 0 — fresh rows append at the bottom because the
    /// secondary sort is `created_at ASC`.
    #[serde(default)]
    pub priority: i64,
    /// Goal card this item is a child of. Set when the planner creates a
    /// child card under a goal (lbl="goal") card; NULL for standalone
    /// cards and for the goal card itself. Added in migration 0015.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_goal_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewBoardItem {
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub lbl: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub priority: Option<i64>,
    /// Goal card this child item belongs to. Set by the planner when
    /// creating child cards under a goal. NULL for standalone items and
    /// the goal card itself.
    #[serde(default)]
    pub parent_goal_id: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BoardPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub body: Option<Option<String>>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub lbl: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub tool: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub workdir: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub model: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub session_id: Option<Option<String>>,
    #[serde(default)]
    pub priority: Option<i64>,
    /// Double-Option per the existing pattern: `None` = field omitted
    /// (no-op), `Some(None)` = detach from goal (set NULL), `Some(Some(id))`
    /// = attach or re-attach to a different goal. This is critical for
    /// letting the planner watchdog detach a child from its goal when the
    /// goal is completed or the child is promoted.
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub parent_goal_id: Option<Option<i64>>,
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

/// Threaded comment on a board item. `author` is free-form so both
/// human actors (`web-…`) and agent ids can post without a separate
/// users table. Comments are render-only — no edit/delete endpoints
/// yet; that's a future ask if the audit-trail vibe takes off.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardComment {
    pub id: i64,
    pub board_id: i64,
    pub author: String,
    pub body: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewBoardComment {
    pub author: String,
    pub body: String,
}

/// Batch reorder payload. The dashboard sends one of these per drag-
/// to-reorder drop; the server writes them in a single transaction so
/// the column is always consistent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReorderEntry {
    pub id: i64,
    pub priority: i64,
}

// ---------- board links ----------

/// Kind of directed edge between two board items. Stored as a TEXT
/// column in `board_links` via `as_str()` / `FromStr`.
///
/// `ParentOf` records the planner-created parent→child relationship
/// (the parent goal "owns" the child card). `Blocks` records a
/// dependency edge: the `from_card_id` card must complete before the
/// `to_card_id` card can start (Phase 3 gate enforces this on PATCH).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    ParentOf,
    Blocks,
}

impl LinkKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            LinkKind::ParentOf => "parent_of",
            LinkKind::Blocks => "blocks",
        }
    }
}

impl fmt::Display for LinkKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LinkKind {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "parent_of" => Ok(LinkKind::ParentOf),
            "blocks" => Ok(LinkKind::Blocks),
            other => Err(CoreError::ParseLinkKind(other.to_string())),
        }
    }
}

/// A directed edge between two board items persisted in `board_links`.
/// `from_card_id → to_card_id` with a `kind` discriminator. The
/// `created_at` is immutable — links are insert-or-delete only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardLink {
    pub from_card_id: i64,
    pub to_card_id: i64,
    pub kind: LinkKind,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

// ---------- notes ----------

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

// ---------- channels + messages ----------

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

    /// Confirms that `BoardItem.parent_goal_id` round-trips through
    /// serde_json correctly for the three cases:
    ///   1. `Some(42)` — present and serialises as `"parent_goal_id": 42`.
    ///   2. `None` — absent (skip_serializing_if omits the key).
    ///   3. Missing from input — deserialises to `None` via `#[serde(default)]`.
    #[test]
    fn parent_goal_id_round_trips_via_serde() {
        // Case 1: Some(42) — round-trips through the JSON wire.
        let json_with = r#"{"id":1,"key":"AG-1","title":"t","status":"todo","created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z","parent_goal_id":42}"#;
        let item: BoardItem = serde_json::from_str(json_with).unwrap();
        assert_eq!(item.parent_goal_id, Some(42));
        let reserialized = serde_json::to_string(&item).unwrap();
        assert!(
            reserialized.contains("\"parent_goal_id\":42"),
            "expected parent_goal_id in output: {reserialized}"
        );

        // Case 2: None — skip_serializing_if omits the key entirely.
        let json_without = r#"{"id":2,"key":"AG-2","title":"t","status":"todo","created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z"}"#;
        let item2: BoardItem = serde_json::from_str(json_without).unwrap();
        assert_eq!(item2.parent_goal_id, None);
        let reserialized2 = serde_json::to_string(&item2).unwrap();
        assert!(
            !reserialized2.contains("parent_goal_id"),
            "expected parent_goal_id absent from output: {reserialized2}"
        );
    }

    /// Confirms that `Session.card_id` round-trips analogously to
    /// `BoardItem.parent_goal_id` (same serde attribute pattern).
    ///
    /// Uses `serde_json::to_value` + field surgery rather than a
    /// hand-written JSON string because `Session` has required RFC3339
    /// timestamp fields (`last_activity_at` etc.) that would make the
    /// fixture brittle. Instead we construct a canonical `Session` and
    /// verify the field presence/absence in the serialised output.
    #[test]
    fn card_id_round_trips_via_serde() {
        let base = Session {
            id: uuid::Uuid::nil(),
            name: "s".into(),
            workdir: "/".into(),
            tool: "claude".into(),
            model: None,
            flags: vec![],
            status: Status::Idle,
            tmux_target: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            last_activity_at: None,
            tokens: None,
            cost_usd: None,
            ctx: None,
            last_log: None,
            uptime_seconds: None,
            state: None,
            pinned: false,
            card_id: None,
        };

        // card_id = None → absent from wire.
        let re = serde_json::to_string(&base).unwrap();
        assert!(
            !re.contains("card_id"),
            "expected card_id absent from output: {re}"
        );

        // card_id = Some(7) → present on wire.
        let with_card = Session {
            card_id: Some(7),
            ..base.clone()
        };
        let re2 = serde_json::to_string(&with_card).unwrap();
        assert!(
            re2.contains("\"card_id\":7"),
            "expected card_id in output: {re2}"
        );

        // Round-trip through JSON: decode back and compare.
        let decoded: Session = serde_json::from_str(&re2).unwrap();
        assert_eq!(decoded.card_id, Some(7));
    }

    /// Exercises the three distinct serde states of
    /// `BoardPatch.parent_goal_id`:
    ///   - Field absent → `None` (no-op, leave column alone).
    ///   - Field = `null` → `Some(None)` (detach child from goal).
    ///   - Field = number → `Some(Some(id))` (set or change goal).
    #[test]
    fn board_patch_parent_goal_id_distinguishes_omitted_explicit_null_and_value() {
        // Omitted → None (the store should ignore the field).
        let omitted: BoardPatch = serde_json::from_str(r#"{}"#).unwrap();
        assert!(
            omitted.parent_goal_id.is_none(),
            "omitted field must be None"
        );

        // Explicit null → Some(None) (the store should clear the column).
        let explicit_null: BoardPatch =
            serde_json::from_str(r#"{"parent_goal_id": null}"#).unwrap();
        assert_eq!(
            explicit_null.parent_goal_id,
            Some(None),
            "explicit null must be Some(None)"
        );

        // Number → Some(Some(42)) (the store should write 42).
        let with_value: BoardPatch = serde_json::from_str(r#"{"parent_goal_id": 42}"#).unwrap();
        assert_eq!(
            with_value.parent_goal_id,
            Some(Some(42)),
            "value must be Some(Some(42))"
        );
    }

    /// Verifies `LinkKind` as_str / FromStr / unknown-string error path.
    #[test]
    fn link_kind_round_trips() {
        assert_eq!(LinkKind::ParentOf.as_str(), "parent_of");
        assert_eq!(LinkKind::Blocks.as_str(), "blocks");

        assert_eq!("parent_of".parse::<LinkKind>().unwrap(), LinkKind::ParentOf);
        assert_eq!("blocks".parse::<LinkKind>().unwrap(), LinkKind::Blocks);

        let err = "nope".parse::<LinkKind>();
        assert!(
            err.is_err(),
            "unknown string should produce ParseLinkKind error"
        );
        assert!(matches!(err.unwrap_err(), CoreError::ParseLinkKind(_)));
    }
}
