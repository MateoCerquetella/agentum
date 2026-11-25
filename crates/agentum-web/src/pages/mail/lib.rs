//! Mail: shared types, vocabularies and pure helpers (no views).
use agentum_ui::prelude::*;
use regex_lite::Regex;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;
use wasm_bindgen::JsValue;

fn s(v: &Value, k: &str) -> String {
    match &v[k] {
        Value::String(x) => x.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}
fn b(v: &Value, k: &str) -> bool {
    v[k].as_bool().unwrap_or(false)
}

/// ViewMsg from /api/mail/* (one email in a list).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Msg {
    pub account: String,
    pub account_name: String,
    pub folder: String,
    pub uid: u32,
    pub msgid: String,
    pub from_name: String,
    pub from_addr: String,
    pub subject: String,
    pub ts: i64,
    pub unread: bool,
    pub starred: bool,
    pub category: String,
    pub to: String,
    pub thrid: Option<String>,
    pub extra: Option<Extra>,
    /// Awaiting reply: days since you wrote.
    pub days: Option<i64>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extra {
    pub snippet: String,
    pub code: String,
    pub amount: String,
}
impl Msg {
    pub fn of(v: &Value) -> Msg {
        let to = match &v["to"] {
            Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "),
            _ => s(v, "to"),
        };
        let extra = v["extra"].as_object().map(|_| Extra { snippet: s(&v["extra"], "snippet"), code: s(&v["extra"], "code"), amount: s(&v["extra"], "amount") });
        Msg {
            account: s(v, "account"),
            account_name: s(v, "account_name"),
            folder: s(v, "folder"),
            uid: v["uid"].as_u64().or_else(|| v["uid"].as_str().and_then(|x| x.parse().ok())).unwrap_or(0) as u32,
            msgid: s(v, "msgid"),
            from_name: s(v, "from_name"),
            from_addr: s(v, "from_addr"),
            subject: s(v, "subject"),
            ts: v["ts"].as_i64().unwrap_or(0),
            unread: b(v, "unread"),
            starred: b(v, "starred"),
            category: s(v, "category"),
            to,
            thrid: v["thrid"].as_str().filter(|x| !x.is_empty()).map(str::to_string),
            extra,
            days: v["days"].as_i64(),
        }
    }
    pub fn list(v: &Value) -> Vec<Msg> {
        v.as_array().map(|a| a.iter().map(Msg::of).collect()).unwrap_or_default()
    }
    pub fn item(&self) -> Item {
        Item { account: self.account.clone(), folder: self.folder.clone(), uid: self.uid }
    }
    pub fn key(&self) -> String {
        key_of(&self.account, &self.folder, self.uid)
    }
    pub fn who(&self) -> &str {
        if self.from_name.is_empty() { &self.from_addr } else { &self.from_name }
    }
}

/// A row: the newest message of a conversation plus all of its messages.
#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    pub m: Msg,
    pub key: String,
    pub all: Vec<Msg>,
}
impl Thread {
    pub fn item(&self) -> Item {
        self.m.item()
    }
    /// For keyed lists: changes whenever what the row shows changes.
    pub fn rev(&self) -> String {
        format!("{}|{}|{}|{}|{}", self.key, self.m.unread, self.m.starred, self.all.len(), self.m.from_name)
    }
}
pub fn single(m: &Msg) -> Thread {
    Thread { key: m.key(), m: m.clone(), all: vec![m.clone()] }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub account: String,
    pub folder: String,
    pub uid: u32,
}
impl Item {
    pub fn key(&self) -> String {
        key_of(&self.account, &self.folder, self.uid)
    }
    pub fn json(&self) -> Value {
        json!({"account": self.account, "folder": self.folder, "uid": self.uid})
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub from: String,
    pub default: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub unread: BTreeMap<String, u64>,
    pub total: u64,
}
impl Counts {
    pub fn of(v: &Value) -> Counts {
        Counts {
            unread: v["unread"].as_object().map(|o| o.iter().map(|(k, n)| (k.clone(), n.as_u64().unwrap_or(0))).collect()).unwrap_or_default(),
            total: v["total"].as_u64().unwrap_or(0),
        }
    }
    pub fn of_account(&self, a: &str) -> u64 {
        if a.is_empty() { self.total } else { self.unread.get(a).copied().unwrap_or(0) }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Att {
    pub index: u32,
    pub filename: String,
    pub size: u64,
    pub content_type: String,
    pub is_inline: bool,
    pub embedded: bool,
}
/// GET /api/mail/read
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Full {
    pub account: String,
    pub folder: String,
    pub uid: u32,
    pub subject: String,
    pub from_name: String,
    pub from_address: String,
    pub date: String,
    pub to: String,
    pub cc: String,
    pub body: String,
    pub body_html: String,
    pub message_id: String,
    pub references: String,
    pub attachments: Vec<Att>,
}
impl Full {
    pub fn of(v: &Value, it: &Item) -> Full {
        Full {
            account: it.account.clone(),
            folder: it.folder.clone(),
            uid: it.uid,
            subject: s(v, "subject"),
            from_name: s(v, "from_name"),
            from_address: s(v, "from_address"),
            date: s(v, "date"),
            to: s(v, "to"),
            cc: s(v, "cc"),
            body: s(v, "body"),
            body_html: s(v, "body_html"),
            message_id: s(v, "message_id"),
            references: s(v, "references"),
            attachments: v["attachments"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|x| Att { index: x["index"].as_u64().unwrap_or(0) as u32, filename: s(x, "filename"), size: x["size"].as_u64().unwrap_or(0), content_type: s(x, "content_type"), is_inline: b(x, "is_inline"), embedded: b(x, "embedded") })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
    pub fn att_url(&self, i: u32) -> String {
        format!("/api/mail/attachment?account={}&folder={}&uid={}&index={i}", enc(&self.account), enc(&self.folder), self.uid)
    }
}

pub const CAT: [(&str, &str); 8] = [
    ("personal", "Personal"),
    ("newsletter", "Newsletter"),
    ("marketing", "Marketing"),
    ("receipt", "Receipt"),
    ("notification", "Notification"),
    ("calendar", "Calendar"),
    ("otp", "Codes"),
    ("cold_email", "Cold email"),
];
pub fn cat(k: &str) -> String {
    CAT.iter().find(|(x, _)| *x == k).map(|(_, l)| l.to_string()).unwrap_or_else(|| k.to_string())
}
pub fn action_label(a: &str) -> &str {
    match a {
        "archive" => "Archive",
        "label" => "Label",
        "move" => "Move to folder",
        "mark_read" => "Mark read",
        "star" => "Star",
        "spam" => "Spam",
        "delete" => "Delete",
        "mark_unread" => "Mark unread",
        "unstar" => "Unstar",
        x => x,
    }
}
pub const RULE_ACTIONS: [&str; 7] = ["archive", "label", "move", "mark_read", "star", "spam", "delete"];
/// Actions that take a message out of the list.
pub fn gone(a: &str) -> bool {
    matches!(a, "archive" | "spam" | "delete" | "move")
}
pub const FOLDERS: [(&str, &str); 4] = [("inbox", "Inbox"), ("junk", "Junk / Spam"), ("archive", "Archive"), ("sent", "Sent")];
/// Labels on inbox rows: [text, css suffix].
pub fn label(c: &str) -> Option<(&'static str, &'static str)> {
    Some(match c {
        "receipt" => ("Finance", "fin"),
        "otp" => ("Security", "sec"),
        "notification" => ("Notification", "note"),
        "calendar" => ("Calendar", "cal"),
        "newsletter" => ("Newsletter", "news"),
        "marketing" => ("Promotion", "news"),
        "cold_email" => ("Cold email", "cold"),
        _ => return None,
    })
}

pub fn key_of(account: &str, folder: &str, uid: u32) -> String {
    format!("{account}|{folder}|{uid}")
}
pub fn kb(n: u64) -> String {
    if n > 1_048_576 { format!("{:.1} MB", n as f64 / 1_048_576.0) } else { format!("{} KB", ((n as f64 / 1024.0).round() as u64).max(1)) }
}
pub fn enc(x: &str) -> String {
    js_sys::encode_uri_component(x).into()
}

fn utf16_hash(s: &str, m: u32) -> u32 {
    let mut h: u32 = 0;
    for c in s.encode_utf16() {
        h = (h * 31 + c as u32) % m;
    }
    h
}
/// One colour per account, the same everywhere; hues fixed by account name.
pub fn color_of_name(name: &str) -> String {
    let n = name.to_lowercase();
    let h = match n.as_str() {
        "gmail" => 4,
        "icloud" => 205,
        "outlook" => 270,
        _ => utf16_hash(&n, 360),
    };
    format!("hsl({h} 70% 72%)")
}
pub fn avatar_hue(s: &str) -> u32 {
    let mut h: u32 = 0;
    for c in s.to_lowercase().chars() {
        let mut b = [0u16; 2];
        h = (h * 31 + c.encode_utf16(&mut b)[0] as u32) % 360;
    }
    h
}

/// `Intl.DateTimeFormat(locale, opts).format(date)`; "hour12": "false" is a boolean.
pub fn fmt_date(ms: f64, locale: Option<&str>, opts: &[(&str, &str)]) -> String {
    let o = js_sys::Object::new();
    for (k, v) in opts {
        let val: JsValue = match *v {
            "false" => false.into(),
            "true" => true.into(),
            x => x.into(),
        };
        let _ = js_sys::Reflect::set(&o, &(*k).into(), &val);
    }
    let locs = js_sys::Array::new();
    if let Some(l) = locale {
        locs.push(&l.into());
    }
    let f = js_sys::Intl::DateTimeFormat::new(&locs, &o);
    f.format().call1(&JsValue::NULL, &js_sys::Date::new(&ms.into())).ok().and_then(|v| v.as_string()).unwrap_or_default()
}

/// HH:MM today, else "d Mon" (plus the year when it isn't this year).
pub fn when(ts: i64) -> String {
    let ms = ts as f64 * 1000.0;
    let d = js_sys::Date::new(&ms.into());
    let now = js_sys::Date::new_0();
    if d.to_date_string() == now.to_date_string() {
        return fmt_date(ms, None, &[("hour", "2-digit"), ("minute", "2-digit")]);
    }
    if d.get_full_year() != now.get_full_year() {
        fmt_date(ms, None, &[("day", "numeric"), ("month", "short"), ("year", "numeric")])
    } else {
        fmt_date(ms, None, &[("day", "numeric"), ("month", "short")])
    }
}

fn re(cell: &'static OnceLock<Regex>, src: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(src).expect("regex"))
}
pub fn bare_subject(x: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"(?i)^\s*((re|fwd?|rv|aw|sv)\s*:\s*)+").replace(x, "").trim().to_lowercase()
}

/// Group a newest-first list into conversations: the provider's thread id, else sender + bare subject.
pub fn threads(ms: &[Msg]) -> Vec<Thread> {
    let mut out: Vec<Thread> = vec![];
    let mut at: HashMap<String, usize> = HashMap::new();
    for m in ms {
        let k = format!("{}|{}|{}", m.account, m.folder, m.thrid.clone().unwrap_or_else(|| format!("{}|{}", m.from_addr, bare_subject(&m.subject))));
        match at.get(&k) {
            Some(&i) => {
                let t = &mut out[i];
                t.all.push(m.clone());
                t.m.unread |= m.unread;
                if t.m.extra.is_none() {
                    t.m.extra = m.extra.clone();
                }
            }
            None => {
                at.insert(k, out.len());
                out.push(single(m));
            }
        }
    }
    out
}

/// What an action on a thread row touches: a pin goes on the newest, read/unread only where needed, the rest on all.
pub fn items_of(t: &Thread, act: &str) -> Vec<Item> {
    if t.all.len() < 2 || act == "star" || act == "unstar" {
        return vec![t.item()];
    }
    t.all.iter().filter(|m| match act {
        "mark_read" => m.unread,
        "mark_unread" => !m.unread,
        _ => true,
    }).map(Msg::item).collect()
}
/// Rows a flag change actually changes ("Read" on 200 with 9 unread sends 9).
pub fn needs(act: &str, t: &Thread) -> Option<bool> {
    Some(match act {
        "mark_read" => t.m.unread,
        "mark_unread" => !t.m.unread,
        "star" => !t.m.starred,
        "unstar" => t.m.starred,
        _ => return None,
    })
}
pub fn undo_flag(act: &str) -> Option<&'static str> {
    Some(match act {
        "mark_read" => "mark_unread",
        "mark_unread" => "mark_read",
        "star" => "unstar",
        "unstar" => "star",
        _ => return None,
    })
}
/// The list after an action: gone actions drop the messages, flag actions set the flag.
pub fn apply_local(ms: &mut Vec<Msg>, items: &[Item], act: &str) {
    let ks: HashSet<String> = items.iter().map(Item::key).collect();
    if gone(act) {
        ms.retain(|m| !ks.contains(&m.key()));
        return;
    }
    for m in ms.iter_mut().filter(|m| ks.contains(&m.key())) {
        match act {
            "mark_read" => m.unread = false,
            "mark_unread" => m.unread = true,
            "star" => m.starred = true,
            "unstar" => m.starred = false,
            _ => {}
        }
    }
}

pub struct Done {
    pub done: u64,
    pub skipped: u64,
    pub errors: Vec<String>,
}
/// POST the action: one message through /message, several through /messages.
pub async fn post_action(items: &[Item], action: &str) -> Result<Done, String> {
    if items.len() == 1 {
        let mut body = items[0].json();
        body["action"] = json!(action);
        let r: Value = api_post("/mail/message", &body).await.map_err(|e| e.message)?;
        if r["ok"] != true {
            return Err(r["entry"]["detail"].as_str().unwrap_or("failed").to_string());
        }
        return Ok(Done { done: 1, skipped: 0, errors: vec![] });
    }
    let r: Value = api_post("/mail/messages", &json!({"items": items.iter().map(Item::json).collect::<Vec<_>>(), "action": action})).await.map_err(|e| e.message)?;
    let errors: Vec<String> = r["errors"].as_array().map(|a| a.iter().map(|e| e.as_str().map(str::to_string).unwrap_or_else(|| e.to_string())).collect()).unwrap_or_default();
    let done = r["done"].as_u64().unwrap_or(0);
    if !errors.is_empty() && done == 0 {
        return Err(errors[0].clone());
    }
    Ok(Done { done, skipped: r["skipped"].as_u64().unwrap_or(0), errors })
}

/// A rule in a sentence: "from x and subject y → Archive".
pub fn describe(r: &Value) -> String {
    let c = &r["conditions"];
    let mut parts: Vec<String> = vec![];
    for (k, pre) in [("from", "from "), ("to", "to "), ("subject", "subject ")] {
        if let Some(v) = c[k].as_str().filter(|v| !v.is_empty()) {
            parts.push(format!("{pre}{v}"));
        }
    }
    if let Some(v) = c["category"].as_str().filter(|v| !v.is_empty()) {
        parts.push(v.split(['|', ',']).map(|x| cat(x.trim())).collect::<Vec<_>>().join("/"));
    }
    if let Some(i) = r["instructions"].as_str().filter(|v| !v.is_empty()) {
        parts.push(format!("AI: {}", i.chars().take(40).collect::<String>()));
    }
    let acts: Vec<String> = r["actions"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| {
                    let mut t = action_label(x["type"].as_str().unwrap_or("")).to_string();
                    if let Some(l) = x["label"].as_str().filter(|l| !l.is_empty()) {
                        t += &format!(" \"{l}\"");
                    }
                    if let Some(f) = x["folder"].as_str().filter(|l| !l.is_empty()) {
                        t += &format!(" \"{f}\"");
                    }
                    t
                })
                .collect()
        })
        .unwrap_or_default();
    let join = if r["operator"] == "OR" { " or " } else { " and " };
    format!(
        "{} → {}",
        if parts.is_empty() { "no conditions".to_string() } else { parts.join(join) },
        if acts.is_empty() { "no actions".to_string() } else { acts.join(", ") }
    )
}

// ------------------------------------------------------------- replies ----
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub id: u64,
    pub account: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub body: String,
    pub reply_to: Option<Full>,
    pub forward_of: Option<Full>,
    pub ai_draft: bool,
}
pub fn quote(m: &Full) -> String {
    let text = m.body.replace('\r', "");
    let text = text.trim();
    format!(
        "\n\nOn {}, {} <{}> wrote:\n{}",
        m.date,
        if m.from_name.is_empty() { &m.from_address } else { &m.from_name },
        m.from_address,
        text.split('\n').map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n")
    )
}
pub const FWD_MARK: &str = "\n\n---------- Forwarded message ----------";
/// Split an address list on commas outside quotes.
fn addrs(x: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut q = false;
    for c in x.chars() {
        match c {
            '"' => {
                q = !q;
                cur.push(c)
            }
            ',' if !q => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}
fn bare(x: &str) -> String {
    match (x.find('<'), x.find('>')) {
        (Some(a), Some(b)) if b > a + 1 => x[a + 1..b].trim().to_lowercase(),
        _ => x.trim().to_lowercase(),
    }
}
fn starts_ci(x: &str, p: &str) -> bool {
    x.len() >= p.len() && x.is_char_boundary(p.len()) && x[..p.len()].eq_ignore_ascii_case(p)
}
/// Reply: quote the original. Reply all: Cc everyone but the sender and me. Forward: the forwarded header.
pub fn reply_draft(m: &Full, kind: &str, mine: &[String]) -> Draft {
    let subj = m.subject.clone();
    if kind == "forward" {
        let head = format!("{FWD_MARK}\nFrom: {} <{}>\nDate: {}\nSubject: {subj}\nTo: {}\n\n", m.from_name, m.from_address, m.date, m.to);
        return Draft {
            account: m.account.clone(),
            subject: if starts_ci(&subj, "fwd:") || starts_ci(&subj, "fw:") { subj } else { format!("Fwd: {subj}") },
            body: head + &m.body,
            forward_of: Some(m.clone()),
            ..Default::default()
        };
    }
    let mut cc = String::new();
    if kind == "all" {
        let mut seen: HashSet<String> = mine.iter().map(|x| x.to_lowercase()).collect();
        seen.insert(m.from_address.to_lowercase());
        let mut keep = vec![];
        for x in addrs(&m.to).into_iter().chain(addrs(&m.cc)) {
            if seen.insert(bare(&x)) {
                keep.push(x);
            }
        }
        cc = keep.join(", ");
    }
    Draft {
        account: m.account.clone(),
        to: m.from_address.clone(),
        cc,
        subject: if starts_ci(&subj, "re:") { subj } else { format!("Re: {subj}") },
        body: quote(m),
        reply_to: Some(m.clone()),
        ..Default::default()
    }
}

// ---------------------------------------------------------- dark emails ----
// Emails follow the theme: plain text takes the theme colours; an email that is already dark (its
// own page colour) is left as designed; any other HTML gets lightness inverted with hues kept and
// images flipped back (what Apple Mail and Gmail do). The background is pre-inverted so that, after
// the filter, it lands exactly on the theme colour.
pub fn rgb(c: &str) -> [f64; 3] {
    let mut out = [0.0; 3];
    let mut i = 0;
    let mut cur = String::new();
    for ch in c.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() || ch == '.' {
            cur.push(ch);
        } else if !cur.is_empty() {
            if i < 3 {
                out[i] = cur.parse().unwrap_or(0.0);
            }
            i += 1;
            cur.clear();
        }
    }
    out
}
pub fn lum(c: [f64; 3]) -> f64 {
    (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]) / 255.0
}
pub fn is_dark(bg: &str) -> bool {
    lum(rgb(bg)) < 0.5
}
fn pre_invert(c: [f64; 3]) -> String {
    let h = [[-0.574, 1.43, 0.144], [0.426, 0.43, 0.144], [0.426, 1.43, -0.856]];
    let v: Vec<String> = h.iter().map(|r| (255.0 - (r[0] * c[0] + r[1] * c[1] + r[2] * c[2]).clamp(0.0, 255.0)).round().to_string()).collect();
    format!("rgb({})", v.join(","))
}
/// The email's own page colour: <body bgcolor>, body style, or a body{} rule.
fn email_background(html: &str) -> Option<[f64; 3]> {
    static TAG: OnceLock<Regex> = OnceLock::new();
    static RULE: OnceLock<Regex> = OnceLock::new();
    static BG: OnceLock<Regex> = OnceLock::new();
    static ST: OnceLock<Regex> = OnceLock::new();
    static RU: OnceLock<Regex> = OnceLock::new();
    static HEX: OnceLock<Regex> = OnceLock::new();
    let tag = re(&TAG, r"(?i)<body\b[^>]*>").find(html).map(|m| m.as_str()).unwrap_or("");
    let rule = re(&RULE, r"(?i)\bbody\s*\{[^}]*\}").find(html).map(|m| m.as_str()).unwrap_or("");
    let val = re(&BG, r#"(?i)\bbgcolor\s*=\s*["']?([^"'\s>]+)"#)
        .captures(tag)
        .and_then(|c| c.get(1))
        .or_else(|| re(&ST, r#"(?i)background(?:-color)?\s*:\s*([^;"']+)"#).captures(tag).and_then(|c| c.get(1)))
        .or_else(|| re(&RU, r"(?i)background(?:-color)?\s*:\s*([^;}!]+)").captures(rule).and_then(|c| c.get(1)))?
        .as_str()
        .trim()
        .to_lowercase();
    if val == "black" {
        return Some([0.0; 3]);
    }
    if val == "white" {
        return Some([255.0; 3]);
    }
    if let Some(c) = re(&HEX, r"^#([0-9a-f]{3}|[0-9a-f]{6})\b").captures(&val) {
        let mut h = c[1].to_string();
        if h.len() == 3 {
            h = h.chars().flat_map(|c| [c, c]).collect();
        }
        let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0) as f64;
        return Some([p(0), p(2), p(4)]);
    }
    (val.starts_with("rgb(") || val.starts_with("rgba(")).then(|| rgb(&val))
}
pub struct Theme {
    pub bg: String,
    pub fg: String,
    pub link: String,
}
pub struct Look {
    pub scheme: &'static str,
    pub frame_bg: String,
    pub css: String,
}
pub fn mail_look(m: &Full, dark: bool, t: &Theme) -> Look {
    let base = "body{font:14px/1.5 system-ui,sans-serif;margin:12px;word-wrap:break-word}img{max-width:100%;height:auto}";
    if !dark {
        return Look { scheme: "light", frame_bg: "#fff".into(), css: format!("{base}body{{color:#222;background:#fff}}") };
    }
    if m.body_html.is_empty() {
        return Look { scheme: "dark", frame_bg: t.bg.clone(), css: format!("{base}html,body{{background:{};color:{}}}a{{color:{}}}", t.bg, t.fg, t.link) };
    }
    if let Some(own) = email_background(&m.body_html).filter(|o| lum(*o) < 0.4) {
        return Look { scheme: "light", frame_bg: format!("rgb({},{},{})", own[0], own[1], own[2]), css: base.into() };
    }
    let flip = "filter:invert(1) hue-rotate(180deg)";
    Look {
        scheme: "light",
        frame_bg: t.bg.clone(),
        // The filter goes on <body>: on <html> Chrome paints the page background unfiltered.
        css: format!(
            "{base}html{{background:{}}}body{{{flip};background:{} !important;color:#222;margin:0;padding:12px;min-height:100vh;box-sizing:border-box}}img,picture,video,canvas,svg image,[background],[style*=\"background-image\"]{{{flip}}}",
            t.bg,
            pre_invert(rgb(&t.bg))
        ),
    }
}
pub fn esc(x: &str) -> String {
    let mut o = String::with_capacity(x.len());
    for c in x.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            _ => o.push(c),
        }
    }
    o
}

