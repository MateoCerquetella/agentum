//! agentd: Agentum's per-machine daemon.
//!
//! It keeps one WebSocket open to the hub ([`hub`]), runs coding agents for threads
//! ([`threads`], [`drivers`]), and serves files ([`fs`]), git ([`git`]) and terminals ([`term`]).
pub mod config;
pub mod db;
pub mod drivers;
pub mod fs;
pub mod git;
pub mod http;
pub mod hub;
pub mod install;
pub mod machine;
pub mod outbox;
pub mod term;
pub mod threads;
pub mod util;

use agentum_proto::machine::{AgentInfo, FromAgentd, Hello, Project, Req};
use config::Config;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Everything agentd runs on.
pub struct App {
    pub cfg: Config,
    pub db: db::Db,
    pub outbox: Arc<outbox::Outbox>,
    pub threads: Arc<threads::Threads>,
    pub terms: term::Terms,
    /// Frames for the hub other than thread events (replies, terminal output, updates).
    pub out: mpsc::Sender<FromAgentd>,
    out_rx: tokio::sync::Mutex<mpsc::Receiver<FromAgentd>>,
    agents: Mutex<Vec<AgentInfo>>,
    projects: Mutex<Vec<Project>>,
    extra: Mutex<Vec<PathBuf>>,
    connected: AtomicBool,
    since: Mutex<Option<i64>>,
    refresh: tokio::sync::Notify,
}

impl App {
    pub async fn new(cfg: Config) -> anyhow::Result<Arc<App>> {
        let data = cfg.data_dir();
        std::fs::create_dir_all(&data)?;
        let db = db::Db::open(&data.join("agentd.db"))?;
        let outbox = Arc::new(outbox::Outbox::new(db.clone()));
        let threads = threads::Threads::new(cfg.clone(), db.clone(), outbox.clone());
        let (out, out_rx) = mpsc::channel(1024);
        let extra: Vec<PathBuf> = match db.kv_get("projects").await {
            Some(j) => serde_json::from_str(&j).unwrap_or_default(),
            None => vec![],
        };
        let app = Arc::new(App {
            terms: term::Terms::new(out.clone()),
            cfg,
            db,
            outbox,
            threads,
            out,
            out_rx: tokio::sync::Mutex::new(out_rx),
            agents: Mutex::new(vec![]),
            projects: Mutex::new(vec![]),
            extra: Mutex::new(extra),
            connected: AtomicBool::new(false),
            since: Mutex::new(None),
            refresh: tokio::sync::Notify::new(),
        });
        app.threads.restore().await?;
        app.scan().await;
        Ok(app)
    }

