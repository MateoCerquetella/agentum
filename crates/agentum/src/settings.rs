//! CERQ settings (the old cerq-mail Settings page): the same CERQ_* keys,
//! stored in kv "settings" as {env:{KEY: value}, ui:{...}}. Unlike the old
//! agent nothing restarts: every reader asks `app.setting(KEY)` live.
use crate::{state::App, util::*};
use axum::{extract::{Query, State}, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn fields() -> Value {
    let f = |key: &str, section: &str, label: &str, ty: &str, default: &str| json!({"key": key, "section": section, "label": label, "type": ty, "default": default});
    let sel = |key: &str, section: &str, label: &str, default: &str, opts: &[&str]| json!({"key": key, "section": section, "label": label, "type": "select", "default": default, "options": opts});
    let num = |key: &str, section: &str, label: &str, default: &str, min: f64, max: f64, unit: &str| json!({"key": key, "section": section, "label": label, "type": "number", "default": default, "min": min, "max": max, "unit": unit});
    json!([
      f("CERQ_MY_NAME", "you", "Your name", "text", ""),
      sel("CERQ_BRIEF_LANG", "you", "Language", "English", &["English", "Spanish"]),
      f("CERQ_BRIEF_AT", "today", "Morning brief at", "time", "07:00"),
      f("CERQ_WRAPUP_AT", "today", "Evening wrap-up at", "time", "21:00"),
      sel("CERQ_WEEK_DAY", "today", "Week ahead on", "Sunday", &["Monday","Tuesday","Wednesday","Thursday","Friday","Saturday","Sunday"]),
      f("CERQ_WEEK_AT", "today", "Week ahead at", "time", "19:00"),
      f("CERQ_DAY_START", "today", "Your day starts", "time", "08:00"),
      f("CERQ_DAY_END", "today", "Your day ends", "time", "22:00"),
      f("CERQ_DEFAULT_CALENDAR", "calendar", "New events go to", "calendar", ""),
      f("CERQ_WORK_CALENDARS", "calendar", "Work accounts", "accounts", ""),
      sel("CERQ_WEEK_START", "calendar", "Weeks start on", "Monday", &["Monday", "Sunday"]),
      f("CERQ_BRIEF_PLACE", "place", "Place", "place", ""),
      num("CERQ_BRIEF_LAT", "place", "Latitude", "", -90.0, 90.0, ""),
      num("CERQ_BRIEF_LON", "place", "Longitude", "", -180.0, 180.0, ""),
      f("CERQ_TZ", "place", "Time zone", "text", "UTC"),
      sel("CERQ_UNITS", "place", "Units", "Celsius", &["Celsius", "Fahrenheit"]),
      num("CERQ_HOT", "place", "Hot from", "", -50.0, 150.0, "°"),
      num("CERQ_COLD", "place", "Cold below", "", -60.0, 120.0, "°"),
      num("CERQ_HOME_LAT", "travel", "Home latitude", "", -90.0, 90.0, ""),
      num("CERQ_HOME_LON", "travel", "Home longitude", "", -180.0, 180.0, ""),
      sel("CERQ_TRAVEL_MODE", "travel", "You get around by", "Car", &["Car", "Bike", "Walking"]),
      num("CERQ_TRAVEL_BUFFER", "travel", "Extra minutes", "10", 0.0, 60.0, "min"),
      num("CERQ_MAIL_INTERVAL", "mail", "Check mail every", "60", 30.0, 3600.0, "s"),
      f("CERQ_MAIL_NOTIFY", "mail", "Notify me about", "kinds", "personal,to_reply,receipt,calendar,notification,otp"),
      sel("CERQ_DATE_ORDER", "mail", "Dates in mail", "Day first", &["Day first", "Month first"]),
      sel("CERQ_WRITE_LANG2", "writing", "Second language", "Spanish", &["Spanish","Spanish (Argentina)","Spanish (Spain)","Spanish (Mexico)","Portuguese (Brazil)","Portuguese","French","German","Italian","Dutch","English"]),
      f("CERQ_MAIL_LLM_URL", "model", "Model server", "url", "http://127.0.0.1:11434/v1"),
      f("CERQ_MAIL_LLM_MODEL", "model", "Model", "model", ""),
      sel("CERQ_VOICE_LANG", "voice", "You speak", "Auto-detect", &["Auto-detect", "Spanish", "English"]),
      f("CERQ_VOICE_URL", "voice", "Speech server", "text", "http://127.0.0.1:8800/v1"),
      f("CERQ_VOICE_STT", "voice", "Speech to text model", "text", "Systran/faster-whisper-small"),
      f("CERQ_VOICE_TTS", "voice", "Text to speech model", "text", "speaches-ai/Kokoro-82M-v1.0-ONNX"),
      f("CERQ_VOICE_EN", "voice", "English voice", "text", "af_heart"),
      f("CERQ_VOICE_ES", "voice", "Spanish voice", "text", "ef_dora"),
    ])
}

fn default_of(key: &str) -> String {
    fields().as_array().unwrap().iter().find(|f| f["key"] == key).map(|f| s(f, "default")).unwrap_or_default()
}

impl App {
    /// A setting: saved value, else the process environment, else the default.
    pub fn setting(&self, key: &str) -> String {
        if let Some(v) = self.settings.read().unwrap().get(key) { return v.clone(); }
        std::env::var(key).unwrap_or_else(|_| default_of(key))
    }
    pub fn tz(&self) -> chrono_tz::Tz { self.setting("CERQ_TZ").parse().unwrap_or(chrono_tz::UTC) }
    pub fn lang_es(&self) -> bool { self.setting("CERQ_BRIEF_LANG").to_lowercase().starts_with("span") || self.setting("CERQ_BRIEF_LANG").to_lowercase().starts_with("esp") }
}

pub async fn load(app: &App) -> anyhow::Result<()> {
    let st = app.db.kv_get("settings").await?.unwrap_or(json!({}));
    let mut m = HashMap::new();
    if let Some(env) = st.get("env").and_then(|e| e.as_object()) {
        for (k, v) in env { if let Some(v) = v.as_str() { m.insert(k.clone(), v.to_string()); } }
    }
    *app.settings.write().unwrap() = m;
    Ok(())
}

fn validate(f: &Value, v: &str) -> Result<String, String> {
    let v = v.trim();
    if v.is_empty() { return Ok(String::new()); }
    let ty = s(f, "type");
    match ty.as_str() {
        "time" => { let ok = v.len() == 5 && chrono::NaiveTime::parse_from_str(v, "%H:%M").is_ok(); if ok { Ok(v.into()) } else { Err("use HH:MM".into()) } }
        "number" => { let n: f64 = v.parse().map_err(|_| "not a number")?; let (lo, hi) = (f["min"].as_f64().unwrap_or(f64::MIN), f["max"].as_f64().unwrap_or(f64::MAX)); if n < lo || n > hi { Err(format!("between {lo} and {hi}")) } else { Ok(v.into()) } }
        "select" => if f["options"].as_array().unwrap().iter().any(|o| o == v) { Ok(v.into()) } else { Err("not an option".into()) },
        "url" => if v.starts_with("http://") || v.starts_with("https://") { Ok(v.trim_end_matches('/').into()) } else { Err("must start with http(s)://".into()) },
        _ => if s(f, "key") == "CERQ_TZ" && v.parse::<chrono_tz::Tz>().is_err() { Err("unknown time zone".into()) }
             else if v.len() > 400 || v.contains('\n') || v.contains('<') || v.contains('>') { Err("too long or has odd characters".into()) } else { Ok(v.into()) },
    }
}

pub async fn get(State(app): State<App>) -> R<Json<Value>> {
    let st = app.db.kv_get("settings").await?.unwrap_or(json!({}));
    let fs: Vec<Value> = fields().as_array().unwrap().iter().map(|f| {
        let mut f = f.clone();
        let k = s(&f, "key");
        let base = std::env::var(&k).unwrap_or_else(|_| s(&f, "default"));
        f["value"] = json!(app.setting(&k)); f["base"] = json!(base); f["changed"] = json!(st["env"].get(&k).is_some());
        f
    }).collect();
    let cals = crate::calendar::calendars(&app).await.unwrap_or_default();
    let accounts = crate::mail::accounts_brief(&app).await.unwrap_or_default();
    Ok(Json(json!({"fields": fs, "ui": st.get("ui").cloned().unwrap_or(json!({})), "calendars": cals, "accounts": accounts, "brand": app.db.kv_get("brand").await?.unwrap_or(json!("CERQ"))})))
}

pub async fn post(State(app): State<App>, Json(body): Json<Value>) -> R<Json<Value>> {
    let mut st = app.db.kv_get("settings").await?.unwrap_or(json!({"env": {}, "ui": {}}));
    if !st["env"].is_object() { st["env"] = json!({}); }
    if let Some(env) = body.get("env").and_then(|e| e.as_object()) {
        let all = fields();
        for (k, v) in env {
            let f = all.as_array().unwrap().iter().find(|f| f["key"] == *k).ok_or_else(|| bad(format!("unknown setting {k}")))?;
            let v = validate(f, v.as_str().unwrap_or("")).map_err(|e| bad(format!("{}: {e}", s(f, "label"))))?;
            let base = std::env::var(k).unwrap_or_else(|_| s(f, "default"));
            if v == base { st["env"].as_object_mut().unwrap().remove(k); } else { st["env"][k] = json!(v); }
        }
    }
    if let Some(ui) = body.get("ui") {
        if !ui.is_object() { return Err(bad("ui must be an object")); }
        st["ui"] = ui.clone();
    }
    if let Some(b) = body.get("brand").and_then(|b| b.as_str()) {
        let b: String = b.trim().chars().filter(|c| !c.is_control() && *c != '<' && *c != '>').take(24).collect();
        app.db.kv_set("brand", &json!(if b.is_empty() { "Agentum".to_string() } else { b })).await?;
    }
    app.db.kv_set("settings", &st).await?;
    load(&app).await?;
    app.emit(json!({"kind": "settings"}));
    get(State(app)).await
}

/// The sidebar's order (page ids): a layout choice, so no Touch ID unlock like the rest of Settings.
pub async fn sidebar(State(app): State<App>, Json(body): Json<Value>) -> R<Json<Value>> {
    let order: Vec<String> = body.get("order").and_then(|o| o.as_array()).ok_or_else(|| bad("order must be a list"))?
        .iter().filter_map(|p| p.as_str()).filter(|p| p.len() <= 20 && p.chars().all(|c| c.is_ascii_lowercase())).take(20).map(String::from).collect();
    let mut st = app.db.kv_get("settings").await?.unwrap_or(json!({"env": {}, "ui": {}}));
    if !st["ui"].is_object() { st["ui"] = json!({}); }
    st["ui"]["sidebar"] = json!(order);
    app.db.kv_set("settings", &st).await?;
    app.emit(json!({"kind": "settings"}));
    Ok(Json(json!({"ok": true, "order": order})))
}

/// The look (Glance or Studio) and System / Light / Dark, for every device: a new browser starts
/// in it. A device that picks again changes it everywhere.
pub async fn look(State(app): State<App>, Json(body): Json<Value>) -> R<Json<Value>> {
    let theme = body["theme"].as_str().filter(|t| ["glance", "studio"].contains(t)).ok_or_else(|| bad("theme is glance or studio"))?;
    let mode = body["mode"].as_str().filter(|m| ["system", "light", "dark"].contains(m)).ok_or_else(|| bad("mode is system, light or dark"))?;
    let mut st = app.db.kv_get("settings").await?.unwrap_or(json!({"env": {}, "ui": {}}));
    if !st["ui"].is_object() { st["ui"] = json!({}); }
    st["ui"]["look"] = json!({"theme": theme, "mode": mode});
    app.db.kv_set("settings", &st).await?;
    app.emit(json!({"kind": "settings"}));
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)] pub struct Q { q: String }
pub async fn geocode(State(app): State<App>, Query(q): Query<Q>) -> R<Json<Value>> {
    if q.q.len() < 2 || q.q.len() > 80 { return Err(bad("2–80 characters")); }
    let r: Value = app.http.get("https://geocoding-api.open-meteo.com/v1/search").query(&[("name", q.q.as_str()), ("count", "6"), ("format", "json")])
        .header("User-Agent", "cerq/1.0").send().await?.json().await?;
    let places: Vec<Value> = r["results"].as_array().cloned().unwrap_or_default().iter().map(|p| json!({
        "name": p["name"], "region": p["admin1"], "country": p["country"], "lat": p["latitude"], "lon": p["longitude"], "tz": p["timezone"]})).collect();
    Ok(Json(json!({"places": places})))
}

pub async fn test_model(State(app): State<App>, Json(b): Json<Value>) -> R<Json<Value>> {
    let url = s(&b, "url");
    let url = if url.is_empty() { app.setting("CERQ_MAIL_LLM_URL") } else { url };
    Ok(Json(match crate::llm::list_models(&app, &url).await {
        Ok(m) => json!({"ok": true, "models": m}),
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    }))
}
