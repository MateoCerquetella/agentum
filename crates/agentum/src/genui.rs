//! Generative UI: a small JSON component tree the server and the model write
//! and the React app draws (web/src/genui). One node: {"t": <type>, ...}.
//! The model gets GUIDE; whatever it answers goes through clean() first
//! (known types and keys only, bounded size, links http(s) or in-app "/…").
use serde_json::{json, Map, Value};

/// What the model is told it can use (chat answers, the week review).
pub const GUIDE: &str = r#"UI nodes (JSON, one object per node, "t" is the type):
{"t":"card","title":"…","children":[…]} · {"t":"row","children":[…]} (side by side) · {"t":"stack","children":[…]}
{"t":"text","text":"… **bold** …"} · {"t":"heading","text":"…"} · {"t":"callout","title":"…","text":"…","tone":"info|good|warn|bad"}
{"t":"bullets","items":[{"text":"…","items":[{"text":"…"}]}]} · {"t":"metric","label":"…","value":"…","note":"…","tone":"…"} (2-4 in a row)
{"t":"badge","text":"…","tone":"…"} · {"t":"keyvalue","pairs":[["k","v"]]} · {"t":"table","columns":["…"],"rows":[["…"]]}
{"t":"bars","labels":["Mon"],"values":[3],"unit":"h"} · {"t":"progress","value":40,"max":100,"label":"…"}
{"t":"timeline","items":[{"time":"09:30","title":"…","detail":"…"}]} · {"t":"steps","items":[{"title":"…","detail":"…","status":"done|current|blocked"}]}
{"t":"button","label":"…","action":{"go":"/journal"}} or {"ask":"a question to send"} or {"open":"https://…"}"#;

const TYPES: &[&str] = &["card", "row", "stack", "text", "heading", "callout", "bullets", "metric", "badge", "keyvalue", "table", "bars", "sparkline",
    "progress", "timeline", "steps", "people", "refs", "button", "divider", "details"];
const STR: &[&str] = &["title", "text", "label", "value", "note", "tone", "unit", "sub", "icon", "time", "detail", "status", "url", "source", "kind"];

fn url_ok(u: &str) -> bool { u.starts_with("https://") || u.starts_with("http://") || (u.starts_with('/') && !u.starts_with("//")) }
fn short(v: &Value, n: usize) -> Option<Value> {
    match v {
        Value::String(s) => Some(json!(s.chars().take(n).collect::<String>())),
        Value::Number(_) | Value::Bool(_) => Some(json!(v.to_string())),
        _ => None,
    }
}
/// An item of bullets / timeline / steps / refs: its strings, a url, refs and sub-items.
fn item(v: &Value, depth: usize, budget: &mut usize) -> Option<Value> {
    if *budget == 0 { return None; }
    *budget -= 1;
    if let Some(s) = v.as_str() { return Some(json!({"text": s.chars().take(400).collect::<String>()})); }
    let o = v.as_object()?;
    let mut m = Map::new();
    for k in STR { if let Some(x) = o.get(*k).and_then(|x| short(x, 400)) { m.insert(k.to_string(), x); } }
    if let Some(u) = m.get("url").and_then(|u| u.as_str()) { if !url_ok(u) { m.remove("url"); } }
    if let Some(r) = o.get("refs").and_then(|r| r.as_array()) {
        m.insert("refs".into(), json!(r.iter().take(6).filter_map(|x| item(x, depth + 1, budget)).collect::<Vec<_>>()));
    }
    if depth < 3 { if let Some(r) = o.get("items").and_then(|r| r.as_array()) {
        m.insert("items".into(), json!(r.iter().take(20).filter_map(|x| item(x, depth + 1, budget)).collect::<Vec<_>>()));
    } }
    if m.is_empty() { None } else { Some(Value::Object(m)) }
}

