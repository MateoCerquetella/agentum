//! `agentum orchestration` — inter-agent mail, task DAG, and dispatch over
//! `/api/orchestration/*`. Reaches the desktop's embedded server inside a pane
//! via `$AGENTUM_API_URL`. `--from`/`--terminal` default to the pane's
//! `$AGENTUM_TERMINAL_HANDLE` (the session name) so an agent can address mail
//! without knowing its own handle.

use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};

use crate::http::ApiClient;

/// Resolve the caller's own handle: an explicit flag, else the pane env, else
/// a placeholder (the server still records it; mail just looks anonymous).
fn self_handle(explicit: Option<String>) -> String {
    explicit
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("AGENTUM_TERMINAL_HANDLE").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "unknown".to_string())
}

fn print_value(json: bool, human: impl FnOnce(&Value), v: &Value) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(v)?);
    } else {
        human(v);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn send(
    to: String,
    subject: String,
    from: Option<String>,
    body: Option<String>,
    msg_type: Option<String>,
    priority: Option<String>,
    thread_id: Option<String>,
    payload: Option<String>,
    json: bool,
) -> Result<()> {
    let client = ApiClient::from_env();
    let payload_json: Option<Value> = match payload {
        Some(p) => Some(serde_json::from_str(&p).unwrap_or(Value::String(p))),
        None => None,
    };
    let body_req = json!({
        "to": to,
        "subject": subject,
        "from": self_handle(from),
        "body": body,
        "type": msg_type,
        "priority": priority,
        "thread_id": thread_id,
        "payload": payload_json,
    });
    let resp = client
        .post_json("/api/orchestration/messages", &body_req)
        .await?;
    print_value(
        json,
        |v| {
            let n = v.get("delivered").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
            let thread = v.get("thread_id").and_then(Value::as_str).unwrap_or("");
            println!("sent to {n} recipient(s) — thread {thread}");
        },
        &resp,
    )
}

pub async fn check(
    terminal: Option<String>,
    types: Vec<String>,
    no_mark_read: bool,
    wait: bool,
    timeout_ms: u64,
    json: bool,
) -> Result<()> {
    let client = ApiClient::from_env();
    let recipient = self_handle(terminal);
    // `check` is the "show me my new mail" command: always unread-only, and
    // consume (mark read) by default so a second check doesn't re-show the same
    // messages. Use `inbox` for a non-consuming view of everything.
    let body = json!({
        "recipient": recipient,
        "unread": true,
        "types": types,
        "mark_read": !no_mark_read,
    });

    let mut waited = 0u64;
    let step = 1000u64;
    loop {
        let resp = client.post_json("/api/orchestration/check", &body).await?;
        let count = resp
            .get("messages")
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0);
        if count > 0 || !wait || waited >= timeout_ms {
            return print_value(json, print_messages, &resp);
        }
        tokio::time::sleep(Duration::from_millis(step.min(timeout_ms - waited))).await;
        waited += step;
    }
}

fn print_messages(v: &Value) {
    let msgs = v.get("messages").and_then(Value::as_array).cloned().unwrap_or_default();
    if msgs.is_empty() {
        println!("(no messages)");
        return;
    }
    for m in msgs {
        let pri = m.get("priority").and_then(Value::as_str).unwrap_or("normal");
        let tag = match pri {
            "urgent" => "[URGENT] ",
            "high" => "[HIGH] ",
            _ => "",
        };
        println!(
            "#{} {}{} → {}: {}",
            m.get("id").and_then(Value::as_i64).unwrap_or(0),
            tag,
            m.get("sender").and_then(Value::as_str).unwrap_or("?"),
            m.get("recipient").and_then(Value::as_str).unwrap_or("?"),
            m.get("subject").and_then(Value::as_str).unwrap_or(""),
        );
        let body = m.get("body").and_then(Value::as_str).unwrap_or("");
        if !body.is_empty() {
            println!("    {body}");
        }
    }
}

pub async fn reply(id: i64, body: String, from: Option<String>, json: bool) -> Result<()> {
    let client = ApiClient::from_env();
    let req = json!({ "id": id, "body": body, "from": from });
    let resp = client.post_json("/api/orchestration/reply", &req).await?;
    print_value(json, |_| println!("replied to #{id}"), &resp)
}

pub async fn inbox(terminal: Option<String>, limit: i64, json: bool) -> Result<()> {
    let client = ApiClient::from_env();
    let recipient = self_handle(terminal);
    let resp = client
        .get_json(&format!(
            "/api/orchestration/inbox?recipient={recipient}&limit={limit}"
        ))
        .await?;
    print_value(json, print_messages, &resp)
}

