//! Sign in: a password, or a passkey (Touch ID / Face ID) where the browser has them.
use agentum_ui::prelude::*;

#[component]
pub fn Login(on_done: Callback<Value>) -> impl IntoView {
    let user = RwSignal::new("admin".to_string());
    let pass = RwSignal::new(String::new());
    let err = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let submit = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        busy.set(true);
        err.set(String::new());
        let body = json!({"username": user.get_untracked(), "password": pass.get_untracked()});
        spawn_local(async move {
            match api_post::<Value>("/auth/login", &body).await {
                Ok(u) => on_done.run(u),
                Err(e) => err.set(e.message),
            }
            busy.set(false);
        });
    };
    let passkey = move |_| {
        busy.set(true);
        err.set(String::new());
        spawn_local(async move {
            match crate::pages::settings::passkey_sign_in().await {
                Ok(u) => on_done.run(u),
                Err(e) if e.is_empty() => {}
                Err(e) => err.set(e),
            }
            busy.set(false);
        });
    };
    view! {
        <div style="height:100%;display:grid;place-items:center;padding:16px">
            <form class="card col" style="width:min(340px,100%);padding:24px;gap:14px" on:submit=submit>
                <div class="row" style="gap:10px;margin-bottom:4px">
                    <img src="/icon-192.png" width="28" height="28" alt=""/>
                    <span class="hi" style="font-size:18px;letter-spacing:.06em">"Agentum"</span>
                </div>
                <label class="field"><span>"Username"</span>
                    <input autocomplete="username" prop:value=user on:input=move |e| user.set(event_target_value(&e))/></label>
                <label class="field"><span>"Password"</span>
                    <input type="password" autocomplete="current-password" autofocus prop:value=pass on:input=move |e| pass.set(event_target_value(&e))/></label>
                <Show when=move || !err.get().is_empty()><div class="err small">{move || err.get()}</div></Show>
                <button class="btn primary" disabled=move || busy.get() || pass.get().is_empty() style="justify-content:center">
                    {move || if busy.get() { "Signing in…" } else { "Sign in" }}
                </button>
                <Show when=crate::pages::settings::passkeys_work>
                    <button type="button" class="btn" disabled=move || busy.get() style="justify-content:center" on:click=passkey>
                        "Sign in with Touch ID / Face ID"
                    </button>
                </Show>
            </form>
        </div>
    }
}