    /// Run until the process is told to stop.
    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        tokio::spawn(self.clone().watch_machine());
        tokio::spawn(self.clone().write_status_loop());
        let hub = tokio::spawn(hub::run(self.clone()));
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
            _ = hub => {}
        }
        tracing::info!("stopping");
        self.terms.close_all();
        self.db.flush().await;
        Ok(())
    }

    fn explicit_projects(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.cfg.projects.iter().map(|p| util::expand(p)).collect();
        v.extend(self.extra.lock().unwrap().iter().cloned());
        v
    }

    /// Look at agents and projects again; true if something changed.
    pub async fn scan(&self) -> bool {
        let agents = machine::agents(&self.cfg).await;
        let (roots, extra, skip) = (self.cfg.roots(), self.explicit_projects(), vec![self.cfg.data_dir()]);
        let projects = tokio::task::spawn_blocking(move || machine::scan_projects(&roots, &extra, &skip)).await.unwrap_or_default();
        let mut changed = false;
        {
            let mut a = self.agents.lock().unwrap();
            if *a != agents {
                *a = agents;
                changed = true;
            }
        }
        {
            let mut p = self.projects.lock().unwrap();
            if *p != projects {
                *p = projects;
                changed = true;
            }
        }
        changed
    }

    pub fn hello(&self) -> Hello {
        Hello {
            protocol: agentum_proto::PROTOCOL,
            machine: self.cfg.name.clone(),
            version: VERSION.to_string(),
            hostname: machine::hostname(),
            os: machine::os(),
            arch: std::env::consts::ARCH.to_string(),
            home: machine::home_string(),
            agents: self.agents.lock().unwrap().clone(),
            projects: self.projects.lock().unwrap().clone(),
            threads: self.threads.live(),
            stats: machine::stats(),
        }
    }

    /// Re-check the machine every minute (and when asked); tell the hub when it changed.
    async fn watch_machine(self: Arc<Self>) {
        let mut last_stats = tokio::time::Instant::now();
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(60)) => {}
                _ = self.refresh.notified() => {}
            }
            let changed = self.scan().await;
            let stale = last_stats.elapsed() >= Duration::from_secs(300);
            if (changed || stale) && self.is_connected() {
                last_stats = tokio::time::Instant::now();
                let _ = self.out.send(FromAgentd::Update(self.hello())).await;
            }
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn set_connected(&self, c: bool) {
        self.connected.store(c, Ordering::Relaxed);
        *self.since.lock().unwrap() = c.then(util::now_ms);
        self.write_status();
    }

    pub fn status_path(cfg: &Config) -> PathBuf {
        cfg.data_dir().join("status.json")
    }

    fn write_status(&self) {
        let s = json!({
            "pid": std::process::id(),
            "version": VERSION,
            "hub": self.cfg.hub,
            "name": self.cfg.name,
            "connected": self.is_connected(),
            "since": *self.since.lock().unwrap(),
            "threads_live": self.threads.live_processes(),
            "terminals": self.terms.count(),
            "projects": self.projects.lock().unwrap().len(),
            "rss_mb": machine::rss_kb() / 1024,
            "at": util::now_ms(),
        });
        let p = Self::status_path(&self.cfg);
        let _ = std::fs::write(&p, serde_json::to_vec_pretty(&s).unwrap_or_default());
    }

    async fn write_status_loop(self: Arc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            self.write_status();
        }
    }

    /// Where file requests may go: home, the projects and the worktrees.
    pub fn allowed(&self) -> fs::Allowed {
        let mut roots = vec![util::home()];
        roots.extend(self.projects.lock().unwrap().iter().map(|p| PathBuf::from(&p.path)));
        roots.push(self.cfg.worktrees_dir());
        let roots = roots.into_iter().filter_map(|r| r.canonicalize().ok()).collect();
        fs::Allowed { roots }
    }

    /// Handle a request. Thread and terminal requests are queued here, in arrival order; the
    /// returned future is their answer.
    pub fn dispatch(self: &Arc<Self>, req: Req) -> futures::future::BoxFuture<'static, anyhow::Result<Value>> {
        use agentum_proto::machine::TermReq;
        use futures::FutureExt;
        let app = self.clone();
        match req {
            // Keystrokes must stay in order: handled right here.
            Req::Term(t @ (TermReq::Input { .. } | TermReq::Resize { .. } | TermReq::Close { .. })) => {
                let r = self.terms.handle(t);
                async move { r }.boxed()
            }
            Req::Term(t) => {
                let terms = self.terms.clone();
                async move { tokio::task::spawn_blocking(move || terms.handle(t)).await? }.boxed()
            }
            // A thread's requests keep their order: they reach its actor's queue right here.
            Req::Start { thread, spec, resume, input, last_seq } => wait(self.threads.start(thread, spec, resume, input, last_seq)),
            Req::Send { thread, input, mode } => wait(self.threads.send(&thread, input, mode)),
            Req::Stop { thread } => wait(self.threads.stop(&thread)),
            Req::Answer { thread, ask, option, text } => wait(self.threads.answer(&thread, ask, option, text)),
            Req::SetModel { thread, model, effort } => wait(self.threads.set_model(&thread, model, effort)),
            Req::Sleep { thread } => wait(self.threads.sleep(&thread)),
            Req::Models { agent } => async move { app.threads.models(agent).await }.boxed(),
            Req::AddProject { path } => async move { app.add_project(path).await }.boxed(),
            Req::Fs(f) => {
                let allowed = self.allowed();
                async move { tokio::task::spawn_blocking(move || fs::handle(f, &allowed)).await? }.boxed()
            }
            Req::Git(g) => {
                let wt = self.cfg.worktrees_dir();
                let allowed = self.allowed();
                async move {
                    check_git_paths(&g, &allowed)?;
                    let r = git::handle(g, &wt).await;
                    if r.is_ok() {
                        // A worktree may be a new project root.
                        app.refresh.notify_one();
                    }
                    r
                }
                .boxed()
            }
        }
    }

    async fn add_project(self: &Arc<Self>, path: String) -> anyhow::Result<Value> {
        let p = util::expand(&path);
        let p = p.canonicalize().map_err(|e| anyhow::anyhow!("{path}: {e}"))?;
        if !p.is_dir() {
            anyhow::bail!("{path}: not a directory");
        }
        {
            let mut extra = self.extra.lock().unwrap();
            if !extra.contains(&p) {
                extra.push(p.clone());
            }
            self.db.kv_set("projects", serde_json::to_string(&*extra).unwrap_or_default());
        }
        self.scan().await;
        if self.is_connected() {
            let _ = self.out.send(FromAgentd::Update(self.hello())).await;
        }
        let proj = self.projects.lock().unwrap().iter().find(|x| PathBuf::from(&x.path) == p).cloned();
        Ok(json!(proj.unwrap_or(Project { path: p.to_string_lossy().into(), name: p.file_name().map(|n| n.to_string_lossy().into()).unwrap_or_default(), branch: machine::branch(&p) })))
    }

    pub(crate) async fn take_out_rx(&self) -> tokio::sync::MutexGuard<'_, mpsc::Receiver<FromAgentd>> {
        self.out_rx.lock().await
    }
}

fn wait(a: anyhow::Result<threads::Answer>) -> futures::future::BoxFuture<'static, anyhow::Result<Value>> {
    use futures::FutureExt;
    async move { threads::answer(a?).await }.boxed()
}

/// Git requests may only touch allowed directories too.
fn check_git_paths(g: &agentum_proto::machine::GitReq, allowed: &fs::Allowed) -> anyhow::Result<()> {
    use agentum_proto::machine::GitReq::*;
    let p = match g {
        Status { cwd } | Diff { cwd, .. } | Commit { cwd, .. } | Push { cwd } | Log { cwd, .. } | Show { cwd, .. } | Branches { cwd } | Pr { cwd } | PrMerge { cwd, .. } => cwd,
        WorktreeAdd { repo, .. } => repo,
        WorktreeRemove { path } => path,
    };
    allowed.check(p).map(|_| ())
}
