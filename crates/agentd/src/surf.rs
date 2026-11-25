//! Surf: each project's own browser. Steel (Chromium with a live viewer) in a Docker container
//! `agentum-surf-<id>`, `<id>` a short hash of the project's path, with a profile volume of its
//! own (sign-ins survive a stop).
//!
//! Safety: both of its ports listen on this machine's 127.0.0.1 only. Agents on this machine
//! drive it over CDP (`AGENTUM_SURF_CDP`); the person watches it through the hub, which reaches
//! the viewer through this daemon's own connection ([`Surfs::get`], [`Surfs::open`]) and only
//! its two viewer paths. No port is opened to the network.
//!
//! A container that exists is the project's browser being on: stopped after `idle_hours` unused,
//! it starts again with the project's next agent. Stop (the person's) removes the container.
use crate::db::Db;
use agentum_proto::machine::{FromAgentd, SockMsg, SurfInfo};
use anyhow::{Context, bail};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// The Steel build BB Surf runs (same digest, so a machine with BB already has it).
pub const IMAGE: &str = "ghcr.io/steel-dev/steel-browser@sha256:f5cd68fbc2cb27e5d7766269860fd0fb29cbe5fe506245a49c82f85ba210e7da";
const PREFIX: &str = "agentum-surf-";
const LABEL: &str = "agentum.surf";
/// Live viewer sockets at once (each is a tab's screencast).
const MAX_SOCKS: usize = 24;

pub struct Surfs {
    image: String,
    db: Db,
    out: mpsc::Sender<FromAgentd>,
    idle_hours: Mutex<f64>,
    used: Mutex<HashMap<String, Instant>>,
    socks: Mutex<HashMap<String, mpsc::UnboundedSender<Message>>>,
    /// One create/start/stop at a time.
    lock: tokio::sync::Mutex<()>,
}

/// The id of a project's browser: 12 hex of the sha256 of its canonical path.
pub fn id_of(project: &Path) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(project.to_string_lossy().as_bytes()))[..12].to_string()
}

