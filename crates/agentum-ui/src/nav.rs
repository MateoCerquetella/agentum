//! Navigation. The URL is `/<left page>[?side=<right page>&<page params>]`: two panes side by
//! side, each page reading its own params. The app's shell does the opening; pages and plugins
//! call [`navigate`] and read params through [`use_params`] / [`use_param`].
use leptos::prelude::*;
use std::cell::RefCell;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    /// Where it is, if on screen; else replace the left pane.
    Replace,
    Left,
    Right,
}

type Opener = Box<dyn Fn(&str, Where, BTreeMap<String, String>)>;

thread_local! {
    static OPEN: RefCell<Option<Opener>> = const { RefCell::new(None) };
    static PARAMS: RwSignal<BTreeMap<String, String>> = RwSignal::new(read());
}

/// The query string of the current URL.
pub fn read() -> BTreeMap<String, String> {
    let search = web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default();
    let mut m = BTreeMap::new();
    if let Ok(p) = web_sys::UrlSearchParams::new_with_str(&search) {
        for e in js_sys::try_iter(&p.entries()).ok().flatten().into_iter().flatten().flatten() {
            let a = js_sys::Array::from(&e);
            if let (Some(k), Some(v)) = (a.get(0).as_string(), a.get(1).as_string()) {
                m.insert(k, v);
            }
        }
    }
    m
}

/// Called by the shell after it changes the URL (and on back/forward).
pub fn refresh() {
    PARAMS.with(|p| p.set(read()));
}

/// The shell registers how pages are opened.
pub fn set_opener(f: impl Fn(&str, Where, BTreeMap<String, String>) + 'static) {
    OPEN.with(|o| *o.borrow_mut() = Some(Box::new(f)));
}

/// Open a page (`"mail"`, `"code"`, `"p/usage"`…) with params.
pub fn navigate(page: &str, at: Where, params: &[(&str, &str)]) {
    let p: BTreeMap<String, String> = params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    OPEN.with(|o| {
        if let Some(f) = o.borrow().as_ref() {
            f(page, at, p)
        }
    });
}

/// The URL params, live.
pub fn use_params() -> Signal<BTreeMap<String, String>> {
    PARAMS.with(|p| (*p).into())
}

/// One URL param, live.
pub fn use_param(key: &'static str) -> Signal<Option<String>> {
    let p = PARAMS.with(|p| *p);
    Signal::derive(move || p.with(|m| m.get(key).cloned()))
}

/// Change one param without a new history entry (None removes it).
pub fn set_param(key: &str, value: Option<&str>) {
    let mut m = read();
    match value {
        Some(v) => {
            m.insert(key.to_string(), v.to_string());
        }
        None => {
            m.remove(key);
        }
    }
    let path = web_sys::window().and_then(|w| w.location().pathname().ok()).unwrap_or_else(|| "/".into());
    let q = query(&m);
    if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
        let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&format!("{path}{q}")));
    }
    refresh();
}

/// `?a=1&b=2` (empty for no params).
pub fn query(m: &BTreeMap<String, String>) -> String {
    let p = web_sys::UrlSearchParams::new().unwrap();
    for (k, v) in m {
        p.append(k, v);
    }
    let s: String = p.to_string().into();
    if s.is_empty() { s } else { format!("?{s}") }
}
