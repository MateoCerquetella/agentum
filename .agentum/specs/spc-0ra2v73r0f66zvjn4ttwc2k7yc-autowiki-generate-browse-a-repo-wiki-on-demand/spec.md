---
schema: 1
id: SPC-0RA2V73R0F66ZVJN4TTWC2K7YC
revision: 1
title: AutoWiki (generate & browse a repo wiki on demand)
source: legacy-import:ai/specs/001-autowiki/spec.md@sha256:78e4bb5e8ea9d4253c89fd35403aea7c049fbbd8e6ef19573a0e00b6fb6b8ebd
---

# AutoWiki (generate & browse a repo wiki on demand)

## Migration provenance

This historical specification was assigned a stable Agentum identity during the
v2 cutover. Its source is included below and its exact original bytes are also
preserved in the external recovery archive and accounted for by SHA-256.

## Requirements

- RQ-001 Preserve the historical specification's stable identity and source provenance.
- RQ-002 Treat this imported revision as historical context until a user explicitly reopens it.

## Acceptance criteria

- AC-001 The source path and SHA-256 match the migration inventory and recovery archive.
- AC-002 New work on this specification creates an immutable later revision through Agentum.

## Imported historical source

> # Spec 001 — AutoWiki (generate & browse a repo wiki on demand)
>
> - **Number:** 001
> - **Status:** PM  <!-- Draft | PM | Architect | In progress | Done -->
> - **Surface:** `crates/agentum-desktop/ui` (new Wiki view) + `crates/agentum-server/src/routes/wiki.rs` (new) + on-disk `.agentum/wiki/`
> - **Author:** Claude (drafted with Mateo)
> - **Date:** 2026-06-30
> - **Base:** `feat/autowiki` off `origin/develop` @ `fe1a2a6a` (citations below are current as of this commit)
>
> ## Problem
>
> Understanding an unfamiliar codebase means reading scattered files. agentum
> already hands agents the *whole* repo's context (`gather_repo_context` —
> `crates/agentum-server/src/routes/chat.rs:207` — inlines the guide + manifests +
> git file tree for Chat/spec work) but **never turns that understanding into a
> durable, navigable artifact a human can browse.** The only docs that exist are
> hand-written and rot (`docs/ARCHITECTURE.md`, `docs/DATA-MODEL.md`). A new
> contributor — or the maintainer six months later — has no "start here" map of the
> project, generated from the code as it actually is.
>
> ## Goal
>
> Let a user point agentum at a local repo and, on demand, generate a structured,
> navigable **wiki** (Overview + Architecture-with-diagram + one page per module)
> that renders in a new **Wiki** view and persists across restarts.
>
> > One slice: **generate + browse, on demand.** A basic mermaid architecture
> > diagram is in scope (it renders for free, see AC-6). *Keeping the wiki in sync as
> > code changes*, and richer-than-mermaid diagrams, are Phase 2.
>
> ## Users / personas
>
> - **New contributor, day one.** Opens an agentum-managed repo they've never seen
>   and wants its shape — modules, responsibilities, where things live — without
>   reading every file.
> - **Maintainer.** Wants living, generated docs they didn't have to hand-write,
>   good enough to point a teammate at.
>
> ## Acceptance criteria
>
> 1. A new **Wiki** item appears in the desktop nav rail; selecting it sets
>    `activeView === 'wiki'` and opens a Wiki view with a left page-tree (TOC) and a
>    right content pane.
> 2. With no wiki generated yet, the Wiki view shows an **explained empty state**
>    with a single **Generate wiki** action (not a blank screen).
> 3. **Generate** for the active repo spawns an agent through the existing one launch
>    path (`spawn_agent_into_pane`, `routes/sessions/provision.rs:91`) into the
>    repo's workdir, with its prompt grounded by `gather_repo_context`; the run is
>    observable/streamable like any other session.
> 4. The agent writes a set of markdown pages + a TOC index to `.agentum/wiki/`
>    (`index.json` + `<slug>.md`), including **at minimum**: an `Overview` page, an
>    `Architecture` page, and **one page per top-level module/crate** (for agentum
>    itself: the `crates/*` map).
> 5. On completion, the TOC lists the generated pages and the content pane **renders
>    the selected page's markdown** via the existing `MarkdownPreview`
>    (`components/editor/MarkdownPreview.tsx`) — headings, code blocks, lists, links.
> 6. The `Architecture` page contains a ` ```mermaid ` block that **renders as a
>    diagram**, via `MarkdownPreview`'s existing mermaid interception
>    (`MarkdownPreview.tsx:1458-1465` → `MermaidBlock`) — **no new render code**.
> 7. **Internal links navigate within the view**: clicking a module/`[[page]]` link
>    opens that page in the content pane (reuse the editor's `markdown-doc-*`
>    wiki-link resolver).
> 8. The generated wiki **persists across app restarts** — reopening Wiki shows the
>    last-generated pages (read from `.agentum/wiki/`) without regenerating.
> 9. **Failure is loud.** If the run fails or writes no/garbled `index.json`, the
>    view shows a clear error — never a half-empty "success" (mirrors the QA gate's
>    "missing/garbled verdict = fail", `harness/helpers.rs:132` `parse_qa_verdict`).
> 10. Served by a new `routes/wiki.rs` (`GET /api/wiki`, `GET /api/wiki/{slug}`,
>     `POST /api/wiki/generate`), registered in `lib.rs` like
>     `routes::notes::router()` (`lib.rs:277`) — **authed by default** via the global
>     `auth::require_token` layer (`lib.rs:300`), and reachable on the loopback
>     embedded server.
>
> ## Scope & non-goals (YAGNI)
>
> - **In (v1):** on-demand generation for a **local** repo; on-disk persistence; a
>   Wiki view with TOC + markdown rendering + a mermaid architecture diagram +
>   intra-wiki navigation; regenerate replaces the prior wiki.
> - **Out (deferred):**
>   - **Auto-sync** / drift detection / regenerate-on-merge (**Phase 2** — the
>     "kept in sync as code changes" promise).
>   - **Diagrams beyond a mermaid block** (interactive/graph-rendered, clickable
>     nodes) — **Phase 2**.
>   - Full-text **search** across the wiki.
>   - **Per-branch / multi-version** wikis.
>   - **In-app hand-editing** of wiki pages.
>   - **Remote-over-SSH** repos (v1 is local; SSH parity is a later slice).
>
> ## Reuse vs build (grounded — `origin/develop` @ `fe1a2a6a`)
>
> ### Reuse — do NOT rebuild
>
> - **Ground the prompt:** `gather_repo_context(workdir) -> Option<String>`
>   (`routes/chat.rs:207`; already used for interview + extraction at `:369/:471/:990`).
> - **Spawn the agent (one launch path):** `spawn_agent_into_pane`
>   (`routes/sessions/provision.rs:91`) — keeps YOLO translation, loopback env, hooks
>   and MCP wiring centralized.
> - **Capture the produced artifact (the recipe to copy):** `run_qa_agent_gate`
>   (`harness/drive.rs:476`) — compute output path (`qa_verdict_path`,
>   `harness/helpers.rs:123`) → clear stale → spawn → prompt the agent to write that
>   file (`build_qa_prompt`, `helpers.rs:141`) → wait-for-settle → read back +
>   parse, missing/garbled = fail (`parse_qa_verdict`, `helpers.rs:132`). AutoWiki
>   generation IS this pattern.
> - **Render markdown + diagrams:** `MarkdownPreview` (`components/editor/MarkdownPreview.tsx`,
>   react-markdown + full plugin stack) which **already** intercepts ` ```mermaid `
>   fences (`:1458-1465`) → `MermaidBlock` (`MermaidBlock.tsx:2`, `mermaid@^11`).
> - **Inter-page links:** the editor's `markdown-doc-*` `[[wiki-link]]` resolver
>   (`components/editor/markdown-doc-completions*`).
> - **Route + auth boilerplate:** `routes/notes.rs:13` `pub fn router()` template;
>   registered at `lib.rs:277`; global auth at `lib.rs:300` (`auth.rs:74` `is_public`
>   whitelist — do NOT add `/api/wiki` to it).
> - **Persistence precedent (if a DB index is later wanted):** the per-domain store
>   module pattern, e.g. `store/notes.rs:10` `create_note` / `:35` `list_notes`
>   (+ numbered migrations, latest `0024_*`). **v1 does not need this** (on-disk).
>
> ### Build new
>
> - **`routes/wiki.rs`** — `GET /api/wiki` (read `index.json`), `GET /api/wiki/{slug}`
>   (read `<slug>.md`), `POST /api/wiki/generate` (spawn + capture).
> - **The generation contract** — the `.agentum/wiki/` layout (`index.json` schema +
>   `<slug>.md`) and the generation prompt (grounded by `gather_repo_context`; the
>   agent enumerates modules from its workdir — note `routes/fs.rs:21` is
>   single-level `read_dir` only, there is no server-side recursive walker, and we
>   don't need one).
> - **The desktop Wiki page-shell** — rail item + `activeView === 'wiki'` view
>   (TOC + content). Mounts `MarkdownPreview`/`MermaidBlock` standalone. *Gap:* the
>   harness file viewer renders raw `<pre>`, so a real rendered doc-page shell does
>   not exist yet — this is the one non-trivial new UI piece.
>
> ### Steps to add the `wiki` view (all precedented)
>
> 1. Add `'wiki'` to the `activeView` union (`store/slices/ui.ts:440`).
> 2. Add `openWikiPage` + `previousViewBeforeWiki` (mirror `openHarnessPage`,
>    `ui.ts:1035-1041`).
> 3. Add a `<PrimaryNavItem … onClick={openWikiPage} />` in `SidebarNav.tsx`
>    (`:34` component, rail items ~`:189`).
> 4. Add `{activeView === 'wiki' ? <WikiPage/> : null}` in `App.tsx` (~`:1754`,
>    lazy-imported like `MissionControlPage` at `:221`).
>
> ## Risks & invariants
>
> - **One launch path.** Generation MUST go through `spawn_agent_into_pane` — no
>   bespoke launch (preserves YOLO translation, loopback env, MCP wiring).
> - **Inconclusive ≠ success** (AC-9). A failed/garbled run must surface an error,
>   never a half-empty wiki.
> - **Context budget.** `gather_repo_context` caps ~1500 files / ~22k tokens; a large
>   repo must degrade gracefully and **log which modules were skipped** rather than
>   silently truncate coverage.
> - **Auth parity.** `/api/wiki` is not public — token-gated on a networked daemon
>   (don't touch `is_public`), open only on the loopback-bound embedded server.
> - **No repo pollution surprise.** `.agentum/wiki/` is written into the repo workdir
>   (the "wiki lives with the code" model); ensure it's `.gitignore`-able / opt-in to
>   commit, not a forced commit.
>
> ## Harness wiring (the gate)
>
> Delivery follows the repo flow: file this spec as a GitHub **epic/issue** (ACs as
> checkboxes — the issue is the live status board), then drive it via the Harness
> Engine. Proposed `.harness/feature_list.json` slices (ordered, on-disk — no DB
> migration):
>
> 1. `wiki-contract` — `.agentum/wiki/` layout (`index.json` schema + `<slug>.md`) +
>    the grounded generation prompt.
> 2. `wiki-routes` — `routes/wiki.rs` (list / page / generate); generate spawns via
>    `provision::spawn_agent_into_pane` and captures via the `run_qa_agent_gate`
>    recipe; register in `lib.rs`, authed by default.
> 3. `wiki-view` — desktop Wiki rail item + view (TOC + `MarkdownPreview` render +
>    mermaid diagram + `[[link]]` nav + empty/failure states).
>
> - **`verify.sh` asserts:** `GET /api/wiki` round-trips a fixture `.agentum/wiki/`
>   (list + page content); `cargo test -p agentum-server --lib` green;
>   `npm run build --prefix crates/agentum-desktop/ui` green.
> - **`qa.sh` asserts (browser QA):** open Wiki → empty state → **Generate** → agent
>   run visible → pages appear in TOC → selecting a page renders markdown → the
>   Architecture page shows a **mermaid diagram** → an internal link navigates.
>   Screenshot evidence per the `browser-verification-loop` skill.
>
> ## Open questions
>
> 1. **Module enumeration heuristic.** "One page per module" = Cargo workspace
>    members (`crates/*`) + top-level UI dirs for this repo; for arbitrary repos,
>    top-level dirs / package manifests. Confirm the heuristic the prompt instructs.
> 2. **Active-repo plumbing.** `Generate` targets the active worktree/workspace —
>    confirm the `activeWorktreeId` → workdir path the `POST /api/wiki/generate`
>    route needs (the id is already in `App.tsx`).
> 3. **`.agentum/wiki/` commit policy.** Default to git-ignored (app-local), or
>    commit-with-the-repo (Ara model)? Leaning ignored-by-default + an explicit
>    "commit wiki" affordance later.
> 4. **Regeneration semantics.** Full replace of `.agentum/wiki/` each run (v1) is
>    simplest; per-page incremental update is a Phase-2 concern tied to sync.
>
> ---
>
> ### Resolved during drafting (was open)
> - **Persistence:** on-disk `.agentum/wiki/*.md` + `index.json` (reuses the harness
>   artifact pattern; survives restart; optionally commit-able). DB table deferred.
> - **Repo context:** `gather_repo_context` exists and is reused (not rebuilt).
> - **Diagrams:** a mermaid architecture block is **in v1** — the renderer already
>   draws it for free.
