//! "Every write asks first": a confirm that shows what, when and which calendar as a list.
//! `ask("Move this event?", rows, "Move").await` → true when you said yes.
use agentum_ui::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::task::{Poll, Waker};

#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub title: String,
    pub rows: Vec<(String, String)>,
    pub ok: String,
}

#[derive(Default)]
struct Slot {
    value: Option<bool>,
    waker: Option<Waker>,
}
struct Answer(Rc<RefCell<Slot>>);
impl std::future::Future for Answer {
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

thread_local! {
    static OPEN: RwSignal<Option<Ask>> = agentum_ui::global(None);
    static SLOT: RefCell<Option<Rc<RefCell<Slot>>>> = const { RefCell::new(None) };
}

fn reply(v: bool) {
    OPEN.with(|o| o.set(None));
    if let Some(s) = SLOT.with(|s| s.borrow_mut().take()) {
        let mut s = s.borrow_mut();
        s.value = Some(v);
        if let Some(w) = s.waker.take() {
            w.wake();
        }
    }
}

pub async fn ask(title: impl Into<String>, rows: Vec<(String, String)>, ok: impl Into<String>) -> bool {
    reply(false); // an older question still open is a no
    let slot = Rc::new(RefCell::new(Slot::default()));
    SLOT.with(|s| *s.borrow_mut() = Some(slot.clone()));
    OPEN.with(|o| o.set(Some(Ask { title: title.into(), rows, ok: ok.into() })));
    Answer(slot).await
}

/// What / When / Calendar (+ more).
pub fn facts(what: &str, when: &str, cal: &str) -> Vec<(String, String)> {
    vec![("What".into(), what.into()), ("When".into(), when.into()), ("Calendar".into(), cal.into())]
}

/// The rows as the review list (`.cal-review`); the first is highlighted.
pub fn review_list(rows: Vec<(String, String)>) -> impl IntoView {
    view! {
        <dl class="cal-review">
            {rows.into_iter().enumerate().map(|(i, (k, v))| view! { <div><dt>{k}</dt><dd class=(i == 0).then_some("hi")>{v}</dd></div> }).collect_view()}
        </dl>
    }
}

/// Mounted once by the Calendar page.
#[component]
pub fn AskLayer() -> impl IntoView {
    let open = OPEN.with(|o| *o);
    on_cleanup(|| reply(false));
    move || {
        open.get().map(|a| {
            view! {
                <Modal title=a.title.clone() on_close=Callback::new(|_| reply(false))>
                    {review_list(a.rows.clone())}
                    <div class="row" style="justify-content:flex-end">
                        <button class="btn" on:click=|_| reply(false)>"Cancel"</button>
                        <button class="btn primary" autofocus on:click=|_| reply(true)>{a.ok.clone()}</button>
                    </div>
                </Modal>
            }
        })
    }
}
