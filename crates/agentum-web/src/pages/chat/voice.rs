//! Voice for the Secretary: record from the mic and have the hub's speech server write it out
//! (`/api/voice/transcribe`), and read replies aloud (`/api/voice/speak`), a few sentences at a
//! time so the first words start while the rest is still being written. One `<audio>` is reused
//! so Safari, once it has played in a tap, lets later replies play on their own.
use agentum_ui::prelude::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

const SILENT: &str = "data:audio/wav;base64,UklGRiQAAABXQVZFZm10IBAAAAABAAEAQB8AAIA+AAACABAAZGF0YQAAAAA=";

fn js_msg(e: &JsValue) -> String {
    js_sys::Reflect::get(e, &"message".into()).ok().and_then(|m| m.as_string()).unwrap_or_else(|| format!("{e:?}"))
}
fn js_name(e: &JsValue) -> String {
    js_sys::Reflect::get(e, &"name".into()).ok().and_then(|m| m.as_string()).unwrap_or_default()
}

pub async fn ready() -> bool {
    api_get::<Value>("/voice/status").await.is_ok_and(|v| v["ok"] == true)
}

/// A recording in progress.
pub struct Recording {
    pub stream: web_sys::MediaStream,
    rec: web_sys::MediaRecorder,
    chunks: Rc<RefCell<Vec<web_sys::Blob>>>,
    mime: String,
}

fn end_tracks(s: &web_sys::MediaStream) {
    for t in s.get_tracks().iter() {
        if let Ok(t) = t.dyn_into::<web_sys::MediaStreamTrack>() {
            t.stop();
        }
    }
}

pub async fn record() -> Result<Recording, String> {
    let w = web_sys::window().ok_or("no window")?;
    let devices = w.navigator().media_devices().map_err(|_| "This browser can’t record here (it needs HTTPS).".to_string())?;
    let c = web_sys::MediaStreamConstraints::new();
    let audio = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&audio, &"echoCancellation".into(), &true.into());
    let _ = js_sys::Reflect::set(&audio, &"noiseSuppression".into(), &true.into());
    c.set_audio(&audio);
    let p = devices.get_user_media_with_constraints(&c).map_err(|e| js_msg(&e))?;
    let stream: web_sys::MediaStream = match JsFuture::from(p).await {
        Ok(s) => s.unchecked_into(),
        Err(e) if js_name(&e) == "NotAllowedError" => return Err("Allow the microphone for Agentum to talk to it.".into()),
        Err(e) => return Err(js_msg(&e)),
    };
    let mime = ["audio/webm;codecs=opus", "audio/mp4", "audio/webm", "audio/ogg"].into_iter().find(|t| web_sys::MediaRecorder::is_type_supported(t)).unwrap_or("").to_string();
    let rec = if mime.is_empty() {
        web_sys::MediaRecorder::new_with_media_stream(&stream)
    } else {
        let o = web_sys::MediaRecorderOptions::new();
        o.set_mime_type(&mime);
        web_sys::MediaRecorder::new_with_media_stream_and_media_recorder_options(&stream, &o)
    }
    .map_err(|e| {
        end_tracks(&stream);
        js_msg(&e)
    })?;
    let chunks = Rc::new(RefCell::new(vec![]));
    let c2 = chunks.clone();
    let on_data = Closure::<dyn FnMut(web_sys::BlobEvent)>::new(move |e: web_sys::BlobEvent| {
        if let Some(b) = e.data().filter(|b| b.size() > 0.0) {
            c2.borrow_mut().push(b);
        }
    });
    rec.set_ondataavailable(Some(on_data.as_ref().unchecked_ref()));
    on_data.forget();
    rec.start_with_time_slice(250).map_err(|e| js_msg(&e))?;
    Ok(Recording { stream, rec, chunks, mime })
}

impl Recording {
    /// Stop and hand over what was said.
    pub async fn stop(self) -> web_sys::Blob {
        let rec = self.rec.clone();
        let done = js_sys::Promise::new(&mut |resolve, _| {
            rec.set_onstop(Some(&resolve));
        });
        let _ = self.rec.stop();
        let _ = JsFuture::from(done).await;
        end_tracks(&self.stream);
        let parts = js_sys::Array::new();
        for b in self.chunks.borrow().iter() {
            parts.push(b);
        }
        let bag = web_sys::BlobPropertyBag::new();
        let ty = Some(self.rec.mime_type()).filter(|m| !m.is_empty()).unwrap_or_else(|| if self.mime.is_empty() { "audio/webm".into() } else { self.mime.clone() });
        bag.set_type(&ty);
        web_sys::Blob::new_with_blob_sequence_and_options(&parts, &bag).unwrap_or_else(|_| web_sys::Blob::new().unwrap())
    }
    pub fn cancel(&self) {
        self.rec.set_onstop(None);
        if self.rec.state() != web_sys::RecordingState::Inactive {
            let _ = self.rec.stop();
        }
        end_tracks(&self.stream);
    }
}

