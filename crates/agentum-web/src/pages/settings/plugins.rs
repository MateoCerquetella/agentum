//! Plugins, by group: each one on or off (`PUT /api/plugins/<id> {enabled}`), its plain settings
//! from its manifest (`{settings: {key: value}}`), and the settings view a plugin draws itself.
use agentum_plugin::{Group, Manifest, SettingDef, SettingKind};
use agentum_ui::prelude::*;
use std::collections::BTreeMap;

const GROUPS: [(Group, &str, &str); 4] = [
    (Group::Code, "Code", "Git, pull requests, files, the editor, history, terminals."),
    (Group::Thread, "Around a thread", "Hover cards, timestamps, drafts, nudges, side chats."),
    (Group::Usage, "Usage & limits", "Usage, context, limits, notifications."),
    (Group::Looks, "Looks", "Themes, the aura, generative UI, the browser."),
];

#[component]
pub fn Plugins() -> impl IntoView {
    // id → {enabled, settings} as the hub has them.
    let data = RwSignal::new(None::<BTreeMap<String, Value>>);
    let load = move || {
        spawn_local(async move {
            match api_get::<Vec<Value>>("/plugins").await {
                Ok(v) => data.set(Some(v.into_iter().filter_map(|p| Some((p["id"].as_str()?.to_string(), p))).collect())),
                Err(e) => toast_bad(e.message),
            }
        })
    };
    load();
    let mut sections: Vec<(&'static Manifest, Option<AnyView>)> = crate::plugins::settings_sections();
    let mut by_group: Vec<(Group, &str, &str, Vec<(&'static Manifest, Option<AnyView>)>)> = GROUPS.iter().map(|(g, t, n)| (*g, *t, *n, vec![])).collect();
    for s in sections.drain(..) {
        if let Some(g) = by_group.iter_mut().find(|g| g.0 == s.0.group) {
            g.3.push(s);
        }
    }
    view! {
        <div class="col ag-plugins" style="gap:22px">
            <Show when=move || data.with(|d| d.is_none())><div class="muted small">"Loading…"</div></Show>
            {by_group.into_iter().filter(|g| !g.3.is_empty()).map(|(_, title, note, list)| view! {
                <section class="ag-plugin-group">
                    <h3>{title}</h3>
                    <p class="small subdue">{note}</p>
                    {list.into_iter().map(|(m, own)| view! { <PluginCard m=m own=own data=data/> }).collect_view()}
                </section>
            }).collect_view()}
        </div>
    }
}

#[component]
fn PluginCard(m: &'static Manifest, own: Option<AnyView>, data: RwSignal<Option<BTreeMap<String, Value>>>) -> impl IntoView {
    let id = m.id;
    let enabled = move || data.with(|d| d.as_ref().and_then(|d| d.get(id)).map(|p| p["enabled"] == true).unwrap_or(m.default_on));
    let toggle = move |on: bool| {
        data.update(|d| {
            if let Some(p) = d.as_mut().and_then(|d| d.get_mut(id)) {
                p["enabled"] = json!(on);
            }
        });
        crate::plugins::off().update(|o| {
            o.retain(|x| x != id);
            if !on {
                o.push(id.to_string());
            }
        });
        spawn_local(async move {
            match api_put::<Value>(&format!("/plugins/{id}"), &json!({"enabled": on})).await {
                Ok(_) => toast(format!("{} is {}.", m.name, if on { "on" } else { "off" })),
                Err(e) => toast_bad(e.message),
            }
        });
    };
    let has_body = !m.settings.is_empty() || own.is_some();
    view! {
        <div class=move || if enabled() { "ag-plugin on" } else { "ag-plugin" }>
            <div class="row ag-plugin-head">
                <Icon name=m.icon size=16 class="muted"/>
                <div class="grow col" style="gap:1px;min-width:0">
                    <span class="hi">{m.name}</span>
                    <span class="small muted">{m.description}</span>
                </div>
                <label class="ag-switch" title=move || if enabled() { "On: click to turn off" } else { "Off: click to turn on" }>
                    <input type="checkbox" role="switch" aria-label=format!("{} on", m.name) prop:checked=enabled on:change=move |e| toggle(event_target_checked(&e))/>
                    <span/>
                </label>
            </div>
            <div class="ag-plugin-body" style=move || if enabled() && has_body { "" } else { "display:none" }>
                {m.settings.iter().map(|s| view! { <SettingRow plugin=id def=s data=data/> }).collect_view()}
                {own.map(|v| view! { <div class="ag-plugin-own">{v}</div> })}
            </div>
        </div>
    }
}

#[component]
fn SettingRow(plugin: &'static str, def: &'static SettingDef, data: RwSignal<Option<BTreeMap<String, Value>>>) -> impl IntoView {
    let key = def.key;
    let value = move || {
        data.with(|d| d.as_ref().and_then(|d| d.get(plugin)).map(|p| p["settings"][key].clone()))
            .filter(|v| !v.is_null())
            .unwrap_or_else(|| serde_json::from_str(def.default).unwrap_or(Value::Null))
    };
    let save = move |v: Value| {
        data.update(|d| {
            if let Some(p) = d.as_mut().and_then(|d| d.get_mut(plugin)) {
                p["settings"][key] = v.clone();
            }
        });
        spawn_local(async move {
            if let Err(e) = api_put::<Value>(&format!("/plugins/{plugin}"), &json!({"settings": {key: v}})).await {
                toast_bad(e.message);
            }
        });
    };
    let ctl = match def.kind {
        SettingKind::Bool => view! {
            <input type="checkbox" prop:checked=move || value() == true on:change=move |e| save(json!(event_target_checked(&e)))/>
        }.into_any(),
        SettingKind::Number { min, max } => view! {
            <input type="number" step="any" min=min max=max style="width:120px" prop:value=move || value().as_f64().map(|n| n.to_string()).unwrap_or_default()
                on:change=move |e| {
                    match event_target_value(&e).parse::<f64>() {
                        Ok(n) if (min..=max).contains(&n) => save(json!(n)),
                        _ => toast_bad(format!("{}: a number from {min} to {max}.", def.label)),
                    }
                }/>
        }.into_any(),
        SettingKind::Text => view! {
            <input class="grow" prop:value=move || value().as_str().unwrap_or("").to_string() on:change=move |e| save(json!(event_target_value(&e)))/>
        }.into_any(),
        SettingKind::Choice { options } => view! {
            <select on:change=move |e| save(json!(event_target_value(&e)))>
                {options.iter().map(|o| view! { <option value=*o selected=move || value().as_str() == Some(*o)>{*o}</option> }).collect_view()}
            </select>
        }.into_any(),
    };
    view! {
        <div class="set-row">
            <div class="set-label">{def.label}{(!def.help.is_empty()).then(|| view! { <div class="small subdue">{def.help}</div> })}</div>
            <div class="set-ctl row">{ctl}</div>
        </div>
    }
}
