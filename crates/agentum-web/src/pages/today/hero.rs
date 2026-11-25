//! Today's hero: the ASCII sky with date · place, the greeting, the temperature counting up, and
//! the landscape button among the page's tools. Also the headline typed out.
use super::sky::{self, Sky};
use super::Tx;
use agentum_ui::prelude::*;
use send_wrapper::SendWrapper;

fn still() -> bool {
    web_sys::window().and_then(|w| w.match_media("(prefers-reduced-motion: reduce)").ok().flatten()).is_some_and(|m| m.matches())
}
pub fn greeting(c: Tx) -> String {
    let h = js_sys::Date::new_0().get_hours();
    c.t(if !(4..19).contains(&h) { "Good evening" } else if h < 12 { "Good morning" } else { "Good afternoon" })
}

#[component]
pub fn Hero(b: Signal<Option<Value>>, c: Tx, #[prop(into)] before: ViewFn, #[prop(into)] after: ViewFn) -> impl IntoView {
    let hero = NodeRef::<leptos::html::Section>::new();
    let cv = NodeRef::<leptos::html::Canvas>::new();
    let engine = StoredValue::new(None::<SendWrapper<Sky>>);
    let scene = RwSignal::new(sky::first_scene());
    let swap = RwSignal::new(false);
    Effect::new(move |_| {
        let (Some(h), Some(canvas)) = (hero.get(), cv.get()) else { return };
        if engine.with_value(Option::is_some) {
            return;
        }
        sky::remember_scene(scene.get_untracked());
        let el: web_sys::HtmlElement = h.into();
        if let Some(e) = Sky::start(canvas.into(), el, scene.get_untracked()) {
            engine.set_value(Some(SendWrapper::new(e)));
        }
    });
    on_cleanup(move || {
        let _ = engine.try_update_value(|e| *e = None);
    });
    // The forecast drives the sky; a new look redraws it in the new colours.
    Effect::new(move |_| {
        let w = b.with(|b| b.as_ref().map(|b| b["weather"].clone()).unwrap_or(Value::Null));
        let s = sky::sky_from(&w);
        engine.with_value(|e| {
            if let Some(e) = e {
                e.set_sky(s)
            }
        });
    });
    let look = crate::pages::theme::look_signal();
    Effect::new(move |prev: Option<()>| {
        look.track();
        if prev.is_some() {
            let t = gloo_timers::callback::Timeout::new(40, move || {
                let _ = engine.try_with_value(|e| {
                    if let Some(e) = e {
                        e.restart()
                    }
                });
            });
            t.forget();
        }
    });
    // Another landscape: fade out, swap, fade in.
    let next = move |_| {
        let s = sky::pick_scene(Some(scene.get_untracked()));
        scene.set(s);
        sky::remember_scene(s);
        swap.set(true);
        gloo_timers::callback::Timeout::new(350, move || {
            let _ = engine.try_with_value(|e| {
                if let Some(e) = e {
                    e.set_scene(s)
                }
            });
            let _ = swap.try_set(false);
        })
        .forget();
    };
    let tick = RwSignal::new(0u32);
    let iv = SendWrapper::new(gloo_timers::callback::Interval::new(60_000, move || tick.update(|n| *n += 1)));
    on_cleanup(move || drop(iv));
    view! {
        <section class="td-hero" node_ref=hero>
            <canvas node_ref=cv class=move || if swap.get() { "swap" } else { "" } aria-hidden="true"/>
            <div class="td-fade"/>
            <div class="td-head">
                <div class="td-kicker">{move || {
                    let s = b.with(|b| b.as_ref().map(|b| [b["date"].as_str().unwrap_or(""), b["weather"]["place"].as_str().unwrap_or("")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")).unwrap_or_default());
                    if s.is_empty() { "\u{a0}".to_string() } else { s }
                }}</div>
                <h1>{move || { tick.track(); greeting(c) }}</h1>
                {move || b.with(|b| b.as_ref().and_then(|b| b["weather"].as_object().map(|_| b["weather"].clone()))).map(|w| view! {
                    <div class="td-temp"><CountUp to=w["now"].as_f64().unwrap_or(f64::NAN).round()/>"°"<small>{format!("{} · {} {}°", w["sky"].as_str().unwrap_or(""), c.t("feels"), w["feels"])}</small></div>
                })}
            </div>
            <div class="td-tools" on:click=|e| e.stop_propagation()>
                {before.run()}
                <button on:click=next title=move || format!("{} · {}", sky::scene_label(scene.get()), c.t("Another landscape")) aria-label=move || c.t("Another landscape")><Icon name="landscape" size=15/></button>
                {after.run()}
            </div>
        </section>
    }
}

#[component]
fn CountUp(to: f64) -> impl IntoView {
    let n = RwSignal::new(if still() || !to.is_finite() { to } else { 0.0 });
    if !still() && to.is_finite() {
        let t0 = js_sys::Date::now();
        let iv = StoredValue::new(None::<SendWrapper<gloo_timers::callback::Interval>>);
        iv.set_value(Some(SendWrapper::new(gloo_timers::callback::Interval::new(30, move || {
            let k = ((js_sys::Date::now() - t0) / 900.0).min(1.0);
            let _ = n.try_set((to * (1.0 - (1.0 - k).powi(3))).round());
            if k >= 1.0 {
                let _ = iv.try_set_value(None);
            }
        }))));
    }
    view! { <span>{move || { let v = n.get(); if v.is_finite() { v.to_string() } else { "–".into() } }}</span> }
}

/// The headline typed out, with a blinking cursor once it's written.
#[component]
pub fn Typed(text: String) -> impl IntoView {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let i = RwSignal::new(if still() { len } else { 0 });
    if !still() && len > 0 {
        let chars2 = chars.clone();
        fn step(i: RwSignal<usize>, chars: std::rc::Rc<Vec<char>>, delay: u32) {
            gloo_timers::callback::Timeout::new(delay, move || {
                let Some(k) = i.try_get_untracked() else { return };
                let k = k + if chars.get(k) == Some(&' ') { 1 } else if js_sys::Math::random() < 0.15 { 2 } else { 1 };
                let k = k.min(chars.len());
                let _ = i.try_set(k);
                if k < chars.len() {
                    step(i, chars, (16.0 + js_sys::Math::random() * 26.0) as u32);
                }
            })
            .forget();
        }
        step(i, std::rc::Rc::new(chars2), 650);
    }
    view! {
        <span>{move || chars.iter().take(i.get()).collect::<String>()}</span>
        <span class=move || if i.get() < len { "td-cursor typing" } else { "td-cursor" }/>
    }
}
