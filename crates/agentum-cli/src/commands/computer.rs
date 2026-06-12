//! `agentum computer …` — macOS computer-use over `/api/computer/*`. Only works
//! against a running desktop on macOS (the engine runs in the .app that holds
//! the Accessibility grant); reaches it via `$AGENTUM_API_URL`.

use anyhow::Result;
use serde_json::{json, Value};

use crate::http::ApiClient;

async fn call(op: &str, body: Value) -> Result<Value> {
    ApiClient::from_env()
        .post_json(&format!("/api/computer/{op}"), &body)
        .await
}

fn show(v: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

pub async fn capabilities() -> Result<()> {
    show(&call("capabilities", json!({})).await?)
}

pub async fn permissions() -> Result<()> {
    show(&call("permissions", json!({})).await?)
}

pub async fn list_apps(json_out: bool) -> Result<()> {
    let resp = call("list-apps", json!({})).await?;
    if json_out {
        return show(&resp);
    }
    let apps = resp.get("apps").and_then(Value::as_array).cloned().unwrap_or_default();
    for a in apps {
        println!(
            "{:>7}  {}",
            a.get("pid").and_then(Value::as_i64).unwrap_or(0),
            a.get("name").and_then(Value::as_str).unwrap_or(""),
        );
    }
    Ok(())
}

pub async fn get_app_state(app: String, json_out: bool) -> Result<()> {
    let resp = call("get-app-state", json!({ "app": app })).await?;
    if json_out {
        return show(&resp);
    }
    let els = resp.get("elements").and_then(Value::as_array).cloned().unwrap_or_default();
    println!(
        "{} elements in {}",
        resp.get("count").and_then(Value::as_i64).unwrap_or(0),
        resp.get("app").and_then(Value::as_str).unwrap_or(""),
    );
    for e in els {
        let title = e.get("title").and_then(Value::as_str).unwrap_or("");
        let value = e.get("value").and_then(Value::as_str).unwrap_or("");
        println!(
            "  [{}] {} {}{}",
            e.get("index").and_then(Value::as_i64).unwrap_or(0),
            e.get("role").and_then(Value::as_str).unwrap_or(""),
            title,
            if value.is_empty() { String::new() } else { format!("= {value}") },
        );
    }
    Ok(())
}

pub async fn click(app: String, element_index: usize) -> Result<()> {
    show(&call("click", json!({ "app": app, "element-index": element_index })).await?)
}

pub async fn set_value(app: String, element_index: usize, value: String) -> Result<()> {
    show(&call(
        "set-value",
        json!({ "app": app, "element-index": element_index, "value": value }),
    )
    .await?)
}

pub async fn type_text(app: String, text: String) -> Result<()> {
    show(&call("type-text", json!({ "app": app, "text": text })).await?)
}

pub async fn press_key(app: String, key: String) -> Result<()> {
    show(&call("press-key", json!({ "app": app, "key": key })).await?)
}
