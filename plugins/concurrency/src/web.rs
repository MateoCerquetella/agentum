//! Concurrency limit's web part: per-machine limits (the default limit is the manifest's setting,
//! which Settings → Plugins draws itself) and what runs on each machine now.
use agentum_plugin::web::WebPlugin;
use agentum_plugin::Manifest;
use agentum_proto::api::Overview;
use agentum_proto::ThreadState;
use agentum_ui::prelude::*;
use std::collections::BTreeMap;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    fn settings(&self) -> Option<AnyView> {
        Some(view! { <Machines/> }.into_any())
    }
}

#[component]
fn Machines() -> impl IntoView {
    // (machine id, name, running now)
    let machines = RwSignal::new(Vec::<(String, String, usize)>::new());
    let overrides = RwSignal::new(BTreeMap::<String, u64>::new());
    spawn_local(async move {
        if let Ok(o) = api_get::<Overview>("/code/overview").await {
            machines.set(o.machines.iter().map(|m| {
                let n = o.threads.iter().filter(|t| t.machine == m.id && matches!(t.state, ThreadState::Starting | ThreadState::Running)).count();
                (m.id.clone(), m.name.clone(), n)
            }).collect());
        }
        if let Ok(v) = rpc::<Value>("concurrency", "overrides", &json!({})).await {
            overrides.set(parse(&v));
        }
    });
    let set = move |machine: String, limit: Option<u64>| {
        spawn_local(async move {
            match rpc::<Value>("concurrency", "set", &json!({"machine": machine, "limit": limit})).await {
                Ok(v) => overrides.set(parse(&v)),
                Err(e) => toast_bad(e.message),
            }
        });
    };
    view! {
        <div class="col" style="gap:6px">
            <span class="small subdue">"Per machine (empty: the limit above)"</span>
            {move || {
                let list = machines.get();
                if list.is_empty() {
                    return view! { <span class="small muted">"No machines yet."</span> }.into_any();
                }
                list.into_iter().map(|(id, name, n)| {
                    let id2 = id.clone();
                    let cur = move || overrides.with(|o| o.get(&id2).map(|n| n.to_string()).unwrap_or_default());
                    let id3 = id.clone();
                    view! {
                        <div class="row" style="gap:8px">
                            <Icon name="server" size=14 class="muted"/>
                            <span class="grow">{name}<span class="small subdue">{format!(" · {n} running")}</span></span>
                            <input type="number" min="1" max="64" placeholder="default" style="width:90px" prop:value=cur
                                on:change=move |e| { let v = event_target_value(&e); set(id3.clone(), v.trim().parse::<u64>().ok()) }/>
                        </div>
                    }
                }).collect_view().into_any()
            }}
        </div>
    }
}

fn parse(v: &Value) -> BTreeMap<String, u64> {
    v["overrides"].as_object().map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_u64()?))).collect()).unwrap_or_default()
}