pub async fn task_create(
    spec: String,
    deps: Option<String>,
    parent: Option<i64>,
    json: bool,
) -> Result<()> {
    let client = ApiClient::from_env();
    let deps_json: Vec<i64> = match deps {
        Some(d) => serde_json::from_str(&d).unwrap_or_default(),
        None => Vec::new(),
    };
    let req = json!({ "spec": spec, "deps": deps_json, "parent": parent });
    let resp = client.post_json("/api/orchestration/tasks", &req).await?;
    print_value(json, print_task, &resp)
}

fn print_task(v: &Value) {
    let t = v.get("task").unwrap_or(v);
    println!(
        "task #{} [{}] {}",
        t.get("id").and_then(Value::as_i64).unwrap_or(0),
        t.get("status").and_then(Value::as_str).unwrap_or("?"),
        t.get("spec").and_then(Value::as_str).unwrap_or(""),
    );
}

pub async fn task_list(status: Option<String>, ready: bool, json: bool) -> Result<()> {
    let client = ApiClient::from_env();
    let mut path = String::from("/api/orchestration/tasks?");
    if let Some(s) = &status {
        path.push_str(&format!("status={s}&"));
    }
    if ready {
        path.push_str("ready=true&");
    }
    let resp = client.get_json(&path).await?;
    print_value(
        json,
        |v| {
            let tasks = v.get("tasks").and_then(Value::as_array).cloned().unwrap_or_default();
            if tasks.is_empty() {
                println!("(no tasks)");
            }
            for t in tasks {
                let deps = t
                    .get("deps")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_i64)
                            .map(|d| d.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default();
                println!(
                    "#{} [{}] {}{}",
                    t.get("id").and_then(Value::as_i64).unwrap_or(0),
                    t.get("status").and_then(Value::as_str).unwrap_or("?"),
                    t.get("spec").and_then(Value::as_str).unwrap_or(""),
                    if deps.is_empty() {
                        String::new()
                    } else {
                        format!("  (deps: {deps})")
                    },
                );
            }
        },
        &resp,
    )
}

pub async fn task_update(
    id: i64,
    status: String,
    result: Option<String>,
    json: bool,
) -> Result<()> {
    let client = ApiClient::from_env();
    let result_json: Option<Value> = result.map(|r| serde_json::from_str(&r).unwrap_or(Value::String(r)));
    let req = json!({ "status": status, "result": result_json });
    let resp = client
        .post_json(&format!("/api/orchestration/tasks/{id}/status"), &req)
        .await?;
    print_value(json, print_task, &resp)
}

pub async fn dispatch(
    task: i64,
    to: String,
    from: Option<String>,
    inject: bool,
    json: bool,
) -> Result<()> {
    let client = ApiClient::from_env();
    let req = json!({ "task": task, "to": to, "from": from });
    let resp = client.post_json("/api/orchestration/dispatch", &req).await?;

    // --inject: also push the task spec into the assignee's terminal so the
    // agent sees the work without a separate `terminal send`. Best-effort.
    if inject {
        if let Some(spec) = resp
            .get("task")
            .and_then(|t| t.get("spec"))
            .and_then(Value::as_str)
        {
            let assignee = resp
                .get("dispatch")
                .and_then(|d| d.get("assignee"))
                .and_then(Value::as_str)
                .unwrap_or(&to);
            // Reuse the terminal_control send path against the resolved handle.
            let _ = crate::commands::terminal_control::send(
                assignee.to_string(),
                vec![format!("# agentum task #{task}: {spec}")],
                false,
            )
            .await;
        }
    }
    print_value(
        json,
        |v| {
            let d = v.get("dispatch");
            println!(
                "dispatched task #{} → {}",
                task,
                d.and_then(|d| d.get("assignee")).and_then(Value::as_str).unwrap_or("?")
            );
        },
        &resp,
    )
}

pub async fn dispatch_show(task: i64, json: bool) -> Result<()> {
    let client = ApiClient::from_env();
    let resp = client
        .get_json(&format!("/api/orchestration/dispatch?task={task}"))
        .await?;
    print_value(
        json,
        |v| {
            print_task(v);
            let ds = v.get("dispatches").and_then(Value::as_array).cloned().unwrap_or_default();
            for d in ds {
                println!(
                    "  dispatch #{} → {} [{}] attempts={}",
                    d.get("id").and_then(Value::as_i64).unwrap_or(0),
                    d.get("assignee").and_then(Value::as_str).unwrap_or("?"),
                    d.get("status").and_then(Value::as_str).unwrap_or("?"),
                    d.get("attempts").and_then(Value::as_i64).unwrap_or(0),
                );
            }
        },
        &resp,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_handle_precedence() {
        // Explicit flag wins.
        assert_eq!(self_handle(Some("explicit".into())), "explicit");
        // Empty explicit is ignored; with no env set, falls back to placeholder.
        // (We don't set the env here to keep the test hermetic.)
        let v = self_handle(Some(String::new()));
        assert!(v == "unknown" || !v.is_empty());
    }
}
