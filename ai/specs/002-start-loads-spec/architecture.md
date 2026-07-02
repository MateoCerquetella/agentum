# Spec 002 — Architecture Blueprint: Start an external ticket → the agent gets the spec (no internal board)

**Self-check passed.** `fn compose_issue_body` is at `routes/chat.rs:914` — current `chat-spec-roundtrip` tree.

**Status:** Architect → ready for Developer (pending the **R1** product decision below).
**Scope (Resolved):** Start-only, external-ticket-direct (no `BoardItem`), live body fetch. Creation untouched.

---

## 0. TL;DR — recommended shape (Option A)

One server entry point that starts an agent directly from an external ticket, reusing every spawn internal except the card coupling:

1. `fetch_ticket_body(provider, id, slug, …)` — live read via existing `gh_in_dir` (GitHub) / `linear::graphql` (Linear).
2. `build_ticket_prompt(header, body)` — pure, seeded by the fetched **body**; graceful fallback + redaction.
3. `start_external_ticket(state, req)` — mirrors `spawn_card_session` (`board_goals.rs:737`) but writes **no `BoardItem`**: `store.create_session` (not `claim_card`) → `spawn_agent_into_pane` → `inject_prompt`.
4. `POST /api/tickets/start` (new `routes/tickets.rs`) — namespaced away from `/api/board`.
5. Desktop: a **"Start"** action on the external Tasks card/detail calling a new `startTicket()` client fn.

