//! Voice for chat: speech to text and text to speech through an
//! OpenAI-compatible speech server (Speaches: Whisper and Kokoro).
use crate::{state::App, util::*};
use axum::{body::Bytes, extract::{DefaultBodyLimit, State}, http::{header, HeaderMap}, response::IntoResponse, routing::{get, post}, Json, Router};
use serde_json::{json, Value};
use std::time::Duration;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/voice/status", get(status))
        .route("/api/voice/transcribe", post(transcribe))
        .route("/api/voice/speak", post(speak))
        .layer(DefaultBodyLimit::max(25 * 1024 * 1024))
}

fn url(app: &App) -> String { app.setting("CERQ_VOICE_URL").trim().trim_end_matches('/').to_string() }

async fn status(State(app): State<App>) -> Json<Value> {
    let u = url(&app);
    if u.is_empty() { return Json(json!({"ok": false, "error": "No speech server is set up."})); }
    let r = app.http.get(format!("{u}/models")).timeout(Duration::from_secs(4)).send().await;
    Json(match r {
        Ok(r) if r.status().is_success() => json!({"ok": true}),
        Ok(r) => json!({"ok": false, "error": format!("The speech server answered {}.", r.status())}),
        Err(_) => json!({"ok": false, "error": "The speech server didn't answer."}),
    })
}

/// The recorded audio as the request body; returns {text}.
async fn transcribe(State(app): State<App>, headers: HeaderMap, body: Bytes) -> R<Json<Value>> {
    let u = url(&app);
    if u.is_empty() { return Err(bad("No speech server is set up (Settings → Voice).")); }
    if body.len() < 100 { return Err(bad("No sound was recorded.")); }
    let ct = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("audio/webm").split(';').next().unwrap_or("audio/webm").trim().to_string();
    let ext = if ct.contains("mp4") || ct.contains("m4a") || ct.contains("aac") { "m4a" } else if ct.contains("ogg") { "ogg" } else if ct.contains("wav") { "wav" } else { "webm" };
    let part = reqwest::multipart::Part::bytes(body.to_vec()).file_name(format!("speech.{ext}")).mime_str(&ct).map_err(|e| bad(e.to_string()))?;
    let mut form = reqwest::multipart::Form::new().part("file", part).text("model", app.setting("CERQ_VOICE_STT")).text("response_format", "json");
    // Naming the language saves Whisper a detection pass (~1.3 s of 3 on Mars' CPU).
    match app.setting("CERQ_VOICE_LANG").as_str() { "Spanish" => form = form.text("language", "es"), "English" => form = form.text("language", "en"), _ => {} }
    let r = app.http.post(format!("{u}/audio/transcriptions")).multipart(form).timeout(Duration::from_secs(180)).send().await
        .map_err(|_| bad("The speech server didn't answer."))?;
    if !r.status().is_success() {
        let st = r.status();
        let msg = r.text().await.unwrap_or_default();
        tracing::warn!("transcribe: {st} {}", msg.chars().take(300).collect::<String>());
        return Err(bad(format!("The speech server couldn't transcribe this ({st}).")));
    }
    let v: Value = r.json().await.map_err(|e| bad(e.to_string()))?;
    Ok(Json(json!({"text": s(&v, "text").trim()})))
}

/// {text, hint?} → MP3 of it read aloud, in a Spanish or English voice to
/// match. A reply is voiced a few sentences at a time; `hint` (the question
/// and the reply so far) keeps every clip of it in the same voice.
async fn speak(State(app): State<App>, Json(b): Json<Value>) -> R<impl IntoResponse> {
    let u = url(&app);
    if u.is_empty() { return Err(bad("No speech server is set up (Settings → Voice).")); }
    let text: String = s(&b, "text").trim().chars().take(4000).collect();
    if text.is_empty() { return Err(bad("Nothing to read.")); }
    let voice = app.setting(if spanish(&format!("{} {text}", s(&b, "hint"))) { "CERQ_VOICE_ES" } else { "CERQ_VOICE_EN" });
    let r = app.http.post(format!("{u}/audio/speech"))
        .json(&json!({"model": app.setting("CERQ_VOICE_TTS"), "voice": voice, "input": text, "response_format": "mp3"}))
        .timeout(Duration::from_secs(180)).send().await.map_err(|_| bad("The speech server didn't answer."))?;
    if !r.status().is_success() {
        let st = r.status();
        tracing::warn!("speak: {st} {}", r.text().await.unwrap_or_default().chars().take(300).collect::<String>());
        return Err(bad(format!("The speech server couldn't read this aloud ({st}).")));
    }
    let audio = r.bytes().await.map_err(|e| bad(e.to_string()))?;
    Ok(([(header::CONTENT_TYPE, "audio/mpeg"), (header::CACHE_CONTROL, "no-store")], audio))
}

/// Rough language guess: Spanish if its common words (and accents) outweigh English ones.
fn spanish(t: &str) -> bool {
    const ES: &[&str] = &["de", "la", "que", "el", "en", "los", "las", "por", "para", "con", "una", "es", "está", "tenés", "tienes", "hoy", "mañana", "qué", "cómo", "no", "sí", "del", "se", "lo", "al", "mi", "te", "y", "un", "tu", "vos", "son", "hay", "pero", "más"];
    const EN: &[&str] = &["the", "and", "is", "you", "to", "of", "in", "it", "for", "on", "with", "your", "are", "this", "that", "have", "at", "a", "an", "be", "will", "today", "tomorrow"];
    let lower = t.to_lowercase();
    let (mut es, mut en) = (0, 0);
    for w in lower.split(|c: char| !c.is_alphabetic()).filter(|w| !w.is_empty()) {
        if ES.contains(&w) { es += 1; }
        if EN.contains(&w) { en += 1; }
    }
    es += lower.chars().filter(|c| "áéíóúñ¿¡".contains(*c)).count().min(10);
    es > en
}

#[cfg(test)]
mod tests {
    #[test]
    fn guesses_the_language() {
        assert!(super::spanish("Mañana tenés una reunión a las 10 con el equipo."));
        assert!(!super::spanish("Tomorrow you have a meeting at 10 with the team."));
        assert!(super::spanish("¿Qué hay en mi calendario?"));
    }
}