fn node(v: &Value, depth: usize, budget: &mut usize) -> Option<Value> {
    if *budget == 0 || depth > 6 { return None; }
    *budget -= 1;
    let o = v.as_object()?;
    let t = o.get("t").or(o.get("type")).and_then(|t| t.as_str())?.to_lowercase();
    if !TYPES.contains(&t.as_str()) { return None; }
    let mut m = Map::new();
    m.insert("t".into(), json!(t));
    for k in STR { if let Some(x) = o.get(*k).and_then(|x| short(x, 1200)) { m.insert(k.to_string(), x); } }
    if let Some(u) = m.get("url").and_then(|u| u.as_str()) { if !url_ok(u) { m.remove("url"); } }
    if let Some(c) = o.get("children").and_then(|c| c.as_array()) {
        m.insert("children".into(), json!(c.iter().take(30).filter_map(|x| node(x, depth + 1, budget)).collect::<Vec<_>>()));
    }
    if let Some(r) = o.get("items").and_then(|r| r.as_array()) {
        m.insert("items".into(), json!(r.iter().take(40).filter_map(|x| item(x, 0, budget)).collect::<Vec<_>>()));
    }
    let strs = |k: &str, n: usize| o.get(k).and_then(|x| x.as_array()).map(|a| a.iter().take(n).filter_map(|x| short(x, 200)).collect::<Vec<_>>());
    if let Some(a) = strs("columns", 8) { m.insert("columns".into(), json!(a)); }
    if let Some(a) = strs("labels", 40) { m.insert("labels".into(), json!(a)); }
    if let Some(a) = o.get("values").and_then(|x| x.as_array()) { m.insert("values".into(), json!(a.iter().take(40).map(|x| x.as_f64().unwrap_or(0.0)).collect::<Vec<_>>())); }
    for k in ["value", "max"] { if t == "progress" { if let Some(n) = o.get(k).and_then(|x| x.as_f64()) { m.insert(k.into(), json!(n)); } } }
    if let Some(rows) = o.get("rows").and_then(|x| x.as_array()) {
        m.insert("rows".into(), json!(rows.iter().take(40).filter_map(|r| r.as_array().map(|c| c.iter().take(8).map(|x| short(x, 300).unwrap_or(json!(""))).collect::<Vec<_>>())).collect::<Vec<_>>()));
    }
    if let Some(p) = o.get("pairs") {
        let pairs: Vec<Value> = match p {
            Value::Array(a) => a.iter().take(20).filter_map(|x| { let a = x.as_array()?; Some(json!([short(a.first()?, 120)?, short(a.get(1)?, 400)?])) }).collect(),
            Value::Object(o) => o.iter().take(20).filter_map(|(k, v)| Some(json!([k, short(v, 400)?]))).collect(),
            _ => vec![],
        };
        m.insert("pairs".into(), json!(pairs));
    }
    if let Some(a) = o.get("action").and_then(|a| a.as_object()) {
        let act = if let Some(u) = a.get("open").and_then(|x| x.as_str()).filter(|u| u.starts_with("https://") || u.starts_with("http://")) { json!({"open": u}) }
            else if let Some(u) = a.get("go").and_then(|x| x.as_str()).filter(|u| u.starts_with('/') && !u.starts_with("//")) { json!({"go": u}) }
            else if let Some(q) = a.get("ask").and_then(|x| x.as_str()) { json!({"ask": q.chars().take(300).collect::<String>()}) }
            else { Value::Null };
        if !act.is_null() { m.insert("action".into(), act); }
    }
    Some(Value::Object(m))
}

/// The model's (or anyone's) UI, safe to send to the app; None when nothing usable is left.
pub fn clean(v: &Value) -> Option<Value> {
    let mut budget = 400;
    let n = node(v, 0, &mut budget)?;
    let empty = n.as_object().is_some_and(|o| o.len() == 1);
    if empty { None } else { Some(n) }
}

// Builders the journal uses.
pub fn heading(text: &str) -> Value { json!({"t": "heading", "text": text}) }
pub fn text(text: &str) -> Value { json!({"t": "text", "text": text}) }
pub fn note(text: &str) -> Value { json!({"t": "text", "text": text, "tone": "muted"}) }
pub fn bullets(items: Vec<Value>) -> Value { json!({"t": "bullets", "items": items}) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_only_safe_ui() {
        let v = json!({"t": "card", "title": "Week", "onclick": "x", "children": [
            {"t": "script", "text": "no"},
            {"t": "bullets", "items": ["a", {"text": "b", "url": "javascript:alert(1)", "items": [{"text": "c"}]}]},
            {"t": "button", "label": "Go", "action": {"open": "javascript:x"}},
            {"t": "button", "label": "Journal", "action": {"go": "/journal"}}]});
        let c = clean(&v).unwrap();
        assert!(c.get("onclick").is_none());
        let ch = c["children"].as_array().unwrap();
        assert_eq!(ch.len(), 3);
        assert_eq!(ch[0]["items"][1]["items"][0]["text"], "c");
        assert!(ch[0]["items"][1].get("url").is_none());
        assert!(ch[1].get("action").is_none());
        assert_eq!(ch[2]["action"]["go"], "/journal");
        assert!(clean(&json!({"t": "nope"})).is_none());
    }
}