/// A CSS colour (a token: `var(--bg)`) as plain rgb(), whatever it is written in (Studio's tokens are
/// oklch and color-mix): for canvases and frames that can't read CSS variables.
pub fn rgb_of(css: &str) -> String {
    use wasm_bindgen::JsCast;
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return "rgb(0, 0, 0)".into() };
    let Some(root) = doc.document_element() else { return "rgb(0, 0, 0)".into() };
    let Ok(probe) = doc.create_element("i") else { return "rgb(0, 0, 0)".into() };
    let _ = probe.set_attribute("style", &format!("color:{css}"));
    let _ = root.append_child(&probe);
    let computed = web_sys::window().and_then(|w| w.get_computed_style(&probe).ok().flatten()).and_then(|s| s.get_property_value("color").ok()).unwrap_or_default();
    probe.remove();
    if computed.starts_with("rgb") {
        let c = rgb(&computed);
        return format!("rgb({}, {}, {})", c[0], c[1], c[2]);
    }
    // oklch()/color(): a 1×1 canvas reads it back as rgb.
    thread_local! { static PX: Option<web_sys::CanvasRenderingContext2d> = None.or_else(|| {
        let c = web_sys::window()?.document()?.create_element("canvas").ok()?.dyn_into::<web_sys::HtmlCanvasElement>().ok()?;
        c.set_width(1);
        c.set_height(1);
        c.get_context("2d").ok()??.dyn_into().ok()
    }); }
    PX.with(|px| {
        let Some(px) = px else { return computed.clone() };
        px.clear_rect(0.0, 0.0, 1.0, 1.0);
        px.set_fill_style_str("#000");
        px.set_fill_style_str(&computed);
        px.fill_rect(0.0, 0.0, 1.0, 1.0);
        match px.get_image_data(0.0, 0.0, 1.0, 1.0) {
            Ok(d) => {
                let v = d.data();
                format!("rgb({}, {}, {})", v[0], v[1], v[2])
            }
            Err(_) => computed.clone(),
        }
    })
}
pub fn token_rgb(name: &str) -> String {
    rgb_of(&format!("var(--{name})"))
}