async fn fetch(path: &str, body: &JsValue, content_type: &str) -> Result<web_sys::Response, String> {
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_body(body);
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    let h = web_sys::Headers::new().map_err(|e| js_msg(&e))?;
    let _ = h.set("Content-Type", content_type);
    init.set_headers(&h);
    let w = web_sys::window().ok_or("no window")?;
    let r: web_sys::Response = JsFuture::from(w.fetch_with_str_and_init(&format!("/api{path}"), &init)).await.map_err(|e| js_msg(&e))?.unchecked_into();
    if r.ok() {
        return Ok(r);
    }
    let text = match r.text() {
        Ok(p) => JsFuture::from(p).await.ok().and_then(|t| t.as_string()).unwrap_or_default(),
        Err(_) => String::new(),
    };
    Err(serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or_else(|| r.status_text()))
}

/// The speech server's words for a recording.
pub async fn transcribe(audio: web_sys::Blob) -> Result<String, String> {
    let ty = Some(audio.type_()).filter(|t| !t.is_empty()).unwrap_or_else(|| "audio/webm".into());
    let r = fetch("/voice/transcribe", &audio, &ty).await?;
    let text = JsFuture::from(r.text().map_err(|e| js_msg(&e))?).await.map_err(|e| js_msg(&e))?.as_string().unwrap_or_default();
    Ok(serde_json::from_str::<Value>(&text).ok().and_then(|v| v["text"].as_str().map(str::to_string)).unwrap_or_default())
}

// ── reading aloud ──

enum Clip {
    Pending,
    Ready(Option<String>),
}

#[derive(Default)]
struct Speech {
    player: Option<web_sys::HtmlAudioElement>,
    generation: u32,
    queue: VecDeque<Rc<RefCell<Clip>>>,
    /// A streaming reply may still add clips.
    open: bool,
}

thread_local! {
    static SPEECH: RefCell<Speech> = RefCell::new(Speech::default());
    static PLAYING: RwSignal<Option<String>> = agentum_ui::global(None);
}

/// The message being read aloud (its key), live.
pub fn playing() -> RwSignal<Option<String>> {
    PLAYING.with(|p| *p)
}

fn player() -> Option<web_sys::HtmlAudioElement> {
    SPEECH.with(|s| {
        let mut s = s.borrow_mut();
        if s.player.is_none() {
            s.player = web_sys::HtmlAudioElement::new().ok();
        }
        s.player.clone()
    })
}
fn generation() -> u32 {
    SPEECH.with(|s| s.borrow().generation)
}

/// Call inside a tap so a reply can be played later without one (Safari).
pub fn unlock() {
    if playing().get_untracked().is_some() {
        return;
    }
    if let Some(a) = player() {
        a.set_src(SILENT);
        if let Ok(p) = a.play() {
            spawn_local(async move {
                let _ = JsFuture::from(p).await;
            });
        }
    }
}

pub fn stop() {
    let a = SPEECH.with(|s| {
        let mut s = s.borrow_mut();
        s.generation += 1;
        s.queue.clear();
        s.open = false;
        s.player.clone()
    });
    if let Some(a) = a {
        let _ = a.pause();
    }
    if playing().get_untracked().is_some() {
        playing().set(None);
    }
}

/// What a reply sounds like read aloud: no code, UI blocks, links or Markdown marks.
pub fn speakable(md: &str) -> String {
    let mut out = String::new();
    let mut fence = false;
    for line in md.lines() {
        let t = line.trim_start();
        if t.starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence || t.starts_with("> cerq:") || t.starts_with("> agentum:") {
            continue;
        }
        let mut l = t;
        loop {
            let before = l;
            l = l.trim_start_matches('#').trim_start_matches('>');
            if let Some(r) = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")).or_else(|| l.strip_prefix("+ ")) {
                l = r;
            }
            let digits = l.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits > 0 && l[digits..].starts_with(". ") {
                l = &l[digits + 2..];
            }
            l = l.trim_start();
            if l == before {
                break;
            }
        }
        out.push_str(&plain_inline(l));
        out.push('\n');
    }
    out.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim().to_string()
}

