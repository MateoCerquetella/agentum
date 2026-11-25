//! Threads: one actor task per thread owns its agent session, its queue and its state, so the
//! requests of one thread are handled in order and its events are numbered without gaps.
use crate::config::Config;
use crate::db::{Db, ThreadRow};
use crate::drivers::{Agents, DriverOut, Session, StartOpts};
use crate::machine::agent_bin;
use crate::outbox::Outbox;
use agentum_proto::machine::{LiveThread, ModelInfo};
use agentum_proto::{AgentKind, EventKind, Input, SendMode, StopReason, ThreadSpec, ThreadState};
use anyhow::{Context, bail};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

type Reply = oneshot::Sender<anyhow::Result<Value>>;
/// The answer to a thread request, once its actor gets to it.
pub type Answer = oneshot::Receiver<anyhow::Result<Value>>;

/// Wait for an actor's answer.
pub async fn answer(rx: Answer) -> anyhow::Result<Value> {
    rx.await.map_err(|_| anyhow::anyhow!("the thread is gone"))?
}

enum Cmd {
    Start { spec: ThreadSpec, resume: Option<String>, input: Option<Input>, reply: Reply },
    Send { input: Input, mode: SendMode, reply: Reply },
    Stop { reply: Reply },
    Answer { ask: String, option: String, text: Option<String>, reply: Reply },
    SetModel { model: String, effort: Option<String>, reply: Reply },
    Sleep { reply: Reply },
}

/// What the rest of agentd can see of a thread without asking its actor.
#[derive(Clone, Debug)]
struct Snapshot {
    state: ThreadState,
    live: bool,
    cwd: String,
}

struct Handle {
    tx: mpsc::UnboundedSender<Cmd>,
    snap: Arc<Mutex<Snapshot>>,
}

pub struct Threads {
    cfg: Config,
    db: Db,
    outbox: Arc<Outbox>,
    pub agents: Arc<Agents>,
    surf: Arc<crate::surf::Surfs>,
    idle: Duration,
    handles: Mutex<HashMap<String, Handle>>,
    /// Threads known from the database that have no actor yet.
    rows: Mutex<HashMap<String, ThreadRow>>,
    models: tokio::sync::Mutex<HashMap<AgentKind, (Instant, Vec<ModelInfo>)>>,
}

impl Threads {
    pub fn new(cfg: Config, db: Db, outbox: Arc<Outbox>, surf: Arc<crate::surf::Surfs>) -> Arc<Threads> {
        let idle = Duration::from_secs(cfg.idle_minutes.max(1) * 60);
        let agents = Arc::new(Agents::new(cfg.idle_minutes));
        Arc::new(Threads {
            cfg,
            db,
            outbox,
            agents,
            surf,
            idle,
            handles: Mutex::new(HashMap::new()),
            rows: Mutex::new(HashMap::new()),
            models: tokio::sync::Mutex::new(HashMap::new()),
        })
    }

    /// At startup: restore every thread's unacked events; threads that were mid-turn are interrupted.
    pub async fn restore(self: &Arc<Self>) -> anyhow::Result<()> {
        for (row, events) in self.db.load_threads().await? {
            self.outbox.restore(&row.id, row.last_seq, row.acked, events);
            let was_busy = row.state.busy();
            let id = row.id.clone();
            self.rows.lock().unwrap().insert(id.clone(), row);
            if was_busy {
                // agentd restarted under a running turn: it is cut short; the next send resumes.
                let h = self.handle(&id).expect("row just inserted");
                drop(h);
            }
        }
        Ok(())
    }

    /// The actor of `id`, spawning it from its row if needed.
    fn handle(self: &Arc<Self>, id: &str) -> Option<mpsc::UnboundedSender<Cmd>> {
        let mut handles = self.handles.lock().unwrap();
        if let Some(h) = handles.get(id) {
            return Some(h.tx.clone());
        }
        let row = self.rows.lock().unwrap().remove(id)?;
        Some(self.spawn_actor(&mut handles, row))
    }

