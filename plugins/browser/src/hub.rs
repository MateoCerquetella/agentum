//! Browser's hub part: each project's browser (Surf) on its machine, through agentd.
//!
//! - `status {machine, project}` → `{surf}` (null: none); `list {machine}` → `{surfs}`.
//! - `start {machine, project}` → `{surf}` once it answers; `stop {machine, project}`.
//!
//! It remembers which projects have a browser, so a starting agent there is told how to use it
//! ([`HubPlugin::instructions`]); with "start automatically", a new thread starts its project's.
use agentum_plugin::hub::{Ctx, HubPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::ThreadSummary;
use agentum_proto::machine::{Req, SurfInfo, SurfReq};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

/// (machine, project) → its browser's agent endpoint, as last seen.
static KNOWN: Mutex<Option<HashMap<(String, String), String>>> = Mutex::new(None);
/// When each machine's browsers were last listed.
static LISTED: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

fn remember(machine: &str, surfs: &[SurfInfo]) {
    let mut k = KNOWN.lock().unwrap();
    let k = k.get_or_insert_with(HashMap::new);
    k.retain(|(m, _), _| m != machine);
    for s in surfs {
        k.insert((machine.to_string(), s.project.clone()), s.cdp_url.clone());
    }
    LISTED.lock().unwrap().get_or_insert_with(HashMap::new).insert(machine.to_string(), Instant::now());
}

fn known(machine: &str, project: &str) -> Option<String> {
    KNOWN.lock().unwrap().as_ref()?.get(&(machine.to_string(), project.to_string())).cloned()
}

async fn list(cx: &Ctx, machine: &str) -> Result<Vec<SurfInfo>, String> {
    let r = cx.machine(machine, Req::Surf(SurfReq::List)).await;
    let surfs: Vec<SurfInfo> = match r {
        Ok(v) => serde_json::from_value(v).map_err(|e| e.to_string())?,
        Err(e) => {
            // No Docker there (or offline): nothing to remember, and don't ask again at once.
            remember(machine, &[]);
            return Err(e);
        }
    };
    remember(machine, &surfs);
    Ok(surfs)
}

fn arg<'a>(input: &'a Value, k: &str) -> Result<&'a str, String> {
    input[k].as_str().filter(|s| !s.is_empty()).ok_or_else(|| format!("{k} is missing"))
}

async fn start(cx: &Ctx, machine: &str, project: &str) -> Result<SurfInfo, String> {
    let idle = cx.setting("idle_hours").await.as_f64();
    let v = cx.machine(machine, Req::Surf(SurfReq::Ensure { project: project.to_string(), idle_hours: idle })).await?;
    let s: SurfInfo = serde_json::from_value(v).map_err(|e| e.to_string())?;
    KNOWN.lock().unwrap().get_or_insert_with(HashMap::new).insert((machine.to_string(), s.project.clone()), s.cdp_url.clone());
    cx.publish(json!({"surf": machine}));
    Ok(s)
}

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    async fn rpc(&self, cx: &Ctx, method: &str, input: Value) -> Result<Value, String> {
        let machine = arg(&input, "machine")?;
        match method {
            "list" => Ok(json!({ "surfs": list(cx, machine).await? })),
            "status" => {
                let project = arg(&input, "project")?;
                let surfs = list(cx, machine).await?;
                Ok(json!({ "surf": surfs.into_iter().find(|s| s.project == project) }))
            }
            "start" => Ok(json!({ "surf": start(cx, machine, arg(&input, "project")?).await? })),
            "stop" => {
                let project = arg(&input, "project")?;
                cx.machine(machine, Req::Surf(SurfReq::Stop { project: project.to_string() })).await?;
                if let Some(k) = KNOWN.lock().unwrap().as_mut() {
                    k.remove(&(machine.to_string(), project.to_string()));
                }
                cx.publish(json!({"surf": machine}));
                Ok(json!({ "ok": true }))
            }
            _ => Err(format!("no method {method}")),
        }
    }

    /// Know whether the thread's project has a browser before its agent starts; with "start
    /// automatically", start it for a new thread. Never refuses a turn.
    async fn before_turn(&self, cx: &Ctx, t: &ThreadSummary, _text: &str) -> Result<(), String> {
        let stale = LISTED.lock().unwrap().as_ref().and_then(|l| l.get(&t.machine)).is_none_or(|at| at.elapsed() > Duration::from_secs(60));
        if stale {
            let _ = list(cx, &t.machine).await;
        }
        let new = t.last_seq == 0 && t.parent.is_none();
        if new && known(&t.machine, &t.project).is_none() && cx.setting("auto_start").await == json!(true) {
            // Quick when the image is there; a first download goes on in the background.
            match tokio::time::timeout(Duration::from_secs(25), start(cx, &t.machine, &t.project)).await {
                Ok(Err(e)) => tracing::info!("browser: couldn't start the browser of {}: {e}", t.project),
                Err(_) => {
                    let (cx, m, p) = (cx.clone(), t.machine.clone(), t.project.clone());
                    tokio::spawn(async move {
                        let _ = start(&cx, &m, &p).await;
                    });
                }
                Ok(Ok(_)) => {}
            }
        }
        Ok(())
    }

    fn instructions(&self, _cx: &Ctx, t: &ThreadSummary) -> Option<String> {
        let url = known(&t.machine, &t.project)?;
        Some(INSTRUCTIONS.replace("{url}", &url))
    }
}

const INSTRUCTIONS: &str = "\
# This project's browser (Agentum Surf)
The project has its own Chromium, shared with the person: they watch it live in this thread's Browser tab. Use it whenever a task needs a browser (open pages, test the app, take screenshots, sign in).
- Connect over CDP to $AGENTUM_SURF_CDP (now {url}). Playwright: `const browser = await chromium.connectOverCDP(process.env.AGENTUM_SURF_CDP); const ctx = browser.contexts()[0]; const page = ctx.pages()[0] ?? await ctx.newPage();` (Python: `p.chromium.connect_over_cdp(os.environ[\"AGENTUM_SURF_CDP\"])`). Use the project's Playwright if it has one, else a scratch script outside the repo (`npm i playwright-core` in /tmp). Don't launch another browser.
- Work in the open tab so the person sees it. Screenshot: `await page.screenshot({ path: \"/tmp/surf.png\" })`, then read the image.
- It runs in a container: this machine's dev servers are at http://host.docker.internal:<port> (listening on 0.0.0.0), not localhost.
- Never ask for passwords, codes or keys in the chat: ask the person to type them in the Browser tab and to tell you when they're done. The profile keeps the sign-in.
- If $AGENTUM_SURF_CDP is empty or doesn't answer, the browser is stopped: ask the person to start it in the Browser tab.";