**Reuse-as-is (don't touch):** `compose_issue_body` (`chat.rs:914`) + the whole external-only creation path (`chat.rs:1011-1018`, `1050`, `1143`).

---

## 1. ⚠️ A spec assumption that is WRONG on the current tree (read first — this is decision R1)

The spec says Start "dies at the door" because `build_card_prompt` (`board_goals.rs:861`) only uses card columns. True **for the board-card path** — but that path is **not what the desktop's external-ticket Start runs today.** There are **two unrelated start mechanisms**, and the spec conflates them:

| | **Path A — board card (the spec's named target)** | **Path B — Tasks "Use" (what the desktop actually does)** |
|---|---|---|
| Trigger | Board card → `moveCard('doing')` → `PATCH /api/board/{id}` (`board.rs:373-384`) | Tasks Kanban → "Use" → `handleUseWorkItem` (`TaskPage.tsx:2365`) → New Workspace composer |
| Spawn | `spawn_card_session(card)` (`board_goals.rs:737`) → `spawn_agent_into_pane` (tmux server session) | `createWorktree` (`useComposerState.ts:2008`) → **local PTY terminal** (`new-workspace.ts:253-305`) |
| Prompt | `build_card_prompt` — card columns only (`board_goals.rs:861`) | `buildAgentPromptWithContext` (`new-workspace.ts:134`) from `linkedContext`/URL |
| Body today | never fetched (the spec's complaint) | **Linear: yes** (`linear-linked-work-item.ts:5-27` snapshots `description`); **GitHub: no** — `openComposerForItem` sets only `{type,number,title,url}` (`TaskPage.tsx:2347-2362`) → agent gets the URL, not the body |
| Reachable from UI? | **No caller.** `startCard`/`moveCard` (`board-client.ts:191-205`) imported nowhere in `components/` | Yes — this is the live "start work on a ticket" flow |

**Consequence:** fixing `build_card_prompt` alone (the Problem's literal framing) changes nothing a user sees — `spawn_card_session` has no Start button. So the architect's first decision is **which runtime "Start" targets:**

- **Option A (recommended — spec-faithful):** build server `start_external_ticket` on `spawn_agent_into_pane` + a **new** "Start" button. Why: satisfies the spec invariant *"one launch path `spawn_agent_into_pane`"* and produces a first-class **server session** — the atom the sidebar/agent-list/watchdog/harness key off. A local-PTY agent is invisible to all of that.
- **Option B (lighter, contradicts the spec):** skip the server — make GitHub "Use" fetch the body into `linkedContext` (mirror `linear-linked-work-item.ts`), ~1 file + one `gh issue view`. Why-not: launches into a **local PTY**, violates *one launch path*, doesn't touch `board_goals.rs`, leaves the body absent for the harness/QA session model.

> **DECISION FOR MATEO (R1):** the spec's "Resolved" names `board_goals.rs` + `spawn_agent_into_pane`, so this blueprint builds **Option A**. But because it adds a server "Start" **next to** the existing local-PTY "Use", the two-runtimes duplication needs a product call (see §5, R1). The deeper point: the user's "when I click Start" may actually map to the **"Use"** button (Path B, live), not the dead Path A the spec named.

Everything below specs Option A.

---

## 2. The external-ticket Start path (no `BoardItem`)

Today's card-Start: `PATCH /api/board/{id}` status→`doing` (`board.rs:373`) → `spawn_card_session(&state, &item)` (`board.rs:384`). Inside `spawn_card_session` (`board_goals.rs:737-855`): `session_name = "card-<key>"` (L789, needs a card key) → `provision_card_worktree` (L794) → `NewSession { card_id: Some(card.id), flags:[YOLO_MARKER], worktree_* }` (L801) → `store.claim_card` (L819, **the hard coupling** — atomic dual-write requiring a card row, `binding.rs:30`) → `spawn_agent_into_pane` (L829, the one launch path) → `inject_prompt(build_card_prompt(card))` fire-and-forget (L844).

**Recommendation: a parallel `start_external_ticket` (don't decouple `spawn_card_session`).** Why: `claim_card`'s dual-write (409-on-already-bound, card↔session FK) is load-bearing for the card path + tested (`store/lib.rs:1442+`); forking avoids destabilizing it for zero benefit.

```rust
// routes/board_goals.rs (new) — mirrors spawn_card_session minus the card
pub(crate) async fn start_external_ticket(state: &AppState, req: &StartTicketReq)
  -> Result<String /* session_id */, ApiError> {
    let tool = req.tool.clone().unwrap_or_else(|| "claude".into());
    let wd = super::util::expand_workdir(&req.workdir)?;     // board_goals.rs:781
    if !wd.exists() { return Err(ApiError::BadRequest(/* workdir does not exist */)); }
    let session_name = ticket_session_name(&req.provider, &req.id);     // §2.1
    let worktree = provision_card_worktree(&wd, &session_name).await?;  // REUSE board_goals.rs:664
    let new = NewSession {
        name: session_name, workdir: wd.to_string_lossy().into_owned(),
        tool, model: req.model.clone(),
        flags: vec![agentum_executor::YOLO_MARKER.to_string()],  // YOLO rule
        card_id: None,                                           // <-- AC-3: no card
        worktree_path:   worktree.as_ref().map(|w| w.path.to_string_lossy().into_owned()),
        worktree_branch: worktree.as_ref().map(|w| w.branch.clone()),
        worktree_base_ref: worktree.as_ref().map(|w| w.base_ref.clone()),
    };
    let session = state.store.create_session(new).await?;       // <-- not claim_card (sessions.rs:15)
    let host = state.store.get_host(agentum_core::LOCAL_HOST_ID).await?
        .ok_or_else(|| ApiError::Internal("local host missing".into()))?;
    let target = agentum_tmux::target_for(&session.name);
    let spawn_wd = super::util::expand_workdir(session.effective_cwd())?;
    super::sessions::spawn_agent_into_pane(state, &session, &host, &target, &spawn_wd).await?;
    // fire-and-forget: fetch body live + inject (never blocks the HTTP response)
    let (s2, sess2, req2, host2) = (state.clone(), session.clone(), req.clone(), host.clone());
    tokio::spawn(async move {
        let prompt = ticket_opening_prompt(&s2, &req2, &host2).await;     // §3/§4
        if let Err(e) = crate::harness::inject_prompt(&s2, &sess2, &prompt).await {
            tracing::warn!(error=%e, "ticket prompt inject failed; session still running");
        }
    });
    Ok(session.id.to_string())
}
```

`store.create_session(NewSession)` (`sessions.rs:15`) is the plain insert used by `POST /api/sessions`; accepts `card_id: None`, runs `validate_name`. Exactly "a session with no card."

### 2.1 Session name / worktree FK with no card `key`
```rust
fn ticket_session_name(provider: &str, id: &str) -> String {
    // github #42 -> "ticket-github-42"; Linear ENG-123 -> "ticket-linear-eng-123"
    let id = id.trim().to_ascii_lowercase()
        .chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>();
    format!("ticket-{}-{}", provider, id.trim_matches('-'))
}
```
Mirrors `card-<key>` (L789); passes `validate_name` (lowercase + dashes). The worktree branch derives from this name → `ticket-github-42`. (Open Q2 answered: session FK = `ticket-<provider>-<id>`.)

> Cosmetic follow-up (not blocking): rename `provision_card_worktree` → `provision_session_worktree`. Leave for later to keep the diff tight.

---

## 3. Live body fetch — `fetch_ticket_body(provider, id)`

Called **inside the fire-and-forget task**, not on the request path. Why: `inject_prompt` already waits seconds for the REPL/trust dialog, so a sub-second `gh`/Linear call is free there and a slow tracker never delays the pane or the HTTP 200 (mirrors `board_goals.rs:844`).

```rust
struct TicketBody { title: String, body: String }
async fn fetch_ticket_body(state: &AppState, req: &StartTicketReq, host: &Host) -> anyhow::Result<TicketBody> {
    match req.provider.as_str() {
        "github" => {
            let slug = match req.slug.as_deref() {
                Some(s) => s.to_string(),
                None => resolve_github_slug(host, &req.workdir, None).await   // REUSE board_goals.rs:247
                            .map_err(|r| anyhow::anyhow!("no github repo: {r:?}"))?,
            };
            let out = crate::host_runtime::gh_in_dir(host, &neutral_cwd_str(),
                &["issue","view", &req.id, "--repo", &slug, "--json", "title,body"]).await?;
            anyhow::ensure!(out.success, "gh issue view failed: {}", out.stderr);
            let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
            Ok(TicketBody { title: v["title"].as_str().unwrap_or_default().into(),
                            body:  v["body"].as_str().unwrap_or_default().into() })
        }
        "linear" => { let (title, body) = crate::linear::fetch_issue_body(&req.id).await?; Ok(TicketBody { title, body }) }
        other => anyhow::bail!("unknown ticket provider {other}"),
    }
}
```

### 3.1 GitHub — reuse `gh_in_dir`
`gh_in_dir(host, cwd, args)` (`host_runtime/git_fs.rs:90`) — host-aware `gh` runner already used to *create* issues remotely (`board_goals.rs:412`), local analogue of `TaskSink::Github`'s shell-out (`task_sink.rs:155`). Use `--repo <slug>` + `neutral_cwd` (`task_sink.rs:310`) so it runs from `$HOME` (identical to `gh_create_argv_with_repo`, `task_sink.rs:300`). `--json title,body` → parse with `serde_json`. Why: zero new shell/auth surface.

### 3.2 Linear — reuse `graphql` + `pick_token`
Add a query + thin fetch in `linear.rs`, reusing `graphql(token, query, vars)` (`linear.rs:93`) + `pick_token(read_creds())` (`linear.rs:77`) — same machinery `transition_issue` uses (`linear.rs:307`). The `$id` var pattern is proven by `ISSUE_STATES_QUERY` (`linear.rs:21`); Linear's body field is `description`.

```rust
// linear.rs (new)
const ISSUE_BODY_QUERY: &str = "query($id: String!){ issue(id:$id){ title description } }";
pub async fn fetch_issue_body(identifier: &str) -> anyhow::Result<(String, String)> {
    let token = pick_token(&read_creds()).ok_or_else(|| anyhow::anyhow!("no Linear token configured"))?;
    let resp = graphql(&token, ISSUE_BODY_QUERY, json!({ "id": identifier })).await?;
    let issue = resp.pointer("/data/issue").ok_or_else(|| anyhow::anyhow!("Linear issue {identifier} not found"))?;
    Ok((issue["title"].as_str().unwrap_or_default().into(), issue["description"].as_str().unwrap_or_default().into()))
}
```
(Open Q3 answered: GitHub = `gh issue view --json`, Linear = one `graphql` call, both behind `fetch_ticket_body`.)

---

## 4. The prompt builder — seeded by the body, graceful fallback, redacted

Keep the **pure** prompt shape separate from the **IO** so `verify.sh` can unit-test against a stubbed body (no network).

```rust
// pure — unit-tested (mirrors build_card_prompt purity, board_goals.rs:861)
fn build_ticket_prompt(header: &str, body: Option<&str>) -> String {
    match body.map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) => format!("Working on {header}\n\n{b}"),  // AC-1
        None    => format!("Working on {header}\n\n(could not load the ticket description; \
                            proceed from the title and ask if you need the full spec)"),  // AC-5
    }
}
async fn ticket_opening_prompt(state: &AppState, req: &StartTicketReq, host: &Host) -> String {
    let header = format!("{}#{}: ", req.provider, req.id);
    match fetch_ticket_body(state, req, host).await {
        Ok(t) => build_ticket_prompt(&format!("{}#{}: {}", req.provider, req.id, t.title), Some(&t.body)),
        Err(e) => {
            let safe = crate::routes::chat::redact(&e.to_string(), linear_token_for_redaction()); // AC-5
            tracing::warn!(error=%safe, "ticket body fetch failed; starting from title only");
            build_ticket_prompt(&header, None)
        }
    }
}
```

Recommendations:
- **Augment, don't replace, `build_card_prompt`.** Extract a shared `opening_prompt(header, body)` core that both `build_card_prompt` (header `<key>: <title>`) and `build_ticket_prompt` (header `<provider>#<id>: <title>`) call. Prevents card/ticket prompt drift.
- **Redaction (AC-5):** `redact` is private (`chat.rs:1230`) → promote to `pub(crate)`. The real token risk is the **Linear** path; the GitHub `gh` path holds no in-process secret (gh owns auth) → redaction there is a harmless no-op (don't hunt for a GitHub token).
- **Never a bare prompt:** the `None` arm always emits a title-anchored instruction (AC-5).

---

## 5. UI Start surface (Open Q1)

**Home:** the external **Tasks** board (`activeView === 'tasks'`) — lists GitHub/Linear/GitLab items, each carrying `number`/`linearIdentifier`, `title`, `url`, `repoId` → workdir.

Two placements (do both): (1) Kanban card action — `TaskKanbanBoard renderCard` (`TaskPage.tsx:4038`), add a small **"Start"** button; (2) work-item detail — next to the existing **"Use"** CTA (`onUse`, `TaskPage.tsx:4014`).

Wiring: `startTicket()` in a new `runtime/tickets-client.ts` (mirror `board-client.ts`'s `request()`/`authHeaders()`):
```ts
// POST /api/tickets/start
export function startTicket(input: {
  provider: 'github' | 'linear'; id: string;  // GitHub number | Linear identifier
  url?: string; workdir: string; tool?: string; slug?: string;
}): Promise<{ session_id: string }>
```
On success open the live session workspace (board path already does this — `startCard` returns the bound `session_id` then `activateAndRevealWorktree`, `TaskPage.tsx:2392`).

**Label discipline (R1):** the detail page will now show **"Use"** (local-PTY, existing) AND **"Start"** (server session, new). Differentiate: e.g. "Start agent" (tracked agent session now) vs "Use in workspace" (open a worktree to drive yourself). Same ticket, two runtimes — without distinct labels this reads as a duplicate button.

---

## 6. Reuse vs build (grounded, file:line)

**Reuse — do NOT rebuild:** `spawn_agent_into_pane` (`sessions/provision.rs:91`); `store.create_session` (`sessions.rs:15`); `provision_card_worktree` (`board_goals.rs:664`); `crate::harness::inject_prompt` (`board_goals.rs:848`); `gh_in_dir` (`host_runtime/git_fs.rs:90`) + `resolve_github_slug` (`board_goals.rs:247`) + `neutral_cwd` (`task_sink.rs:310`); `linear::graphql` (`linear.rs:93`) + `pick_token` (`linear.rs:77`); `redact` (`chat.rs:1230`, promote pub(crate)); `YOLO_MARKER` (`board_goals.rs:806`). **Creation untouched:** `compose_issue_body` (`chat.rs:914`), external-only match (`chat.rs:1011`), `create_github_issue`/`create_linear_issue` (`chat.rs:1050`/`1143`), `NewFeature` (`chat.rs:1103`).

**Build new:** `start_external_ticket` + `ticket_session_name`; `fetch_ticket_body` + `TicketBody`; `build_ticket_prompt` / shared `opening_prompt`; `linear::fetch_issue_body` + `ISSUE_BODY_QUERY`; `routes/tickets.rs` → `POST /api/tickets/start` (registered in `lib.rs::router`, auth-protected); desktop `runtime/tickets-client.ts` + the "Start" affordance.

**Route placement:** new `routes/tickets.rs`, NOT under `/api/board` (AC-3 — keep it namespaced away so nobody wires it back through the board).

---

## 7. Build order — `.harness/feature_list.json` slices

1. **`ticket-body-fetch`** — `fetch_ticket_body` + `linear::fetch_issue_body` + `ISSUE_BODY_QUERY`. Done when: `cargo test -p agentum-server --lib` green; a `#[ignore]` live test reads a real issue body per provider.
2. **`ticket-prompt-builder`** — `build_ticket_prompt`/shared `opening_prompt`; promote `redact` pub(crate). Done when: a unit test asserts the prompt **contains a stubbed body** (AC-1) and the `None` arm yields the title-only fallback (AC-5), no network. **This is the verify.sh gate.**
3. **`external-ticket-start`** — `start_external_ticket` + `ticket_session_name` (no `BoardItem`, `create_session`, `spawn_agent_into_pane`, fire-and-forget inject). Done when: a test spawns off a ticket with **no card row** and asserts a session with `card_id == None` (AC-2/AC-3).
4. **`ticket-start-route`** — `routes/tickets.rs` + `lib.rs` wiring. Done when: `POST /api/tickets/start` → `{session_id}`; auth-protected; unknown provider → typed 400.
5. **`desktop-start-surface`** — `tickets-client.ts` + "Start" on Tasks card/detail. Done when: `npm run build` + vitest green; `qa.sh`: Chat-file an issue → click **Start** → the agent's first message reflects the issue's full spec (AC-1 e2e).

Slices 1–4 backend; slice 5 desktop UI.

---

## 8. Risks & invariants

- **R1 — two start runtimes (headline).** Server "Start" lands next to the local-PTY "Use" (§1, §5). Resolve by labeling + treating server-session Start as canonical; do NOT silently ship both unlabeled. **Confirm direction with Mateo.**
- **One launch path.** `start_external_ticket` MUST go through `spawn_agent_into_pane` — never a hand-rolled tmux spawn. ✓
- **Never the internal board (AC-3).** `card_id: None`, `create_session` (not `claim_card`), route outside `/api/board`, no `TaskSink::Board`. ✓
- **Graceful on fetch failure (AC-5).** Fetch in the fire-and-forget task; failure → title-only prompt, redacted error, session still live. ✓
- **Secret redaction.** Reuse `redact`; real risk is the Linear path, not `gh`. ✓
- **YOLO marker.** Push `YOLO_MARKER` verbatim; adapter translates. ✓
- **Live-fetch cost.** One `gh`/Linear call per Start, off the request path, once per Start; no cache.
- **Workdir required for Linear too.** A Linear ticket has no repo; the UI must pass the active repo's `workdir` (which project to code in), like Chat creation resolves a local project (`chat.rs:1066`). Note in the route's 400 contract.

---

## 9. Chat creation — confirmed reuse-as-is (untouched)

Verified: `compose_issue_body` (`chat.rs:914`) writes summary + priority checklist; `resolve_provider` + the match (`chat.rs:1011-1018`) is GitHub/Linear only (hard-400 on anything else, never the board); `create_github_issue`/`create_linear_issue` (`chat.rs:1050`/`1143`) build `NewFeature { title, body: Some(compose_issue_body(plan)) }`. **Spec 002 does not modify this (AC-4).** Only chat.rs change: promote `redact` to `pub(crate)` (no behavior change).

---

## 10. Open questions — answered
- **Q1 (UI):** external Tasks board — "Start" on the Kanban card (`TaskPage.tsx:4038`) + beside "Use" on the detail (`TaskPage.tsx:4014`).
- **Q2 (decouple vs parallel):** **parallel** `start_external_ticket`; session FK = `ticket-<provider>-<id>`.
- **Q3 (fetch client):** `gh issue view --json title,body` via `gh_in_dir` (GitHub) + `linear::fetch_issue_body` via `graphql` (Linear).
- **Q4 (cost):** one off-request-path call per Start; no cache.

**Handoff → Developer:** build slices 1→5 in order; the verify.sh gate is slice 2's pure-prompt test (AC-1 + AC-5). **Get Mateo's call on R1 first** (server "Start" vs the existing local-PTY "Use") — it sets whether slice 5 adds a button or re-points the existing one. **Reviewer:** police AC-3 (no `claim_card`, no `TaskSink::Board`, route outside `/api/board`).
