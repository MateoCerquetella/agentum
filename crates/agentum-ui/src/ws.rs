//! The page's one WebSocket to the hub (`/api/ws`).
//!
//! Everything live comes through it: thread summaries and events, machines, terminal output,
//! plugin messages and the personal space's events (mail, calendar…, channel "app").
//! [`subscribe`] a channel for as long as a view needs it; [`on_ws`] hears every message.
//! It reconnects by itself (0.5 s → 10 s) and re-subscribes; listeners get a `Reconnected`
//! signal through [`connected`] so they can refetch what they may have missed.
use agentum_proto::api::{WsIn, WsOut};
use leptos::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

type Listener = Rc<dyn Fn(&WsOut)>;

#[derive(Default)]
struct State {
    sock: Option<web_sys::WebSocket>,
    open: bool,
    subs: HashMap<String, u32>,
    listeners: Vec<(u64, Listener)>,
    next_id: u64,
    backoff_ms: u32,
    started: bool,
}

thread_local! {
    static WS: RefCell<State> = RefCell::new(State::default());
    static CONNECTED: RwSignal<u32> = crate::global(0);
}

/// Bumps every time the socket (re)opens: read it in an Effect to refetch after a gap.
pub fn connected() -> RwSignal<u32> {
    CONNECTED.with(|c| *c)
}

fn url() -> String {
    let loc = web_sys::window().unwrap().location();
    let proto = if loc.protocol().unwrap_or_default() == "https:" { "wss" } else { "ws" };
    format!("{proto}://{}/api/ws", loc.host().unwrap_or_default())
}

fn send_raw(msg: &WsIn) {
    WS.with(|w| {
        let w = w.borrow();
        if let (Some(s), true) = (&w.sock, w.open) {
            let _ = s.send_with_str(&serde_json::to_string(msg).unwrap_or_default());
        }
    });
}

/// Send a message to the hub (terminal keystrokes…). Dropped while disconnected.
pub fn send(msg: WsIn) {
    start();
    send_raw(&msg);
}

fn connect() {
    let Ok(sock) = web_sys::WebSocket::new(&url()) else { return retry() };
    let onopen = Closure::<dyn FnMut()>::new(move || {
        let chans: Vec<String> = WS.with(|w| {
            let mut w = w.borrow_mut();
            w.open = true;
            w.backoff_ms = 0;
            w.subs.keys().cloned().collect()
        });
        if !chans.is_empty() {
            send_raw(&WsIn::Sub { ch: chans });
        }
        connected().update(|n| *n += 1);
    });
    let onmsg = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
        let Some(text) = e.data().as_string() else { return };
        let Ok(msg) = serde_json::from_str::<WsOut>(&text) else { return };
        let ls: Vec<Listener> = WS.with(|w| w.borrow().listeners.iter().map(|(_, l)| l.clone()).collect());
        for l in ls {
            l(&msg);
        }
    });
    let onclose = Closure::<dyn FnMut()>::new(move || {
        WS.with(|w| {
            let mut w = w.borrow_mut();
            w.open = false;
            w.sock = None;
        });
        retry();
    });
    sock.set_onopen(Some(onopen.as_ref().unchecked_ref()));
    sock.set_onmessage(Some(onmsg.as_ref().unchecked_ref()));
    sock.set_onclose(Some(onclose.as_ref().unchecked_ref()));
    onopen.forget();
    onmsg.forget();
    onclose.forget();
    WS.with(|w| w.borrow_mut().sock = Some(sock));
}

fn retry() {
    let ms = WS.with(|w| {
        let mut w = w.borrow_mut();
        w.backoff_ms = (w.backoff_ms * 2).clamp(500, 10_000);
        w.backoff_ms
    });
    gloo_timers::callback::Timeout::new(ms, connect).forget();
}

fn start() {
    let first = WS.with(|w| {
        let mut w = w.borrow_mut();
        !std::mem::replace(&mut w.started, true)
    });
    if first {
        connect();
        // Keep the connection warm through proxies that drop idle sockets.
        gloo_timers::callback::Interval::new(25_000, || send_raw(&WsIn::Ping)).forget();
    }
}

/// Subscribe to channels while the calling view lives (unsubscribed on cleanup).
pub fn subscribe(ch: &[&str]) {
    start();
    let fresh: Vec<String> = WS.with(|w| {
        let mut w = w.borrow_mut();
        let mut fresh = vec![];
        for c in ch {
            let n = w.subs.entry(c.to_string()).or_insert(0);
            *n += 1;
            if *n == 1 {
                fresh.push(c.to_string());
            }
        }
        fresh
    });
    if !fresh.is_empty() {
        send_raw(&WsIn::Sub { ch: fresh });
    }
    let owned: Vec<String> = ch.iter().map(|s| s.to_string()).collect();
    on_cleanup(move || {
        let gone: Vec<String> = WS.with(|w| {
            let mut w = w.borrow_mut();
            let mut gone = vec![];
            for c in &owned {
                if let Some(n) = w.subs.get_mut(c) {
                    *n -= 1;
                    if *n == 0 {
                        w.subs.remove(c);
                        gone.push(c.clone());
                    }
                }
            }
            gone
        });
        if !gone.is_empty() {
            send_raw(&WsIn::Unsub { ch: gone });
        }
    });
}

/// Hear every message while the calling view lives.
pub fn on_ws(f: impl Fn(&WsOut) + 'static) {
    start();
    let id = WS.with(|w| {
        let mut w = w.borrow_mut();
        w.next_id += 1;
        let id = w.next_id;
        w.listeners.push((id, Rc::new(f)));
        id
    });
    on_cleanup(move || WS.with(|w| w.borrow_mut().listeners.retain(|(i, _)| *i != id)));
}