    fn spawn_actor(self: &Arc<Self>, handles: &mut HashMap<String, Handle>, row: ThreadRow) -> mpsc::UnboundedSender<Cmd> {
        let (tx, rx) = mpsc::unbounded_channel();
        let snap = Arc::new(Mutex::new(Snapshot { state: row.state, live: false, cwd: row.spec.cwd.clone() }));
        handles.insert(row.id.clone(), Handle { tx: tx.clone(), snap: snap.clone() });
        let actor = Actor::new(self.clone(), row, snap);
        tokio::spawn(actor.run(rx));
        tx
    }

    fn check_spec(&self, spec: &ThreadSpec) -> anyhow::Result<(AgentKind, PathBuf)> {
        let agent = spec.agent.unwrap_or(AgentKind::ClaudeCode);
        let bin = agent_bin(&self.cfg, agent).with_context(|| format!("{} is not installed on this machine", agent.label()))?;
        if !PathBuf::from(&spec.cwd).is_dir() {
            bail!("{}: no such directory", spec.cwd);
        }
        Ok((agent, bin))
    }

    /// Queue a Start (synchronously, so requests keep their order).
    pub fn start(self: &Arc<Self>, id: String, spec: ThreadSpec, resume: Option<String>, input: Option<Input>, last_seq: u64) -> anyhow::Result<Answer> {
        let (agent, _) = self.check_spec(&spec)?;
        if id.is_empty() {
            bail!("no thread id");
        }
        self.outbox.ensure(&id, last_seq);
        let tx = match self.handle(&id) {
            Some(tx) => tx,
            None => {
                let row = ThreadRow {
                    id: id.clone(),
                    agent,
                    spec: ThreadSpec { agent: Some(agent), ..spec.clone() },
                    session_id: resume.clone(),
                    state: ThreadState::Idle,
                    last_seq: self.outbox.last_seq(&id),
                    acked: self.outbox.acked(&id),
                    turn: 0,
                };
                self.db.save_thread(row.clone());
                let mut handles = self.handles.lock().unwrap();
                match handles.get(&id) {
                    Some(h) => h.tx.clone(),
                    None => self.spawn_actor(&mut handles, row),
                }
            }
        };
        let (reply, rx) = oneshot::channel();
        tx.send(Cmd::Start { spec: ThreadSpec { agent: Some(agent), ..spec }, resume, input, reply }).map_err(|_| anyhow::anyhow!("thread {id} is gone"))?;
        Ok(rx)
    }

    fn submit(self: &Arc<Self>, id: &str, make: impl FnOnce(Reply) -> Cmd) -> anyhow::Result<Answer> {
        let tx = self.handle(id).with_context(|| format!("no thread {id} on this machine"))?;
        let (reply, rx) = oneshot::channel();
        tx.send(make(reply)).map_err(|_| anyhow::anyhow!("thread {id} is gone"))?;
        Ok(rx)
    }

    pub fn send(self: &Arc<Self>, id: &str, input: Input, mode: SendMode) -> anyhow::Result<Answer> {
        self.submit(id, |reply| Cmd::Send { input, mode, reply })
    }
    pub fn stop(self: &Arc<Self>, id: &str) -> anyhow::Result<Answer> {
        self.submit(id, |reply| Cmd::Stop { reply })
    }
    pub fn answer(self: &Arc<Self>, id: &str, ask: String, option: String, text: Option<String>) -> anyhow::Result<Answer> {
        self.submit(id, |reply| Cmd::Answer { ask, option, text, reply })
    }
    pub fn set_model(self: &Arc<Self>, id: &str, model: String, effort: Option<String>) -> anyhow::Result<Answer> {
        self.submit(id, |reply| Cmd::SetModel { model, effort, reply })
    }
    pub fn sleep(self: &Arc<Self>, id: &str) -> anyhow::Result<Answer> {
        self.submit(id, |reply| Cmd::Sleep { reply })
    }

