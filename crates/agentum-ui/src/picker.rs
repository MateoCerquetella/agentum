//! BB-style pickers: a chip that opens a popover list (searchable, keyboard: ↑↓ ⏎ Esc), and a
//! menu of actions (a ⋯ button, or a right-click menu at the pointer).
use crate::icons::Icon;
use crate::nav::PaneAction;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct PickItem {
    pub id: String,
    pub label: String,
    /// A muted second part (a path, "offline", a version).
    pub sub: String,
    pub icon: &'static str,
    /// Items with the same group are listed under its name.
    pub group: String,
    pub disabled: bool,
    /// The current choice (a check mark).
    pub on: bool,
    /// Picking it leaves the menu open (a mode switch: "Existing worktree").
    pub keep_open: bool,
}

fn contains(root: &NodeRef<leptos::html::Div>, target: Option<web_sys::EventTarget>) -> bool {
    let (Some(root), Some(t)) = (root.get_untracked(), target) else { return false };
    t.dyn_ref::<web_sys::Node>().is_some_and(|n| root.contains(Some(n)))
}

/// A chip and its popover list. `children` is what the chip shows.
#[component]
pub fn Picker(
    #[prop(into)] items: Signal<Vec<PickItem>>,
    on_pick: Callback<String>,
    children: Children,
    #[prop(optional)] search: bool,
    #[prop(optional, into)] class: String,
    /// Open upwards (a chip at the bottom of the screen).
    #[prop(optional)]
    up: bool,
    #[prop(optional, into)] title: String,
    #[prop(optional, into)] placeholder: Option<String>,
    /// Extra rows at the end of the popover (an "Add…" form).
    #[prop(optional)]
    footer: Option<ViewFn>,
    #[prop(optional, into)] disabled: Signal<bool>,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let q = RwSignal::new(String::new());
    let sel = RwSignal::new(0usize);
    let root = NodeRef::<leptos::html::Div>::new();
    let input = NodeRef::<leptos::html::Input>::new();
    let shown = Memo::new(move |_| {
        let ql = q.get().to_lowercase();
        items.get().into_iter().filter(|i| ql.is_empty() || i.label.to_lowercase().contains(&ql) || i.sub.to_lowercase().contains(&ql) || i.group.to_lowercase().contains(&ql)).collect::<Vec<_>>()
    });
    crate::on_window(leptos::ev::mousedown, move |e| {
        if open.get_untracked() && !contains(&root, e.target()) {
            open.set(false);
        }
    });
    Effect::new(move |_| {
        if open.get() {
            q.set(String::new());
            sel.set(shown.get_untracked().iter().position(|i| i.on).unwrap_or(0));
            if let Some(i) = input.get() {
                let _ = i.focus();
            }
        }
    });
    let pick = move |i: usize| {
        if let Some(it) = shown.get_untracked().get(i).filter(|it| !it.disabled) {
            on_pick.run(it.id.clone());
            if !it.keep_open {
                open.set(false);
            }
        }
    };
    let key = move |e: leptos::ev::KeyboardEvent| match e.key().as_str() {
        "ArrowDown" => {
            e.prevent_default();
            sel.update(|s| *s = (*s + 1).min(shown.get_untracked().len().saturating_sub(1)))
        }
        "ArrowUp" => {
            e.prevent_default();
            sel.update(|s| *s = s.saturating_sub(1))
        }
        "Enter" => {
            e.prevent_default();
            pick(sel.get_untracked())
        }
        "Escape" => {
            e.prevent_default();
            open.set(false)
        }
        _ => {}
    };
    let ph = placeholder.unwrap_or_else(|| "Search".into());
    view! {
        <div class="pk" node_ref=root on:keydown=key>
            <button type="button" class=format!("pk-chip {class}") title=title disabled=move || disabled.get() aria-expanded=move || open.get().to_string()
                on:click=move |_| open.update(|o| *o = !*o)>
                {children()}
                <Icon name="chevron" size=12 class="pk-chev"/>
            </button>
            <Show when=move || open.get()>
                <div class=if up { "pk-pop up" } else { "pk-pop" } role="listbox">
                    {search.then(|| view! { <input class="pk-search" node_ref=input placeholder=ph.clone() prop:value=q on:input=move |e| { q.set(event_target_value(&e)); sel.set(0) }/> })}
                    <div class="pk-list">
                        {move || {
                            let v = shown.get();
                            if v.is_empty() {
                                return view! { <div class="pk-empty">"Nothing matches."</div> }.into_any();
                            }
                            let mut last = String::new();
                            v.into_iter().enumerate().map(|(i, it)| {
                                let head = (it.group != last && !it.group.is_empty()).then(|| it.group.clone());
                                last = it.group.clone();
                                view! {
                                    {head.map(|g| view! { <div class="pk-group">{g}</div> })}
                                    <button type="button" class=move || format!("pk-item{}{}", if sel.get() == i { " sel" } else { "" }, if it.disabled { " off" } else { "" })
                                        disabled=it.disabled on:mouseenter=move |_| sel.set(i) on:click=move |_| pick(i)>
                                        {(!it.icon.is_empty()).then(|| view! { <Icon name=it.icon size=14/> })}
                                        <span class="pk-label">{it.label.clone()}</span>
                                        {(!it.sub.is_empty()).then(|| view! { <span class="pk-sub">{it.sub.clone()}</span> })}
                                        {it.on.then(|| view! { <Icon name="check" size=13 class="pk-check"/> })}
                                    </button>
                                }
                            }).collect_view().into_any()
                        }}
                    </div>
                    {footer.as_ref().map(|f| view! { <div class="pk-foot">{f.run()}</div> })}
                </div>
            </Show>
        </div>
    }
}