fn valid_id(id: &str) -> bool {
    id.len() == 12 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Only the viewer's page and its screencast socket are reachable from the hub.
fn viewer_path(path: &str, want: &str) -> bool {
    path.starts_with(want) && matches!(path.as_bytes().get(want.len()), None | Some(b'?')) && path.bytes().all(|b| b.is_ascii_graphic())
}

async fn docker(args: &[&str], wait: Duration) -> anyhow::Result<String> {
    let mut cmd = tokio::process::Command::new("docker");
    cmd.args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
    let out = match tokio::time::timeout(wait, cmd.output()).await {
        Err(_) => bail!("docker {} took longer than {}s", args.first().unwrap_or(&""), wait.as_secs()),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("Docker isn't installed on this machine (the project's browser runs in a Docker container)")
        }
        Ok(r) => r.context("running docker")?,
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        if err.contains("Cannot connect to the Docker daemon") || err.contains("permission denied while trying to connect") {
            bail!("Docker isn't running here, or this user may not use it: {err}");
        }
        bail!("docker {}: {}", args.first().unwrap_or(&""), crate::util::truncate(err, 400));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn port_free(p: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", p)).is_ok()
}

/// The repo a directory belongs to (a worktree's main repo), canonical.
pub async fn project_of(cwd: &Path) -> Option<PathBuf> {
    let out = tokio::process::Command::new("git").arg("-C").arg(cwd).args(["rev-parse", "--path-format=absolute", "--git-common-dir"]).output().await.ok()?;
    if !out.status.success() {
        return cwd.canonicalize().ok();
    }
    let common = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let root = if common.file_name().is_some_and(|n| n == ".git") { common.parent()?.to_path_buf() } else { common };
    root.canonicalize().ok()
}

impl Surfs {
    pub async fn new(db: Db, out: mpsc::Sender<FromAgentd>, image: Option<String>) -> Surfs {
        let idle = db.kv_get("surf.idle_hours").await.and_then(|s| s.parse().ok()).unwrap_or(4.0);
        Surfs {
            image: image.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| IMAGE.to_string()),
            db,
            out,
            idle_hours: Mutex::new(idle),
            used: Mutex::new(HashMap::new()),
            socks: Mutex::new(HashMap::new()),
            lock: tokio::sync::Mutex::new(()),
        }
    }

    fn touch(&self, id: &str) {
        self.used.lock().unwrap().insert(id.to_string(), Instant::now());
    }

    /// Every agentum-surf container on this machine.
    pub async fn list(&self) -> anyhow::Result<Vec<SurfInfo>> {
        let fmt = format!("{{{{.Names}}}}\t{{{{.State}}}}\t{{{{.Status}}}}\t{{{{.Label \"{LABEL}.project\"}}}}\t{{{{.Label \"{LABEL}.api\"}}}}\t{{{{.Label \"{LABEL}.cdp\"}}}}");
        let filter = format!("label={LABEL}");
        let text = docker(&["ps", "-a", "--filter", &filter, "--format", &fmt], Duration::from_secs(20)).await?;
        let used = self.used.lock().unwrap().clone();
        let now = crate::util::now_ms();
        Ok(text
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                let id = f.first()?.strip_prefix(PREFIX)?.to_string();
                let (api, cdp): (u16, u16) = (f.get(4)?.parse().ok()?, f.get(5)?.parse().ok()?);
                let state = match *f.get(1)? {
                    "running" => "running",
                    "created" | "restarting" => "starting",
                    _ => "stopped",
                };
                let last_used = used.get(&id).map(|t| now - t.elapsed().as_millis() as i64);
                Some(SurfInfo { project: f.get(3)?.to_string(), state: state.into(), status: f.get(2).unwrap_or(&"").to_string(), api_port: api, cdp_port: cdp, cdp_url: format!("ws://127.0.0.1:{api}/"), last_used, id })
            })
            .collect())
    }

    async fn find(&self, id: &str) -> anyhow::Result<Option<SurfInfo>> {
        Ok(self.list().await?.into_iter().find(|s| s.id == id))
    }

    pub fn set_idle_hours(&self, h: f64) {
        let h = h.clamp(0.0, 24.0 * 30.0);
        *self.idle_hours.lock().unwrap() = h;
        self.db.kv_set("surf.idle_hours", h.to_string());
    }

    /// Start (or wake, or reuse) a project's browser; answers once Chromium does.
    pub async fn ensure(&self, project: &str, allowed: &crate::fs::Allowed) -> anyhow::Result<SurfInfo> {
        let p = allowed.check(project)?;
        anyhow::ensure!(p.is_dir(), "{project}: not a directory");
        let p = p.canonicalize()?;
        let id = id_of(&p);
        let info = {
            let _g = self.lock.lock().await;
            match self.find(&id).await? {
                Some(s) if s.state == "running" => s,
                Some(s) => match docker(&["start", &format!("{PREFIX}{id}")], Duration::from_secs(60)).await {
                    Ok(_) => s,
                    Err(e) => {
                        // Its ports may have been taken meanwhile: a new container, same profile.
                        tracing::info!("surf {id}: start failed ({e:#}); recreating it");
                        self.remove(&id).await?;
                        self.create(&id, &p).await?
                    }
                },
                None => self.create(&id, &p).await?,
            }
        };
        self.touch(&id);
        if let Err(e) = ready(info.api_port, info.cdp_port, Duration::from_secs(60)).await {
            bail!("the browser started but doesn't answer yet ({e:#}); try again in a moment");
        }
        self.touch(&id);
        Ok(SurfInfo { state: "running".into(), ..self.find(&id).await?.unwrap_or(info) })
    }

    async fn create(&self, id: &str, project: &Path) -> anyhow::Result<SurfInfo> {
        let name = format!("{PREFIX}{id}");
        let volume = format!("{name}-profile");
        // A profile left by a container that was killed keeps Chromium's lock files: clear them.
        if docker(&["volume", "inspect", &volume], Duration::from_secs(10)).await.is_ok() {
            let v = format!("{volume}:/profiles");
            let _ = docker(&["run", "--rm", "--entrypoint", "sh", "-v", &v, &self.image, "-c", "rm -f /profiles/chrome/Singleton*"], Duration::from_secs(120)).await;
        }
        let start = 42000 + (u32::from_str_radix(&id[..6], 16).unwrap_or(0) % 1500) as u16 * 2;
        let mut tries = 0;
        for k in 0..400u16 {
            let api = 42000 + (start - 42000 + k * 2) % 3000;
            let cdp = api + 1;
            if !port_free(api) || !port_free(cdp) {
                continue;
            }
            let args: Vec<String> = vec![
                "run".into(), "-d".into(), "--init".into(), "--restart".into(), "unless-stopped".into(), "--shm-size".into(), "1g".into(),
                "--hostname".into(), "agentum-surf".into(), "--name".into(), name.clone(),
                "--label".into(), format!("{LABEL}=1"),
                "--label".into(), format!("{LABEL}.project={}", project.to_string_lossy()),
                "--label".into(), format!("{LABEL}.api={api}"),
                "--label".into(), format!("{LABEL}.cdp={cdp}"),
                // The project's dev servers on this machine: http://host.docker.internal:<port>.
                "--add-host".into(), "host.docker.internal:host-gateway".into(),
                "-p".into(), format!("127.0.0.1:{api}:3000"),
                "-p".into(), format!("127.0.0.1:{cdp}:9223"),
                "-e".into(), format!("DOMAIN=127.0.0.1:{api}"),
                "-e".into(), "USE_SSL=false".into(),
                "-e".into(), "CHROME_USER_DATA_DIR=/profiles/chrome".into(),
                "-v".into(), format!("{volume}:/profiles"),
                self.image.clone(),
            ];
            let a: Vec<&str> = args.iter().map(String::as_str).collect();
            match docker(&a, Duration::from_secs(900)).await {
                Ok(_) => {
                    tracing::info!("surf {id}: started for {} on 127.0.0.1:{api} (CDP {cdp})", project.display());
                    return Ok(SurfInfo { id: id.into(), project: project.to_string_lossy().into(), state: "starting".into(), status: String::new(), api_port: api, cdp_port: cdp, cdp_url: format!("ws://127.0.0.1:{api}/"), last_used: None });
                }
                Err(e) if e.to_string().contains("already allocated") || e.to_string().contains("address already in use") => {
                    let _ = docker(&["rm", "-f", &name], Duration::from_secs(30)).await;
                    tries += 1;
                    if tries >= 5 {
                        return Err(e);
                    }
                }
                Err(e) => {
                    let _ = docker(&["rm", "-f", &name], Duration::from_secs(30)).await;
                    return Err(e);
                }
            }
        }
        bail!("no free ports for the browser on this machine")
    }

    async fn remove(&self, id: &str) -> anyhow::Result<()> {
        let name = format!("{PREFIX}{id}");
        // Gracefully first, so Chromium leaves its profile tidy.
        let _ = docker(&["stop", "-t", "10", &name], Duration::from_secs(30)).await;
        docker(&["rm", "-f", &name], Duration::from_secs(30)).await.map(|_| ())
    }

    /// The person's Stop: the container goes, the profile stays.
    pub async fn stop(&self, project: &str) -> anyhow::Result<Value> {
        let p = PathBuf::from(project);
        let id = id_of(&p.canonicalize().unwrap_or(p));
        let _g = self.lock.lock().await;
        if self.find(&id).await?.is_none() {
            return Ok(json!({"stopped": false}));
        }
        self.close_socks_of(&id);
        self.remove(&id).await?;
        self.used.lock().unwrap().remove(&id);
        tracing::info!("surf {id}: stopped for {project}");
        Ok(json!({"stopped": true}))
    }

    async fn running(&self, id: &str) -> anyhow::Result<SurfInfo> {
        anyhow::ensure!(valid_id(id), "no such browser");
        match self.find(id).await? {
            Some(s) if s.state == "running" => Ok(s),
            Some(_) => bail!("the browser is stopped"),
            None => bail!("no such browser"),
        }
    }

    /// The viewer's page, for the hub to serve.
    pub async fn get(&self, id: &str, path: &str) -> anyhow::Result<Value> {
        anyhow::ensure!(viewer_path(path, "/v1/sessions/debug"), "not a viewer page");
        let s = self.running(id).await?;
        self.touch(id);
        let addr = format!("127.0.0.1:{}", s.api_port);
        let r = crate::http::request(&addr, "GET", path, "", None).await?;
        Ok(json!({"status": r.status, "body": r.text(), "origin": addr}))
    }

    /// A WebSocket to the viewer, relayed to the hub as `sock`.
    pub async fn open(self: &Arc<Self>, id: &str, sock: &str, path: &str) -> anyhow::Result<Value> {
        anyhow::ensure!(viewer_path(path, "/v1/sessions/cast"), "not the viewer's socket");
        anyhow::ensure!(!sock.is_empty() && sock.len() <= 64, "bad socket id");
        anyhow::ensure!(self.socks.lock().unwrap().len() < MAX_SOCKS, "too many viewers open");
        let s = self.running(id).await?;
        let url = format!("ws://127.0.0.1:{}{path}", s.api_port);
        let (ws, _) = tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(url)).await.context("the viewer didn't answer")??;
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
        self.socks.lock().unwrap().insert(format!("{id}/{sock}"), tx);
        self.touch(id);
        tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                let close = matches!(m, Message::Close(_));
                if sink.send(m).await.is_err() || close {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        let (out, sock2, id2) = (self.out.clone(), sock.to_string(), id.to_string());
        let me = self.clone();
        tokio::spawn(async move {
            while let Some(Ok(m)) = stream.next().await {
                let msg = match m {
                    Message::Text(t) => SockMsg { sock: sock2.clone(), text: Some(t.to_string()), data: None },
                    Message::Binary(b) => {
                        use base64::Engine;
                        SockMsg { sock: sock2.clone(), text: None, data: Some(base64::engine::general_purpose::STANDARD.encode(&b)) }
                    }
                    Message::Close(_) => break,
                    _ => continue,
                };
                me.touch(&id2);
                if out.send(FromAgentd::Sock(msg)).await.is_err() {
                    break;
                }
            }
            if me.socks.lock().unwrap().remove(&format!("{id2}/{sock2}")).is_some() {
                let _ = out.send(FromAgentd::SockClose { sock: sock2 }).await;
            }
        });
        Ok(json!({"sock": sock}))
    }

    fn sock_key(&self, sock: &str) -> Option<String> {
        self.socks.lock().unwrap().keys().find(|k| k.split_once('/').map(|(_, s)| s) == Some(sock)).cloned()
    }

    /// From the hub: a message for one of the viewer's sockets.
    pub fn sock_send(&self, m: SockMsg) {
        let Some(key) = self.sock_key(&m.sock) else { return };
        let msg = match (m.text, m.data) {
            (Some(t), _) => Message::text(t),
            (None, Some(d)) => {
                use base64::Engine;
                match base64::engine::general_purpose::STANDARD.decode(d) {
                    Ok(b) => Message::binary(b),
                    Err(_) => return,
                }
            }
            _ => return,
        };
        if let Some((id, _)) = key.split_once('/') {
            self.touch(id);
        }
        if let Some(tx) = self.socks.lock().unwrap().get(&key) {
            let _ = tx.send(msg);
        }
    }

    pub fn sock_close(&self, sock: &str) {
        if let Some(key) = self.sock_key(sock)
            && let Some(tx) = self.socks.lock().unwrap().remove(&key)
        {
            let _ = tx.send(Message::Close(None));
        }
    }

    fn close_socks_of(&self, id: &str) {
        self.socks.lock().unwrap().retain(|k, tx| {
            let mine = k.starts_with(&format!("{id}/"));
            if mine {
                let _ = tx.send(Message::Close(None));
            }
            !mine
        });
    }

    /// The hub went away: its viewers are gone.
    pub fn close_all_socks(&self) {
        for (_, tx) in self.socks.lock().unwrap().drain() {
            let _ = tx.send(Message::Close(None));
        }
    }

    /// The environment of an agent starting in `cwd`: the project's browser, woken if it slept.
    pub async fn env_for(&self, cwd: &Path) -> Vec<(String, String)> {
        let Some(project) = project_of(cwd).await else { return vec![] };
        let id = id_of(&project);
        let s = match self.find(&id).await {
            Ok(Some(s)) => s,
            _ => return vec![],
        };
        if s.state != "running" {
            let woke = {
                let _g = self.lock.lock().await;
                docker(&["start", &format!("{PREFIX}{id}")], Duration::from_secs(60)).await
            };
            if let Err(e) = woke {
                tracing::info!("surf {id}: couldn't wake it for an agent: {e:#}");
                return vec![];
            }
            tracing::info!("surf {id}: woken for an agent in {}", project.display());
            if let Err(e) = ready(s.api_port, s.cdp_port, Duration::from_secs(25)).await {
                tracing::info!("surf {id}: woke, but not ready: {e:#}");
            }
        }
        self.touch(&id);
        vec![("AGENTUM_SURF_CDP".into(), s.cdp_url.clone()), ("AGENTUM_SURF_DEVTOOLS".into(), format!("http://127.0.0.1:{}", s.cdp_port))]
    }

    /// Every minute: browsers unused for `idle_hours` stop (an agent running in the project, or
    /// an open viewer, is use).
    pub async fn idle_loop(self: Arc<Self>, live_cwds: impl Fn() -> Vec<String> + Send + 'static) {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let hours = *self.idle_hours.lock().unwrap();
            let Ok(list) = self.list().await else { continue };
            let running: Vec<SurfInfo> = list.into_iter().filter(|s| s.state == "running").collect();
            if running.is_empty() {
                continue;
            }
            for cwd in live_cwds() {
                if let Some(p) = project_of(Path::new(&cwd)).await {
                    self.touch(&id_of(&p));
                }
            }
            if hours <= 0.0 {
                continue;
            }
            for s in running {
                let last = *self.used.lock().unwrap().entry(s.id.clone()).or_insert_with(Instant::now);
                if last.elapsed() >= Duration::from_secs_f64(hours * 3600.0) {
                    let _g = self.lock.lock().await;
                    self.close_socks_of(&s.id);
                    match docker(&["stop", "-t", "10", &format!("{PREFIX}{}", s.id)], Duration::from_secs(30)).await {
                        Ok(_) => tracing::info!("surf {}: unused for {hours}h, stopped ({})", s.id, s.project),
                        Err(e) => tracing::warn!("surf {}: idle stop failed: {e:#}", s.id),
                    }
                }
            }
        }
    }
}