pub fn copy_text(t: &str) {
    if let Some(w) = web_sys::window() {
        let _ = w.navigator().clipboard().write_text(t);
    }
}

thread_local! {
    static QUIET: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}
/// After your own action the server's "mail changed" echo needn't reload the whole list.
pub fn hush() {
    QUIET.with(|q| q.set(js_sys::Date::now() + 2500.0));
}
pub fn hushed() -> bool {
    QUIET.with(|q| js_sys::Date::now() < q.get())
}

/// Second writing language from Settings (CERQ_WRITE_LANG2).
pub async fn lang2() -> String {
    thread_local! { static L2: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) }; }
    if let Some(l) = L2.with(|l| l.borrow().clone()) {
        return l;
    }
    let l = match api_get::<Value>("/settings").await {
        Ok(st) => st["fields"]
            .as_array()
            .and_then(|f| f.iter().find(|x| x["key"] == "CERQ_WRITE_LANG2"))
            .and_then(|x| x["value"].as_str())
            .filter(|v| !v.is_empty())
            .map(|v| v.split(" (").next().unwrap_or(v).to_string())
            .unwrap_or_else(|| "Spanish".into()),
        Err(_) => "Spanish".into(),
    };
    L2.with(|x| *x.borrow_mut() = Some(l.clone()));
    l
}
