//! Navigation. The URL is `/<left page>?<its params>&side=<right page>&r_<its params>`: two panes
//! side by side, any page on either side (two threads, mail beside a thread, a thread beside its
//! terminal). A page reads and writes its own params through [`use_param`] / [`set_param`]; in the
//! right pane they are the `r_` ones without it knowing. The shell does the opening; pages and
//! plugins call [`navigate`] ([`here`] = the pane they are in) and set their pane's title and
//! actions through [`pane`].
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

/// An action a page puts in its pane's header (an icon button, or an entry of its ⋯ menu).
#[derive(Clone)]
pub struct PaneAction {
    pub icon: &'static str,
    pub label: String,
    pub run: Callback<()>,
    /// Shown pressed (a toggle that is on).
    pub on: Option<Signal<bool>>,
    pub danger: bool,
}
impl PaneAction {
    pub fn new(icon: &'static str, label: impl Into<String>, run: impl Fn() + Send + Sync + 'static) -> Self {
        PaneAction { icon, label: label.into(), run: Callback::new(move |_| run()), on: None, danger: false }
    }
    pub fn toggled(mut self, on: Signal<bool>) -> Self {
        self.on = Some(on);
        self
    }
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

/// The pane a page is drawn in, given by the shell as context.
#[derive(Clone, Copy)]
pub struct PaneCtx {
    pub right: bool,
    /// Replaces the page's name in the pane header (a thread's title).
    pub title: RwSignal<Option<String>>,
    /// A muted line beside the title (machine · project · branch).
    pub sub: RwSignal<Option<String>>,
    /// Icon buttons in the header, before maximize/close.
    pub actions: RwSignal<Vec<PaneAction>>,
    /// The header's ⋯ menu.
    pub menu: RwSignal<Vec<PaneAction>>,
    /// What the header's + launcher offers for this page (a thread's Files, Git, Terminal…),
    /// besides the app's pages.
    pub tools: RwSignal<Vec<PaneTool>>,
}

/// A tool in the pane header's + launcher: clicked, it opens here (`open`) or beside; dragged
/// onto a half, it opens there as `page` with `params`.
#[derive(Clone)]
pub struct PaneTool {
    pub label: String,
    pub icon: &'static str,
    pub page: &'static str,
    pub params: Vec<(String, String)>,
    pub open: Option<Callback<()>>,
}
impl PaneCtx {
    pub fn new(right: bool) -> Self {
        PaneCtx { right, title: RwSignal::new(None), sub: RwSignal::new(None), actions: RwSignal::new(vec![]), menu: RwSignal::new(vec![]), tools: RwSignal::new(vec![]) }
    }
    /// Safe after the pane is gone (a page's cleanup can run after its pane's).
    pub fn clear(&self) {
        let _ = self.title.try_set(None);
        let _ = self.sub.try_set(None);
        let _ = self.actions.try_set(vec![]);
        let _ = self.menu.try_set(vec![]);
        let _ = self.tools.try_set(vec![]);
    }
}

/// The pane the calling component is in (call while building a view, not in a handler).
pub fn pane() -> Option<PaneCtx> {
    use_context::<PaneCtx>()
}
fn right_pane() -> bool {
    match pane() {
        Some(p) => p.right,
        None => ACTIVE_RIGHT.with(|a| a.get()),
    }
}

/// The shell says which pane was last touched.
pub fn set_active_pane(right: bool) {
    ACTIVE_RIGHT.with(|a| a.set(right));
}

/// Where "this pane" is, for [`navigate`] (call while building a view).
pub fn here() -> Where {
    if right_pane() { Where::Right } else { Where::Left }
}

type Opener = Box<dyn Fn(&str, Where, BTreeMap<String, String>)>;

thread_local! {
    static OPEN: RefCell<Option<Opener>> = const { RefCell::new(None) };
    /// The pane last touched (pointer, keys, focus): what "this pane" means in an event handler,
    /// where there is no component context. Kept by the shell.
    static ACTIVE_RIGHT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PARAMS: RwSignal<BTreeMap<String, String>> = crate::global(read());
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

/// This pane's URL params, live.
pub fn use_params() -> Signal<BTreeMap<String, String>> {
    let p = PARAMS.with(|p| *p);
    let right = right_pane();
    Signal::derive(move || p.with(|m| pane_params(m, right)))
}

/// A pane's params out of the whole query: the `r_` ones (stripped) for the right pane.
pub fn pane_params(m: &BTreeMap<String, String>, right: bool) -> BTreeMap<String, String> {
    if right {
        m.iter().filter_map(|(k, v)| k.strip_prefix("r_").map(|k| (k.to_string(), v.clone()))).collect()
    } else {
        m.iter().filter(|(k, _)| !k.starts_with("r_") && k.as_str() != "side").map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

/// One of this pane's URL params, live (call while building a view).
pub fn use_param(key: &'static str) -> Signal<Option<String>> {
    let p = PARAMS.with(|p| *p);
    let key = if right_pane() { format!("r_{key}") } else { key.to_string() };
    Signal::derive(move || p.with(|m| m.get(&key).cloned()))
}

/// Change one of this pane's params without a new history entry (None removes it). Call while
/// building a view, or pass `right` yourself with [`set_param_in`].
pub fn set_param(key: &str, value: Option<&str>) {
    set_param_in(right_pane(), key, value)
}

/// [`set_param`] for a given pane.
pub fn set_param_in(right: bool, key: &str, value: Option<&str>) {
    let key = if right { format!("r_{key}") } else { key.to_string() };
    let key = key.as_str();
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