/// A ⋯ (or any) button with a menu of actions.
#[component]
pub fn Menu(#[prop(into)] items: Signal<Vec<PaneAction>>, #[prop(optional, into)] icon: Option<String>, #[prop(optional, into)] title: String) -> impl IntoView {
    let open = RwSignal::new(false);
    let root = NodeRef::<leptos::html::Div>::new();
    crate::on_window(leptos::ev::mousedown, move |e| {
        if open.get_untracked() && !contains(&root, e.target()) {
            open.set(false);
        }
    });
    let icon = icon.unwrap_or_else(|| "more".into());
    view! {
        <div class="pk" node_ref=root>
            <button type="button" class="icon-btn" title=title aria-expanded=move || open.get().to_string() on:click=move |_| open.update(|o| *o = !*o)><Icon name=icon.clone()/></button>
            <Show when=move || open.get()>
                <div class="pk-pop right" role="menu">
                    <div class="pk-list">{move || items.get().into_iter().map(|a| {
                        let run = a.run;
                        view! {
                            <button type="button" class=if a.danger { "pk-item danger" } else { "pk-item" } on:click=move |_| { open.set(false); run.run(()) }>
                                <Icon name=a.icon size=14/><span class="pk-label">{a.label.clone()}</span>
                            </button>
                        }
                    }).collect_view()}</div>
                </div>
            </Show>
        </div>
    }
}

thread_local! {
    static CTX: RwSignal<Option<(i32, i32, Vec<PaneAction>)>> = crate::global(None);
}

/// A right-click menu at the pointer.
pub fn context_menu(e: &web_sys::MouseEvent, items: Vec<PaneAction>) {
    e.prevent_default();
    CTX.with(|c| c.set(Some((e.client_x(), e.client_y(), items))));
}

/// Mounted once by the app (in the UI layer).
#[component]
pub fn ContextMenuLayer() -> impl IntoView {
    let c = CTX.with(|c| *c);
    crate::on_window(leptos::ev::mousedown, move |e| {
        let inside = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).is_some_and(|el| el.closest(".ctx-menu").ok().flatten().is_some());
        if !inside {
            c.set(None);
        }
    });
    crate::on_window(leptos::ev::keydown, move |e| {
        if e.key() == "Escape" {
            c.set(None)
        }
    });
    move || {
        c.get().map(|(x, y, items)| {
            let vw = web_sys::window().and_then(|w| w.inner_width().ok()).and_then(|v| v.as_f64()).unwrap_or(1200.0) as i32;
            let vh = web_sys::window().and_then(|w| w.inner_height().ok()).and_then(|v| v.as_f64()).unwrap_or(800.0) as i32;
            let (x, y) = (x.min(vw - 230), y.min(vh - 40 - 34 * items.len() as i32));
            view! {
                <div class="ctx-menu pk-pop" role="menu" style=format!("position:fixed;left:{x}px;top:{y}px")>
                    <div class="pk-list">{items.into_iter().map(|a| {
                        let run = a.run;
                        view! {
                            <button type="button" class=if a.danger { "pk-item danger" } else { "pk-item" } on:click=move |_| { c.set(None); run.run(()) }>
                                <Icon name=a.icon size=14/><span class="pk-label">{a.label.clone()}</span>
                            </button>
                        }
                    }).collect_view()}</div>
                </div>
            }
        })
    }
}