/// Steel's API and Chromium's DevTools both answer.
async fn ready(api: u16, cdp: u16, wait: Duration) -> anyhow::Result<()> {
    let t0 = Instant::now();
    let mut last = anyhow::anyhow!("not yet");
    while t0.elapsed() < wait {
        let h = crate::http::request(&format!("127.0.0.1:{api}"), "GET", "/v1/health", "", None).await;
        let c = crate::http::request(&format!("127.0.0.1:{cdp}"), "GET", "/json/version", "", None).await;
        match (h, c) {
            (Ok(h), Ok(c)) if h.ok() && c.ok() && h.json().ok().is_some_and(|v| v["status"] == "ok") => return Ok(()),
            (Err(e), _) | (_, Err(e)) => last = e,
            _ => last = anyhow::anyhow!("not ready"),
        }
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_paths() {
        let id = id_of(Path::new("/home/me/Developer/app"));
        assert!(valid_id(&id));
        assert_eq!(id, id_of(Path::new("/home/me/Developer/app")));
        assert_ne!(id, id_of(Path::new("/home/me/Developer/app2")));
        assert!(viewer_path("/v1/sessions/debug?interactive=true", "/v1/sessions/debug"));
        assert!(viewer_path("/v1/sessions/debug", "/v1/sessions/debug"));
        assert!(!viewer_path("/v1/sessions/debugger", "/v1/sessions/debug"));
        assert!(!viewer_path("/v1/sessions/debug?x=1 HTTP/1.1\r\n", "/v1/sessions/debug"));
        assert!(viewer_path("/v1/sessions/cast?pageId=AB12", "/v1/sessions/cast"));
        assert!(!viewer_path("/v1/sessions/release", "/v1/sessions/cast"));
    }
}
