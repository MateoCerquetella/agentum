//! Events: the persisted activity log that backs the dashboard's watchdog
//! feed and the cold-start activity overlay.

use crate::{Result, Store, StoreError};
use agentum_core::Event;
use sqlx::FromRow;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

impl Store {
    /// Newest-first list of recent watchdog-eligible events. Filters
    /// for kinds the dashboard's watchdog feed renders (`watchdog.*`
    /// and `session.crashed` / `session.started` etc.). `limit` caps
    /// the result; pass 50 for the default cold-start payload.
    pub async fn list_watchdog_events(&self, limit: i64) -> Result<Vec<Event>> {
        let rows: Vec<EventRow> = sqlx::query_as::<_, EventRow>(
            "SELECT session_id, kind, payload, ts FROM events
             WHERE kind LIKE 'watchdog.%' OR kind LIKE 'session.%'
             ORDER BY ts DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Event::try_from).collect()
    }

    /// One row per session: the most-recent `agent.*` event for each.
    /// Used by the `/api/events` WS handler to bootstrap a fresh client
    /// with the current activity overlay (idle / awaiting input /
    /// working) before live events start streaming. Without this a
    /// dashboard tab opened mid-flight has no way to tell that a
    /// `running` session's agent has already finished its turn —
    /// `agent.finished` only fires once per transition and isn't
    /// replayed on the bus.
    pub async fn latest_agent_event_per_session(&self) -> Result<Vec<Event>> {
        let rows: Vec<EventRow> = sqlx::query_as::<_, EventRow>(
            "SELECT e.session_id, e.kind, e.payload, e.ts FROM events e
             INNER JOIN (
                 SELECT session_id, MAX(id) AS max_id FROM events
                 WHERE session_id IS NOT NULL AND kind LIKE 'agent.%'
                 GROUP BY session_id
             ) latest ON latest.max_id = e.id
             ORDER BY e.ts ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Event::try_from).collect()
    }

    /// Persist an event row. Best-effort (failures should not break callers).
    pub async fn insert_event(&self, ev: &Event) -> Result<()> {
        let payload = serde_json::to_string(&ev.payload)?;
        let ts = ev.ts.format(&Rfc3339)?;
        sqlx::query("INSERT INTO events (session_id, kind, payload, ts) VALUES (?, ?, ?, ?)")
            .bind(ev.session_id.map(|u| u.to_string()))
            .bind(&ev.kind)
            .bind(payload)
            .bind(ts)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[derive(Debug, FromRow)]
struct EventRow {
    session_id: Option<String>,
    kind: String,
    payload: String,
    ts: String,
}

impl TryFrom<EventRow> for Event {
    type Error = StoreError;
    fn try_from(r: EventRow) -> Result<Self> {
        Ok(Event {
            kind: r.kind,
            session_id: r.session_id.as_deref().map(Uuid::parse_str).transpose()?,
            // session_name is not persisted in the events table; the SSE
            // path passes it through directly. Cold-start GETs leave it
            // None and the watchdog projection falls back to session_id.
            session_name: None,
            payload: serde_json::from_str(&r.payload).unwrap_or(serde_json::Value::Null),
            ts: OffsetDateTime::parse(&r.ts, &Rfc3339)?,
        })
    }
}