fn plain_inline(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        // ![alt](url) → nothing; [text](url) → text; [12] → nothing.
        if c == '!' && rest[1..].starts_with('[') {
            if let Some(end) = rest.find("](").and_then(|i| rest[i..].find(')').map(|j| i + j)) {
                rest = &rest[end + 1..];
                continue;
            }
        }
        if c == '[' {
            if let Some(close) = rest.find(']') {
                let inner = &rest[1..close];
                if rest[close + 1..].starts_with('(') {
                    if let Some(end) = rest[close..].find(')') {
                        out.push_str(inner);
                        rest = &rest[close + end + 1..];
                        continue;
                    }
                } else if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit()) {
                    rest = &rest[close + 1..];
                    continue;
                }
            }
        }
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            rest = &rest[end..];
            continue;
        }
        if rest.starts_with("**") || rest.starts_with("__") || rest.starts_with("~~") {
            rest = &rest[2..];
            continue;
        }
        match c {
            '`' | '*' => {}
            '|' => out.push(' '),
            c => out.push(c),
        }
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Whole sentences at the start of `text` (an ending needs a space after it, so "10.30" stays
/// whole), and how many bytes they use.
pub fn sentences(text: &str, fin: bool) -> (Vec<String>, usize) {
    let cs: Vec<(usize, char)> = text.char_indices().collect();
    let at = |j: usize| if j < cs.len() { cs[j].0 } else { text.len() };
    let (mut done, mut used, mut i) = (vec![], 0, 0);
    while i < cs.len() {
        let c = cs[i].1;
        let mut end = None;
        if ".!?…".contains(c) {
            let mut j = i;
            while j < cs.len() && ".!?…".contains(cs[j].1) {
                j += 1;
            }
            while j < cs.len() && "\"'”’)]".contains(cs[j].1) {
                j += 1;
            }
            let k = j;
            while j < cs.len() && cs[j].1.is_whitespace() {
                j += 1;
            }
            if j > k {
                end = Some(j);
            } else {
                i = j;
                continue;
            }
        } else if c == '\n' {
            let mut j = i;
            while j < cs.len() && cs[j].1 == '\n' {
                j += 1;
            }
            end = Some(j);
        }
        match end {
            Some(j) => {
                let s = text[used..at(j)].trim();
                if !s.is_empty() {
                    done.push(s.to_string());
                }
                used = at(j);
                i = j;
            }
            None => i += 1,
        }
    }
    if fin && !text[used..].trim().is_empty() {
        done.push(text[used..].trim().to_string());
        used = text.len();
    }
    (done, used)
}

/// Group sentences into clips: the first alone (to start fast), then ~200 characters each.
fn clips(done: Vec<String>, first: bool, fin: bool) -> (Vec<String>, Vec<String>) {
    let (mut out, mut buf): (Vec<String>, Vec<String>) = (vec![], vec![]);
    for s in done {
        buf.push(s);
        if (first && out.is_empty()) || buf.join(" ").len() >= 200 {
            out.push(buf.join(" "));
            buf.clear();
        }
    }
    if fin && !buf.is_empty() {
        out.push(buf.join(" "));
        buf.clear();
    }
    (out, buf)
}

/// Voice one clip in the background; it plays in its turn.
fn clip(text: String, hint: &str) {
    let slot = Rc::new(RefCell::new(Clip::Pending));
    SPEECH.with(|s| s.borrow_mut().queue.push_back(slot.clone()));
    let g = generation();
    let hint: String = { let cs: Vec<char> = hint.chars().collect(); cs[cs.len().saturating_sub(2000)..].iter().collect() };
    spawn_local(async move {
        let body = JsValue::from_str(&json!({"text": text, "hint": hint}).to_string());
        let res = async {
            let r = fetch("/voice/speak", &body, "application/json").await?;
            let blob: web_sys::Blob = JsFuture::from(r.blob().map_err(|e| js_msg(&e))?).await.map_err(|e| js_msg(&e))?.unchecked_into();
            web_sys::Url::create_object_url_with_blob(&blob).map_err(|e| js_msg(&e))
        }
        .await;
        let url = match res {
            Ok(u) if g == generation() => Some(u),
            Ok(u) => {
                let _ = web_sys::Url::revoke_object_url(&u);
                None
            }
            Err(e) => {
                if g == generation() {
                    toast_bad(e);
                }
                None
            }
        };
        *slot.borrow_mut() = Clip::Ready(url);
    });
}

async fn play_loop(g: u32) {
    let Some(a) = player() else { return };
    loop {
        if g != generation() {
            return;
        }
        let next = SPEECH.with(|s| s.borrow_mut().queue.pop_front());
        let Some(slot) = next else {
            if !SPEECH.with(|s| s.borrow().open) {
                break;
            }
            gloo_timers::future::TimeoutFuture::new(60).await;
            continue;
        };
        let url = loop {
            if g != generation() {
                return;
            }
            if let Clip::Ready(u) = &*slot.borrow() {
                break u.clone();
            }
            gloo_timers::future::TimeoutFuture::new(40).await;
        };
        let Some(url) = url else { continue };
        let a2 = a.clone();
        let u2 = url.clone();
        let ended = js_sys::Promise::new(&mut |resolve, _| {
            a2.set_onended(Some(&resolve));
            a2.set_onerror(Some(&resolve));
            a2.set_onpause(Some(&resolve));
            a2.set_src(&u2);
            match a2.play() {
                Ok(p) => {
                    let r = resolve.clone();
                    spawn_local(async move {
                        if JsFuture::from(p).await.is_err() {
                            let _ = r.call0(&JsValue::NULL);
                        }
                    });
                }
                Err(_) => {
                    let _ = resolve.call0(&JsValue::NULL);
                }
            }
        });
        let _ = JsFuture::from(ended).await;
        a.set_onended(None);
        a.set_onerror(None);
        a.set_onpause(None);
        let _ = web_sys::Url::revoke_object_url(&url);
    }
    if g == generation() {
        playing().set(None);
    }
}

fn begin(key: &str, streaming: bool) -> u32 {
    stop();
    let g = SPEECH.with(|s| {
        let mut s = s.borrow_mut();
        s.open = streaming;
        s.generation
    });
    playing().set(Some(key.to_string()));
    spawn_local(play_loop(g));
    g
}

/// Read a finished reply aloud; the same key again stops it.
pub fn speak(key: &str, md: &str) {
    if playing().get_untracked().as_deref() == Some(key) {
        stop();
        return;
    }
    let text: String = speakable(md).chars().take(8000).collect();
    if text.is_empty() {
        return;
    }
    begin(key, false);
    for c in clips(sentences(&text, true).0, true, true).0 {
        clip(c, md);
    }
}

/// Read a reply aloud while it streams in: `push` each delta, `end` when it's done.
pub struct Streamer {
    g: u32,
    raw: String,
    used: usize,
    pending: Vec<String>,
    first: bool,
    question: String,
}

pub fn speak_stream(key: &str, question: &str) -> Streamer {
    Streamer { g: begin(key, true), raw: String::new(), used: 0, pending: vec![], first: true, question: question.to_string() }
}

impl Streamer {
    fn flush(&mut self, fin: bool) {
        if self.g != generation() {
            return;
        }
        // Speak only what comes before an unfinished ``` block.
        let ready = if self.raw.matches("```").count() % 2 == 1 { &self.raw[..self.raw.rfind("```").unwrap_or(0)] } else { &self.raw[..] };
        let plain = speakable(ready);
        let Some(rest) = plain.get(self.used..) else { return };
        let (done, used) = sentences(rest, fin);
        self.used += used;
        let mut all = std::mem::take(&mut self.pending);
        all.extend(done);
        let (out, keep) = clips(all, self.first, fin);
        self.pending = keep;
        let hint = format!("{}\n{}", self.question, self.raw);
        for c in out {
            clip(c, &hint);
            self.first = false;
        }
        if fin {
            SPEECH.with(|s| s.borrow_mut().open = false);
        }
    }
    pub fn push(&mut self, delta: &str) {
        self.raw.push_str(delta);
        self.flush(false);
    }
    pub fn end(&mut self) {
        self.flush(true);
    }
}

// ── while you talk ──

/// Bars for the last few seconds of loudness, newest on the right.
#[component]
pub fn Waveform(stream: web_sys::MediaStream) -> impl IntoView {
    let canvas = NodeRef::<leptos::html::Canvas>::new();
    let started = std::cell::Cell::new(false);
    Effect::new(move |_| {
        let Some(c) = canvas.get() else { return };
        if started.replace(true) {
            return;
        }
        let c: web_sys::HtmlCanvasElement = c;
        let Ok(ctx) = web_sys::AudioContext::new() else { return };
        let _ = ctx.resume();
        let Ok(source) = ctx.create_media_stream_source(&stream) else { return };
        let Ok(analyser) = ctx.create_analyser() else { return };
        analyser.set_fft_size(1024);
        let _ = source.connect_with_audio_node(&analyser);
        let color = web_sys::window().and_then(|w| w.get_computed_style(&c).ok().flatten()).and_then(|s| s.get_property_value("color").ok()).unwrap_or_else(|| "#888".into());
        let levels = Rc::new(RefCell::new(VecDeque::<f64>::new()));
        let last = Rc::new(std::cell::Cell::new(0.0));
        let frame: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> = Rc::new(RefCell::new(None));
        let f2 = frame.clone();
        let mut samples = vec![0f32; 1024];
        *frame.borrow_mut() = Some(Closure::new(move |t: f64| {
            // Gone from the page: let go of the mic's audio.
            if !c.is_connected() {
                let _ = source.disconnect();
                let _ = ctx.close();
                f2.borrow_mut().take();
                return;
            }
            let dpr = web_sys::window().map(|w| w.device_pixel_ratio()).unwrap_or(1.0);
            let (w, h) = (c.client_width() as f64, c.client_height() as f64);
            let (pw, ph) = ((w * dpr).round() as u32, (h * dpr).round() as u32);
            if c.width() != pw || c.height() != ph {
                c.set_width(pw);
                c.set_height(ph);
            }
            let (step, bar) = (5.0, 3.0);
            let slots = ((w / step).floor() as usize).max(1);
            if t - last.get() > 70.0 {
                last.set(t);
                analyser.get_float_time_domain_data(&mut samples);
                let rms = (samples.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / samples.len() as f64).sqrt();
                let mut l = levels.borrow_mut();
                l.push_back((rms * 7.0 - 0.03).clamp(0.0, 1.0));
                while l.len() > slots {
                    l.pop_front();
                }
            }
            if let Some(g) = c.get_context("2d").ok().flatten().and_then(|g| g.dyn_into::<web_sys::CanvasRenderingContext2d>().ok()) {
                let _ = g.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
                g.clear_rect(0.0, 0.0, w, h);
                g.set_fill_style_str(&color);
                let l = levels.borrow();
                for i in 0..slots {
                    let level = (l.len() + i).checked_sub(slots).and_then(|j| l.get(j)).copied();
                    let bh = level.map(|v| (v * h * 0.9).max(2.0)).unwrap_or(2.0);
                    g.set_global_alpha(if level.is_some() { 1.0 } else { 0.35 });
                    g.fill_rect(i as f64 * step, (h - bh) / 2.0, bar, bh);
                }
            }
            if let (Some(w), Some(f)) = (web_sys::window(), f2.borrow().as_ref()) {
                let _ = w.request_animation_frame(f.as_ref().unchecked_ref());
            }
        }));
        if let (Some(w), Some(f)) = (web_sys::window(), frame.borrow().as_ref()) {
            let _ = w.request_animation_frame(f.as_ref().unchecked_ref());
        }
    });
    view! { <canvas node_ref=canvas class="chat-wave" aria-hidden="true"/> }
}

/// m:ss since it started.
#[component]
pub fn Elapsed() -> impl IntoView {
    let start = now_ms();
    let now = RwSignal::new(start);
    // Dropped (stopped) with the view.
    let _tick = StoredValue::new_local(gloo_timers::callback::Interval::new(500, move || now.set(now_ms())));
    view! { <span class="chat-elapsed small subdue">{move || { let s = (now.get() - start) / 1000; format!("{}:{:02}", s / 60, s % 60) }}</span> }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sentences_and_speech() {
        assert_eq!(sentences("Hi there. It is 10.30 now! And", false), (vec!["Hi there.".to_string(), "It is 10.30 now!".to_string()], 27));
        assert_eq!(sentences("one\ntwo", true).0, vec!["one", "two"]);
        assert_eq!(speakable("## Title\n- **bold** [link](http://x) see https://y.z [1]\n```ui\n{}\n```\nend"), "Title\nbold link see\nend");
    }
}