    /// The agent's models (cached for 10 minutes).
    pub async fn models(&self, agent: AgentKind) -> anyhow::Result<Value> {
        let mut cache = self.models.lock().await;
        if let Some((at, m)) = cache.get(&agent)
            && at.elapsed() < Duration::from_secs(600)
        {
            return Ok(json!(m));
        }
        let bin = agent_bin(&self.cfg, agent).with_context(|| format!("{} is not installed on this machine", agent.label()))?;
        let m = self.agents.get(agent).models(bin, crate::util::home()).await?;
        cache.insert(agent, (Instant::now(), m.clone()));
        Ok(json!(m))
    }

    /// Threads with a live process or unacked events (for Hello).
    pub fn live(&self) -> Vec<LiveThread> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for (id, h) in self.handles.lock().unwrap().iter() {
            let s = h.snap.lock().unwrap().clone();
            if s.live || self.outbox.unacked(id) > 0 || s.state.busy() {
                seen.insert(id.clone());
                out.push(LiveThread { thread: id.clone(), state: s.state, last_seq: self.outbox.last_seq(id) });
            }
        }
        let rows = self.rows.lock().unwrap();
        for id in self.outbox.pending_threads() {
            if seen.contains(&id) {
                continue;
            }
            let state = rows.get(&id).map(|r| r.state).unwrap_or_default();
            out.push(LiveThread { last_seq: self.outbox.last_seq(&id), thread: id, state });
        }
        out.sort_by(|a, b| a.thread.cmp(&b.thread));
        out
    }

    /// The directories of the agents running now.
    pub fn live_cwds(&self) -> Vec<String> {
        self.handles.lock().unwrap().values().map(|h| h.snap.lock().unwrap().clone()).filter(|s| s.live).map(|s| s.cwd).collect()
    }

    pub fn live_processes(&self) -> usize {
        self.handles.lock().unwrap().values().filter(|h| h.snap.lock().unwrap().live).count()
    }
}

struct Actor {
    threads: Arc<Threads>,
    row: ThreadRow,
    snap: Arc<Mutex<Snapshot>>,
    session: Option<Box<dyn Session>>,
    drv: Option<mpsc::UnboundedReceiver<DriverOut>>,
    queue: VecDeque<(Input, u64)>,
    asks: HashSet<String>,
    last_active: Instant,
    /// A Stop is waiting for the turn to end (with a deadline to force it).
    stopping: Option<Instant>,
    sleep_after: bool,
    /// The session must restart for a setting to apply (effort).
    restart: bool,
    /// We are ending the process ourselves: its exit is not news.
    closing: bool,
}

const STOP_GRACE: Duration = Duration::from_secs(15);

impl Actor {
    fn new(threads: Arc<Threads>, row: ThreadRow, snap: Arc<Mutex<Snapshot>>) -> Actor {
        Actor {
            threads,
            row,
            snap,
            session: None,
            drv: None,
            queue: VecDeque::new(),
            asks: HashSet::new(),
            last_active: Instant::now(),
            stopping: None,
            sleep_after: false,
            restart: false,
            closing: false,
        }
    }

    fn emit(&mut self, kind: EventKind) -> u64 {
        let seq = self.threads.outbox.push(&self.row.id, kind);
        self.row.last_seq = seq;
        seq
    }

    fn save(&self) {
        self.threads.db.save_thread(self.row.clone());
    }

    fn set_state(&mut self, s: ThreadState) {
        if self.row.state != s {
            self.row.state = s;
            self.emit(EventKind::State { state: s });
            self.save();
        }
        self.snap.lock().unwrap().state = s;
    }

