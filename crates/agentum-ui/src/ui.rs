//! Toasts, confirm dialogs and modals. The app mounts [`UiLayer`] once; anything can call
//! [`toast`] or `confirm(...).await`.
use crate::icons::Icon;
use leptos::prelude::*;
use std::cell::RefCell;

#[derive(Clone)]
struct Toast {
    id: u64,
    text: String,
    bad: bool,
    on_click: Option<Callback<()>>,
}

#[derive(Clone)]
struct Confirm {
    title: String,
    body: String,
    ok: String,
    danger: bool,
}

thread_local! {
    static TOASTS: RwSignal<Vec<Toast>> = crate::global(vec![]);
    static DIALOG: RwSignal<Option<Confirm>> = crate::global(None);
    static ANSWER: RefCell<Option<futures_channel::Sender>> = const { RefCell::new(None) };
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

mod futures_channel {
    //! A one-shot answer for confirm() without pulling in a channel crate.
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::task::{Poll, Waker};
    #[derive(Default)]
    struct Slot {
        value: Option<bool>,
        waker: Option<Waker>,
    }
    pub struct Sender(Rc<RefCell<Slot>>);
    pub struct Receiver(Rc<RefCell<Slot>>);
    pub fn pair() -> (Sender, Receiver) {
        let s = Rc::new(RefCell::new(Slot::default()));
        (Sender(s.clone()), Receiver(s))
    }
    impl Sender {
        pub fn send(self, v: bool) {
            let mut s = self.0.borrow_mut();
            s.value = Some(v);
            if let Some(w) = s.waker.take() {
                w.wake();
            }
        }
    }
    impl std::future::Future for Receiver {
        type Output = bool;
        fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<bool> {
            let mut s = self.0.borrow_mut();
            match s.value.take() {
                Some(v) => Poll::Ready(v),
                None => {
                    s.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }
    }
}

fn push(text: String, bad: bool, on_click: Option<Callback<()>>) {
    let id = NEXT.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let ms = if on_click.is_some() { 8000 } else if bad { 6000 } else { 3500 };
    TOASTS.with(|t| t.update(|v| v.push(Toast { id, text, bad, on_click })));
    gloo_timers::callback::Timeout::new(ms, move || TOASTS.with(|t| t.update(|v| v.retain(|x| x.id != id)))).forget();
}

/// A short message at the bottom of the screen.
pub fn toast(text: impl Into<String>) {
    push(text.into(), false, None);
}
/// The same, in red (an error).
pub fn toast_bad(text: impl Into<String>) {
    push(text.into(), true, None);
}
/// A toast you can click (it does `f`).
pub fn toast_action(text: impl Into<String>, f: impl Fn() + Send + Sync + 'static) {
    push(text.into(), false, Some(Callback::new(move |_| f())));
}

/// Ask before doing something: `if confirm("Delete it?", "", "Delete", true).await { … }`.
pub async fn confirm(title: impl Into<String>, body: impl Into<String>, ok: impl Into<String>, danger: bool) -> bool {
    let (tx, rx) = futures_channel::pair();
    ANSWER.with(|a| {
        if let Some(old) = a.borrow_mut().replace(tx) {
            old.send(false);
        }
    });
    DIALOG.with(|d| d.set(Some(Confirm { title: title.into(), body: body.into(), ok: ok.into(), danger })));
    rx.await
}

fn answer(v: bool) {
    DIALOG.with(|d| d.set(None));
    ANSWER.with(|a| {
        if let Some(tx) = a.borrow_mut().take() {
            tx.send(v);
        }
    });
}

/// Toasts and the confirm dialog; mounted once by the app.
#[component]
pub fn UiLayer() -> impl IntoView {
    let ctx_menu = view! { <crate::picker::ContextMenuLayer/> };
    let toasts = TOASTS.with(|t| *t);
    let dialog = DIALOG.with(|d| *d);
    let _esc = window_event_listener(leptos::ev::keydown, move |e| {
        if e.key() == "Escape" && dialog.get_untracked().is_some() {
            answer(false);
        }
    });
    view! {
        {ctx_menu}
        <div class="toasts">
            <For each=move || toasts.get() key=|t| t.id let:t>
                {match t.on_click {
                    Some(cb) => {
                        let id = t.id;
                        view! {
                            <button type="button" class=if t.bad { "toast bad" } else { "toast" } on:click=move |_| {
                                cb.run(());
                                toasts.update(|v| v.retain(|x| x.id != id));
                            }>{t.text.clone()}</button>
                        }.into_any()
                    }
                    None => view! { <div class=if t.bad { "toast bad" } else { "toast" }>{t.text.clone()}</div> }.into_any(),
                }}
            </For>
        </div>
        {move || dialog.get().map(|d| view! {
            <div class="dialog-bg" on:mousedown=move |e| {
                if e.target() == e.current_target() { answer(false) }
            }>
                <div class="dialog" role="dialog" aria-modal="true">
                    <h3>{d.title.clone()}</h3>
                    {(!d.body.is_empty()).then(|| view! { <p class="muted">{d.body.clone()}</p> })}
                    <div class="row" style="justify-content:flex-end">
                        <button class="btn" on:click=move |_| answer(false)>"Cancel"</button>
                        <button class=if d.danger { "btn danger" } else { "btn primary" } autofocus on:click=move |_| answer(true)>{d.ok.clone()}</button>
                    </div>
                </div>
            </div>
        })}
    }
}

/// A dialog with a title, closed by ×, Escape or a click outside.
#[component]
pub fn Modal(#[prop(into)] title: String, on_close: Callback<()>, #[prop(optional)] wide: bool, children: Children) -> impl IntoView {
    let _esc = window_event_listener(leptos::ev::keydown, move |e| {
        if e.key() == "Escape" {
            on_close.run(())
        }
    });
    view! {
        <div class="dialog-bg" on:mousedown=move |e| { if e.target() == e.current_target() { on_close.run(()) } }>
            <div class="dialog" style=if wide { "width:min(860px,96vw)" } else { "" } role="dialog" aria-modal="true">
                <div class="row">
                    <h3 class="grow">{title}</h3>
                    <button class="icon-btn" aria-label="Close" on:click=move |_| on_close.run(())><Icon name="x"/></button>
                </div>
                {children()}
            </div>
        </div>
    }
}
