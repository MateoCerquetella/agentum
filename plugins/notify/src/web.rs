//! Notifications' web part: the settings are the manifest's (drawn by Settings → Plugins); this
//! adds "Send a test".
use agentum_plugin::web::WebPlugin;
use agentum_plugin::Manifest;
use agentum_ui::prelude::*;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    fn settings(&self) -> Option<AnyView> {
        let busy = RwSignal::new(false);
        let test = move |_| {
            busy.set(true);
            spawn_local(async move {
                match rpc::<Value>("notify", "test", &json!({})).await {
                    Ok(v) if v["quiet"] == true => toast("Sent. (Quiet hours are on now: threads stay silent until quiet hours end.)"),
                    Ok(_) => toast("Sent: check your devices."),
                    Err(e) => toast_bad(e.message),
                }
                busy.set(false);
            });
        };
        Some(view! {
            <div class="row" style="gap:8px">
                <button class="btn" disabled=move || busy.get() on:click=test><Icon name="bell" size=14/>"Send a test"</button>
                <span class="small subdue">"Pushes go to the devices that allowed notifications, and a toast shows in open pages."</span>
            </div>
        }.into_any())
    }
}