    fn set_live(&self, live: bool) {
        self.snap.lock().unwrap().live = live;
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        if self.row.state.busy() {
            // agentd restarted mid-turn.
            self.emit(EventKind::Notice { text: "agentd restarted: the turn was cut short. The next message resumes the session.".into() });
            self.emit(EventKind::TurnDone { turn: self.row.turn, stop: StopReason::Interrupted, usage: None, cost_usd: None, duration_ms: 0 });
            self.set_state(ThreadState::Interrupted);
        }
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let drv = async {
                match self.drv.as_mut() {
                    Some(r) => r.recv().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(c) => self.command(c).await,
                    None => break,
                },
                out = drv => match out {
                    Some(o) => self.driver(o).await,
                    None => self.drv = None,
                },
                _ = tick.tick() => self.tick().await,
            }
        }
        self.close().await;
    }

    async fn tick(&mut self) {
        if let Some(t) = self.stopping
            && t.elapsed() > STOP_GRACE
        {
            // The agent ignored the interrupt: end it.
            tracing::warn!("thread {}: the agent did not stop; ending it", self.row.id);
            self.close().await;
            self.force_end(StopReason::Interrupted).await;
        }
        if self.session.is_some() && !self.row.state.busy() && self.last_active.elapsed() >= self.threads.idle {
            tracing::info!("thread {}: idle, ending its agent process", self.row.id);
            self.close().await;
        }
    }

    async fn close(&mut self) {
        if let Some(mut s) = self.session.take() {
            self.closing = true;
            s.close().await;
        }
        self.drv = None;
        self.asks.clear();
        self.set_live(false);
    }

    async fn command(&mut self, c: Cmd) {
        match c {
            Cmd::Start { spec, resume, input, reply } => {
                let changed = spec.agent != self.row.spec.agent || spec.cwd != self.row.spec.cwd || spec.permission != self.row.spec.permission;
                if changed && self.session.is_some() && !self.row.state.busy() {
                    self.close().await;
                }
                if let Some(a) = spec.agent {
                    self.row.agent = a;
                }
                self.snap.lock().unwrap().cwd = spec.cwd.clone();
                self.row.spec = spec;
                if let Some(r) = resume
                    && self.row.session_id.as_deref() != Some(&r)
                {
                    if self.session.is_some() && !self.row.state.busy() {
                        self.close().await;
                    }
                    self.row.session_id = Some(r);
                }
                self.save();
                let last_seq = self.threads.outbox.last_seq(&self.row.id);
                match input {
                    Some(input) => self.send(input, SendMode::Queue, reply).await,
                    None => {
                        let _ = reply.send(Ok(json!({"thread": self.row.id, "last_seq": last_seq})));
                    }
                }
            }
            Cmd::Send { input, mode, reply } => self.send(input, mode, reply).await,
            Cmd::Stop { reply } => {
                self.sleep_after = false;
                let dropped = self.drop_queue();
                if self.row.state.busy() {
                    let ok = match self.session.as_mut() {
                        Some(s) => s.interrupt().await,
                        None => Err(anyhow::anyhow!("no session")),
                    };
                    match ok {
                        Ok(()) => self.stopping = Some(Instant::now()),
                        Err(e) => {
                            tracing::info!("thread {}: interrupt failed ({e}); ending the agent", self.row.id);
                            self.close().await;
                            self.force_end(StopReason::Interrupted).await;
                        }
                    }
                }
                let _ = reply.send(Ok(json!({"dropped": dropped})));
            }
            Cmd::Answer { ask, option, text, reply } => {
                let r = match self.session.as_mut() {
                    Some(s) => s.answer(&ask, &option, text.as_deref()).await.map(|_| Value::Null),
                    None => Err(anyhow::anyhow!("the agent is not running")),
                };
                let _ = reply.send(r);
            }
            Cmd::SetModel { model, effort, reply } => {
                self.row.spec.model = Some(model.clone());
                if effort.is_some() {
                    self.row.spec.effort = effort.clone();
                }
                self.save();
                if let Some(s) = self.session.as_mut() {
                    match s.set_model(&model, effort.as_deref()).await {
                        Ok(true) => {}
                        Ok(false) => self.restart = true,
                        Err(e) => {
                            tracing::info!("thread {}: live model switch failed ({e}); restarting the session", self.row.id);
                            self.restart = true;
                        }
                    }
                    if self.restart && !self.row.state.busy() {
                        self.restart = false;
                        self.close().await;
                    }
                }
                let what = match &effort {
                    Some(e) => format!("Model: {model} ({e})"),
                    None => format!("Model: {model}"),
                };
                self.emit(EventKind::Notice { text: what });
                let _ = reply.send(Ok(Value::Null));
            }
            Cmd::Sleep { reply } => {
                if self.row.state.busy() {
                    self.sleep_after = true;
                    if let Some(s) = self.session.as_mut() {
                        let _ = s.interrupt().await;
                    }
                    self.stopping = Some(Instant::now());
                } else {
                    self.close().await;
                }
                let _ = reply.send(Ok(Value::Null));
            }
        }
    }

    /// Queued messages are dropped (a stop): say so for each.
    fn drop_queue(&mut self) -> usize {
        let q: Vec<(Input, u64)> = self.queue.drain(..).collect();
        for (_, seq) in &q {
            self.emit(EventKind::Dequeued { seq_of_user: *seq, dropped: true });
        }
        q.len()
    }

    async fn send(&mut self, input: Input, mode: SendMode, reply: Reply) {
        self.last_active = Instant::now();
        if self.row.state.busy() {
            if mode == SendMode::Steer
                && let Some(s) = self.session.as_mut()
                && matches!(s.steer(&input).await, Ok(true))
            {
                self.emit(EventKind::User { text: input.text, images: input.images, queued: false });
                let _ = reply.send(Ok(json!({"queued": false, "steered": true})));
                return;
            }
            let seq = self.emit(EventKind::User { text: input.text.clone(), images: input.images.clone(), queued: true });
            if mode == SendMode::Interrupt {
                self.queue.push_front((input, seq));
                if let Some(s) = self.session.as_mut()
                    && let Err(e) = s.interrupt().await
                {
                    tracing::info!("thread {}: interrupt failed: {e}", self.row.id);
                }
            } else {
                self.queue.push_back((input, seq));
            }
            let _ = reply.send(Ok(json!({"queued": true})));
            return;
        }
        self.emit(EventKind::User { text: input.text.clone(), images: input.images.clone(), queued: false });
        let _ = reply.send(Ok(json!({"queued": false})));
        self.deliver(input).await;
    }

    async fn ensure_session(&mut self) -> anyhow::Result<()> {
        if self.session.is_some() {
            return Ok(());
        }
        let agent = self.row.agent;
        let bin = agent_bin(&self.threads.cfg, agent).with_context(|| format!("{} is not installed on this machine", agent.label()))?;
        let (tx, rx) = mpsc::unbounded_channel();
        let cwd = PathBuf::from(&self.row.spec.cwd);
        // The project's browser, if it has one on (woken if it slept).
        let env = self.threads.surf.env_for(&cwd).await;
        let opts = StartOpts {
            thread: self.row.id.clone(),
            bin,
            cwd,
            model: self.row.spec.model.clone(),
            effort: self.row.spec.effort.clone(),
            permission: self.row.spec.permission,
            resume: self.row.session_id.clone(),
            instructions: self.row.spec.instructions.clone(),
            env,
        };
        let resuming = opts.resume.is_some();
        let s = self.threads.agents.get(agent).start(opts, tx).await?;
        tracing::info!("thread {}: {} {}", self.row.id, agent.label(), if resuming { "resumed" } else { "started" });
        self.session = Some(s);
        self.drv = Some(rx);
        self.closing = false;
        self.set_live(true);
        Ok(())
    }

    /// Start a turn with `input` now.
    async fn deliver(&mut self, input: Input) {
        self.last_active = Instant::now();
        if self.session.is_none() {
            self.set_state(ThreadState::Starting);
        }
        if let Err(e) = self.ensure_session().await {
            self.emit(EventKind::Error { message: format!("{e:#}"), fatal: false });
            self.set_state(ThreadState::Error);
            return;
        }
        self.row.turn += 1;
        let turn = self.row.turn;
        self.emit(EventKind::TurnStarted { turn });
        self.set_state(ThreadState::Running);
        let r = self.session.as_mut().unwrap().send(&input).await;
        if let Err(e) = r {
            self.emit(EventKind::Error { message: format!("{e:#}"), fatal: false });
            self.close().await;
            self.force_end(StopReason::Error).await;
        }
    }

    /// End the turn ourselves (the agent can't say so any more).
    async fn force_end(&mut self, stop: StopReason) {
        self.emit(EventKind::TurnDone { turn: self.row.turn, stop, usage: None, cost_usd: None, duration_ms: 0 });
        self.end_turn(stop).await;
    }

    /// The turn is over (the agent said so, or we decided): settle the state, then the queue.
    fn end_turn(&mut self, stop: StopReason) -> futures::future::BoxFuture<'_, ()> {
        Box::pin(self.end_turn_inner(stop))
    }

    async fn end_turn_inner(&mut self, stop: StopReason) {
        self.last_active = Instant::now();
        self.asks.clear();
        let stopped = self.stopping.take().is_some();
        if self.sleep_after {
            self.sleep_after = false;
            self.close().await;
        }
        if !stopped && let Some((input, seq)) = self.queue.pop_front() {
            self.emit(EventKind::Dequeued { seq_of_user: seq, dropped: false });
            self.deliver(input).await;
            return;
        }
        let state = match stop {
            StopReason::End | StopReason::MaxTurns => ThreadState::Idle,
            StopReason::Interrupted => ThreadState::Interrupted,
            StopReason::Error => ThreadState::Error,
        };
        self.set_state(if stopped { ThreadState::Interrupted } else { state });
        if self.restart {
            self.restart = false;
            self.close().await;
        }
    }

    async fn driver(&mut self, o: DriverOut) {
        match o {
            DriverOut::Event(kind) => {
                self.last_active = Instant::now();
                match kind {
                    EventKind::Session { session_id, model } => {
                        if self.row.session_id.as_deref() != Some(&session_id) {
                            self.row.session_id = Some(session_id.clone());
                            self.save();
                        }
                        self.emit(EventKind::Session { session_id, model });
                    }
                    EventKind::Ask { .. } => {
                        if let EventKind::Ask { id, .. } = &kind {
                            self.asks.insert(id.clone());
                        }
                        self.emit(kind);
                        if self.row.state.busy() {
                            self.set_state(ThreadState::Waiting);
                        }
                    }
                    EventKind::Answered { .. } => {
                        if let EventKind::Answered { id, .. } = &kind {
                            self.asks.remove(id);
                        }
                        self.emit(kind);
                        if self.row.state == ThreadState::Waiting && self.asks.is_empty() {
                            self.set_state(ThreadState::Running);
                        }
                    }
                    EventKind::TurnDone { stop, usage, cost_usd, duration_ms, .. } => {
                        if !self.row.state.busy() {
                            // A stray end (e.g. of a turn we already closed): keep it out of the log.
                            tracing::debug!("thread {}: TurnDone while {:?}", self.row.id, self.row.state);
                            return;
                        }
                        self.emit(EventKind::TurnDone { turn: self.row.turn, stop, usage, cost_usd, duration_ms });
                        self.end_turn(stop).await;
                    }
                    other => {
                        self.emit(other);
                    }
                }
            }
            DriverOut::Exited(err) => {
                let ours = self.closing;
                self.session = None;
                self.drv = None;
                self.closing = false;
                self.set_live(false);
                if ours {
                    return;
                }
                tracing::info!("thread {}: the agent exited ({})", self.row.id, err.as_deref().unwrap_or("ok"));
                if self.row.state.busy() {
                    let msg = err.unwrap_or_else(|| "The agent exited during the turn".into());
                    self.emit(EventKind::Error { message: msg, fatal: false });
                    let stop = if self.stopping.is_some() { StopReason::Interrupted } else { StopReason::Error };
                    self.force_end(stop).await;
                } else if let Some(e) = err {
                    self.emit(EventKind::Notice { text: format!("The agent process ended: {e}") });
                }
            }
        }
    }
}
