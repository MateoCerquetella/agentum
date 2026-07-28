# Architecture Blueprint — Spec 001 AutoWiki

**Architect validation against the `autowiki` worktree @ `fe1a2a6a`.** Verdict: the
spec is **sound and buildable as scoped**. Reuse strategy is correct, slicing is
right, and the hardest unknown (active-repo plumbing) is **fully precedented and
low-risk**. Below: a citation audit, resolutions to the 4 open questions, concrete
artifact shapes, refined build order, and risks — plus **five places where the
spec's framing is wrong or imprecise** (called out inline with ⚠️).

---

## 1. Citation audit (verified on this tree)

| Spec citation | Status | Note |
|---|---|---|
| `gather_repo_context` — `chat.rs:207` | ✅ exact | **but it's a private `fn`** (see ⚠️-A) |
| `spawn_agent_into_pane` — `provision.rs:91` | ✅ exact | `pub(crate)`, reusable |
| `run_qa_agent_gate` — `harness/drive.rs:476` | ✅ exact | the recipe to copy |
| `qa_verdict_path` / `parse_qa_verdict` / `build_qa_prompt` — `helpers.rs:123/132/141` | ✅ exact | all three |
| `notes::router()` — `notes.rs:13` | ✅ exact | template for `wiki::router()` |
| route merge + auth — `lib.rs:277` / `:300` | ✅ exact | merge at 277, `auth::require_token` layer at 298-301 wraps every prior `.merge` |
| `is_public` — `auth.rs:74` | ✅ exact | do NOT add `/api/wiki` |
| mermaid interception — `MarkdownPreview.tsx:1458-1465` | ✅ ~ within 5 lines | comment at 1457; `<MermaidBlock>` actually rendered at **1462-1466** |
| `MermaidBlock` import | ✅ | `MarkdownPreview.tsx:61`, props `content`/`isDark`/`htmlLabels` |
| `activeView` union — `ui.ts:440` | ✅ exact | **but it's 6 unions, not 1** (see ⚠️-C) |
| `openHarnessPage` mirror — `ui.ts:1035-1041` | ✅ exact | `openHarnessPage`/`closeHarnessPage` at 1033-1042 |
| view switch — `App.tsx:1754` | ✅ ~ | `:1754`=`activity`; the **`harness`→`<ChatPage/>`** line is `:1755`; lazy-import precedent `MissionControlPage` at `:221` ✅ |
| `PrimaryNavItem` — `SidebarNav.tsx:34` | ✅ exact | rail items at 189-201 |
| `fs.rs:21` "single-level, no recursive walker" | ✅ claim holds | `:21` is the `router()` fn; the single-level listers are `list_dir`/`list_entries` below — no recursive walker exists or is needed |

**No load-bearing citation is wrong.** Line drift is cosmetic (±5). The substantive
corrections are about *reuse mechanics*, below.

---

## 2. Open questions — resolved

### (a) Module-enumeration heuristic

The agent runs **inside the workdir with YOLO + filesystem access**, so it
enumerates modules by reading the repo itself. The prompt instructs this priority
ladder (generalizes beyond agentum):

1. **Workspace members from the root manifest** (most precise): Cargo
   `[workspace].members` (here: `crates/*`), `package.json` `workspaces`,
   `pnpm-workspace.yaml`, `go.mod`, `pyproject.toml` packages.
2. **Else conventional source roots**: `crates/*`, `packages/*`, `apps/*`,
   `services/*`, `cmd/*`, top-level `src/<pkg>`.
3. **Else top-level directories**, excluding `node_modules`, `target`, `dist`,
   `build`, `vendor`, `.git`, dotdirs.

Cap at **~20 module pages**; beyond that, summarize the remainder on the
Architecture page and **list which modules were omitted** (this is the
budget-degradation mitigation in §5). For agentum this yields exactly the
`crates/*` map the spec wants (Cargo.toml `[workspace].members` is ground truth).

> **Recommendation:** put the ladder *in the prompt* and have the agent read
> `Cargo.toml`/`package.json` itself. **Why:** the server has no recursive walker
> (`fs.rs`), and the agent's own reads beat any server-side enumeration we'd build.

### (b) Active-repo plumbing — the "highest-risk unknown" is actually fully precedented

The chain is concrete and already exists end-to-end:

- The desktop holds `activeWorktreeId` in the store (`App.tsx:323`,
  `const activeWorktreeId = useAppStore((s) => s.activeWorktreeId)`).
- **The workdir is encoded *in the id*:** `Worktree.id` is `` `${repoId}::${path}` ``
  (`shared/types.ts:227`). Derive the absolute path with the existing helper
  **`splitWorktreeIdForFilesystem(activeWorktreeId)?.worktreePath`**
  (`shared/worktree-id.ts:31`) — use the `*ForFilesystem` variant, not
  `splitWorktreeId` (`:20`), because folder-project ids carry a UUID suffix the
  filesystem variant strips.
- The route receives the workdir **in the request** and resolves it with the
  shared helper every workdir-taking route uses:
  **`super::util::expand_workdir(&req.workdir)`** (`util.rs:19`, `pub(crate)`;
  handles `~`, trims, rejects empty).

This is a verbatim copy of the harness register route: `POST /api/harness` takes
`StartRequest { workdir: String }` → `expand_workdir` → returns
`StartResponse { harness_id }` (`harness.rs:39-63`). **No server-side
worktree-registry lookup is needed.** Worktrees live in a JSON registry at
`~/.agentum/worktrees.json` (`worktrees.rs:65-80`), but you never touch it — the
client passes the path.

> **Recommendation:** `POST /api/wiki/generate` takes `{ workdir: string }`; the
> GET routes take `?workdir=`. The desktop derives it via
> `splitWorktreeIdForFilesystem`. **Why:** mirrors the established workdir-route
> contract exactly; zero new resolution logic.

### (c) `.agentum/wiki/` commit policy

The repo `.gitignore` ignores `.agentum-uploads/` (`.gitignore:32`) but **not**
`.agentum/` — so without action the wiki is committable by default. ⚠️ The spec's
"ensure it's `.gitignore`-able" is not free; you must write the ignore.

> **Recommendation:** **git-ignored by default, enforced by a self-contained
> `.agentum/.gitignore`** containing `wiki/`, written idempotently by the generate
> route (not by editing the user's root `.gitignore`). **Why:** keeps the wiki
> app-local like `.agentum-uploads/`, never mutates the user's tracked
> `.gitignore`, and a Phase-2 "commit wiki" affordance just removes/overrides that
> one line. Ignoring only `wiki/` (not all of `.agentum/`) leaves room for future
> committed app state.

### (d) Regeneration semantics

**Full-replace for v1, confirmed.** The precedent is the QA recipe's "clear stale
then write" (`drive.rs:496`, `remove_file(&verdict_abs)` before the run).

> **Recommendation:** at generate **start**, clear `.agentum/wiki/` (remove dir
> contents, recreate) and write a `.status.json {state:"running"}`; the agent
> writes fresh pages + `index.json`. **Why:** simplest, matches the QA recipe, and
> stale pages from a prior run can't linger. **Hardening option (note for the
> developer, not required v1):** have the agent write to `.agentum/wiki.tmp/`,
> validate, then atomic-`rename` over `.agentum/wiki/` — this makes a *failed
> regen* non-destructive (old wiki survives). Adopt only if "a failed regen wipes
> the previous wiki" is judged unacceptable.

---

## 3. Concrete artifacts

### 3.1 `.agentum/wiki/` on-disk contract

```
<workdir>/.agentum/
  .gitignore            # contains "wiki/"  (written by generate, idempotent)
  wiki/
    index.json          # the TOC + schema version
    overview.md         # required
    architecture.md     # required, contains one ```mermaid block
    <module-slug>.md    # one per enumerated module
    .status.json        # present only while running / on failure
```

**`index.json` (the agent's machine-contract — keep it minimal so the agent can't
get it wrong):**
```jsonc
{
  "schemaVersion": 1,
  "pages": [
    { "slug": "overview",      "title": "Overview" },
    { "slug": "architecture",  "title": "Architecture" },
    { "slug": "agentum-server", "title": "agentum-server" }
  ]
}
```
- **Ordering = array order** (no separate `order` field — one less thing the agent
  can corrupt). `overview` first by convention.
- **`generatedAt` is stamped by the route on readback**, not written by the agent
  (more reliable than trusting the agent's clock).
- **Page-file naming:** `<slug>.md` where `slug` matches `^[a-z0-9][a-z0-9-]*$`
  (lowercase, hyphenated). The route **rejects any slug outside this set**
  (path-traversal guard — reuse the `sanitize` idea from `helpers.rs:170`).

**`.status.json`** (mirrors the QA verdict-file pattern — a deterministic readback
the route trusts):
```jsonc
{ "state": "running" | "failed", "sessionId": "<uuid>", "startedAt": 0, "error": "..." }
```

### 3.2 `routes/wiki.rs`

Template = `notes.rs:13`. Three handlers, registered in `lib.rs` next to `notes`
(anywhere in the `.merge(...)` block 258-297 — the `auth::require_token` layer at
298-301 covers it; **do not** touch `is_public`).

**Wire structs** (serde `rename_all = "camelCase"` so on-disk == wire, like
`worktrees.rs:47`):
```rust
struct GenerateRequest { workdir: String }
struct GenerateResponse { session_id: Uuid }            // job model — see below
struct WikiPageMeta { slug: String, title: String }
// GET /api/wiki returns one of three states:
enum WikiIndexResponse {
    Ready  { schema_version: u32, generated_at: u64, pages: Vec<WikiPageMeta> },
    Running { session_id: Uuid },
    Failed { error: String },
    Empty,                                              // never generated
}
```

**Handlers:**

| Route | Signature sketch | Body |
|---|---|---|
| `GET /api/wiki?workdir=` | `Query<WorkdirQuery>` → `Json<WikiIndexResponse>` | `expand_workdir` → if `index.json` parses valid ⇒ `Ready`; else read `.status.json` ⇒ `Running`/`Failed`; else `Empty` |
| `GET /api/wiki/{slug}?workdir=` | `Path<String>, Query<WorkdirQuery>` → `Json<{ content: String }>` (or `getText`) | validate slug regex → read `<wiki_dir>/<slug>.md` → 404 if absent |
| `POST /api/wiki/generate` | `Json<GenerateRequest>` → `Json<GenerateResponse>` | the generate flow ↓ |

**Generate flow (the `run_qa_agent_gate` recipe, transposed):**
1. `workdir = expand_workdir(&req.workdir)?`; `if !workdir.is_dir() { BadRequest }`.
2. Compute `wiki_dir = workdir.join(".agentum").join("wiki")`. **Clear stale**
   (remove + recreate) and write `.agentum/.gitignore` + `.status.json{running}` —
   mirrors `drive.rs:490-496`.
3. Build `Session` (`NewSession { workdir, tool, model, flags: vec![YOLO_MARKER], .. }`)
   + `create_session_on_host(.., LOCAL_HOST_ID)` — copy the ~30-line shape from
   `spawn_qa_agent` (`drive.rs:423-461`). **YOLO is mandatory** (autonomy invariant).
4. `spawn_agent_into_pane(state, &session, &host, &target, &workdir)` — **the one
   launch path** (`provision.rs:91`).
5. **Spawn a background task** (`tokio::spawn`) that runs
   `inject_prompt → wait_for_settle → teardown_session → read_back`:
   - `inject_prompt(state, &session, &prompt)` (`drive.rs:903`, `pub(crate)` ✓ —
     does the trust-dialog + two-step submit).
   - `wait_for_settle(&state.bus, session.id, grace, timeout)` (`drive.rs:956`).
   - `teardown_session(state, &session)` (`drive.rs:931`).
   - Read `index.json`; **valid ⇒ stamp `generatedAt`, delete `.status.json`;
     missing/garbled ⇒ write `.status.json{failed,error}`** (the AC-9
     "inconclusive ≠ success" gate, exactly like `parse_qa_verdict` at
     `helpers.rs:132`).
6. Return `GenerateResponse { session_id }` **immediately**.

> **Blocking vs job-id — Recommendation: return a job id (the `session_id`).**
> **Why:** AC-3 requires the run be "observable/streamable like any other session" —
> returning `session_id` lets the desktop open the existing session WS
> (`/api/sessions/{id}/stream`) and show the pane live; it also avoids a
> minutes-long open HTTP request. This matches the harness `run` route (spawns
> `drive` as a background task, returns immediately). The *blocking*
> `run_qa_agent_gate` fn is the recipe for the **background task body**, not the
> HTTP handler. The UI learns completion from the global `/api/events` bus
> (`agent.finished` for `session_id`, which the desktop already consumes) and
> re-fetches `GET /api/wiki`.

### 3.3 Generation prompt contract

⚠️-A **The spec lists `gather_repo_context` as "reuse — do NOT rebuild" but it is a
private `fn` (`chat.rs:207`), and Chat inlines it *because Chat has no filesystem*.
A spawned wiki agent sits in the workdir with YOLO and reads the repo itself — so
the snapshot is a *seed*, not the mechanism.**

> **Recommendation:** widen `gather_repo_context` to `pub(crate)` (one-word change)
> and **prepend its output as a "starter map"** in the prompt, then instruct the
> agent to **read further on disk** for anything truncated. **Why:** honors AC-3
> verbatim ("grounded by `gather_repo_context`"), gives a fast deterministic
> enumeration anchor, *and* the agent's own file reads cover what the 90k-char cap
> (`CONTEXT_BUDGET = 90_000`, `chat.rs:133`; tree capped at `TREE_MAX_FILES =
> 1_500`, `:142`) drops — directly defusing the big-repo degradation risk.

Prompt skeleton (model on `build_qa_prompt`, `helpers.rs:141` — explicit "write
exactly these files, don't stop, don't ask the human"):
- **Identity/role:** "You are the AutoWiki generator. Produce a navigable wiki for
  THIS repo at `<workdir>`."
- **Starter map:** the `gather_repo_context(workdir)` snapshot, framed as "a static
  seed — read more files as needed."
- **Output dir (absolute):** `<workdir>/.agentum/wiki/`.
- **Required pages:** `overview.md`, `architecture.md` (**must contain one
  ` ```mermaid ` block** — graph of the modules), one `<slug>.md` per module
  (enumeration ladder from §2a).
- **Index contract:** "write `index.json` exactly as `{schemaVersion:1,
  pages:[{slug,title},…]}`, `overview` first."
- **Internal links:** use `[[Page Title]]` where the title matches another page's
  `title` (this is what the MarkdownPreview doc-link resolver keys on — §3.4).
- **Termination:** "Do not stop until `index.json` and every page exist. Do not ask
  the human anything." (verbatim spirit of `helpers.rs:159-160`).

### 3.4 Desktop `WikiPage`

⚠️-D **The spec worries `MarkdownPreview` is "coupled to the editor/tab machinery"
and may need "a thin extraction." Investigation says: reuse it as-is — no
extraction.** Evidence:
- `MarkdownPreview` is `export default function` (`MarkdownPreview.tsx:428`) with
  **only 3 required props**: `content`, `filePath`, `scrollCacheKey` (`:93-99`).
  Everything else is optional.
- Its heavy imports (`useAppStore` reads at `:463-534`, `@/tauri` `api`, runtime
  clients) are **global-store selectors + interaction-only action callbacks** — the
  Zustand store is app-global and always mounted, so the component renders fine
  outside the editor. The store actions (`openFile`, `activateMarkdownLink`) only
  fire on specific link clicks, never on mount.

**Mount it standalone with:**
```tsx
<MarkdownPreview
  content={activePageMarkdown}
  filePath={`${workdir}/.agentum/wiki/${activeSlug}.md`}   // real path → link base + scroll cache
  scrollCacheKey={`wiki:${activeSlug}`}
  markdownDocuments={pagesAsMarkdownDocuments}              // enables [[link]] resolution
  onOpenDocument={(doc) => setActiveSlug(slugForDoc(doc))} // intra-wiki nav (AC-7)
/>
```
- **Internal-link nav (AC-7) routes through `onOpenDocument` + `markdownDocuments`**
  — the resolver is `createMarkdownDocumentIndex(markdownDocuments)` (`:548`) and the
  click handler at `:1176-1177` calls `onOpenDocument(resolvedDocument)`. Map each
  wiki page to a `MarkdownDocument` (`shared/types.ts:2734`): `{ filePath,
  relativePath, basename, name }`, with **`name` = the page `title`** so `[[Title]]`
  resolves. **This is the clean seam — do not hack the resolver.**
- The mermaid diagram (AC-6) renders for free: the `code` override at `:1462-1466`
  detects `language-mermaid` and renders `<MermaidBlock content … isDark …
  htmlLabels={false} />`. **No new render code.**

**Component shape:** a 2-pane view — left TOC (list from `index.json` `pages`, click
sets `activeSlug`), right `<MarkdownPreview/>`. Plus the **empty state** (AC-2:
explained + single "Generate wiki" button), a **running state** (stream the
`session_id`'s pane or just a spinner tied to `/api/events`), and a **failure
state** (AC-9: show `.status.json.error`, never a half-empty success).

**Data fetch — `runtime/wiki-client.ts`** (pattern = `runtime/harness-client.ts`
over the helpers in `runtime/server-http.ts`):
```ts
import { getJson, getText, postJson } from './server-http'
export const getWiki = (workdir: string) =>
  getJson<WikiIndexResponse>(`/api/wiki?workdir=${encodeURIComponent(workdir)}`)
export const getWikiPage = (workdir: string, slug: string) =>
  getText(`/api/wiki/${slug}?workdir=${encodeURIComponent(workdir)}`)
export const generateWiki = (workdir: string) =>
  postJson<{ sessionId: string }>('/api/wiki/generate', { workdir })
```
`server-http.ts` already does loopback URL resolution + bearer header
(`server-http.ts:11-48`); keep wire types faithful to the Rust structs (the harness
client's stated discipline).

### 3.5 The store/UI edits to add `'wiki'`

⚠️-C **The spec's "Add `'wiki'` to the `activeView` union" is 6 union edits + a new
field, not one line.** Precise insertion points:

1. **`ui.ts:440-445`** — add `'wiki'` to the **main union (`:440`) AND each of the
   5 `previousViewBefore*` unions (`:441-445`)**, and add a new field
   `previousViewBeforeWiki: …`. (The harness view did exactly this — every sibling
   union already lists `'harness'`.)
2. **`ui.ts` ~`:1033-1042`** — add `openWikiPage`/`closeWikiPage` mirroring
   `openHarnessPage`/`closeHarnessPage`.
3. **`App.tsx:221`** — `const WikiPage = lazy(() => import('./components/wiki/WikiPage'))`
   (next to `MissionControlPage`).
4. **`App.tsx` ~`:1755`** — add `{activeView === 'wiki' ? <WikiPage /> : null}` in
   the switch block (1749-1759).
5. **`SidebarNav.tsx` ~`:201`** — a `<PrimaryNavItem icon={…} label="Wiki"
   active={…} onClick={openWikiPage} />` after the Chat item (`:196-201`).

Also check: if persisted `activeView` is validated/reset anywhere on load, ensure
`'wiki'` is accepted (the harness precedent implies it's a passthrough; verify the
persist hydration doesn't whitelist).

---

## 4. Build order — 3 slices with crisp "done when"

| Slice | Builds | Done when (`verify.sh`) | Done when (`qa.sh`) |
|---|---|---|---|
| **1. `wiki-contract`** | `.agentum/wiki/` layout doc + `WikiIndex`/`WikiPageMeta` structs + `parse_wiki_index` (valid⇒ok, missing/garbled⇒err, mirrors `parse_qa_verdict`) + the prompt builder | `cargo test -p agentum-server --lib` green incl. a `parse_wiki_index` test (fixture parses; garbled fails) | n/a (no UI) |
| **2. `wiki-routes`** | `routes/wiki.rs` (GET list / GET page / POST generate), registered in `lib.rs`, authed; generate spawns via `spawn_agent_into_pane` + background settle/readback | `GET /api/wiki?workdir=<fixture>` round-trips a hand-written fixture index + a page's content; slug path-traversal rejected; `cargo test … --lib` green | n/a (optional `#[ignore]` live-agent test like `tests/harness_live_agent.rs`) |
| **3. `wiki-view`** | `WikiPage` (TOC + standalone `MarkdownPreview` + mermaid + `[[link]]` nav + empty/running/failure states) + the 6 store/UI edits + `wiki-client.ts` | `npm run build --prefix crates/agentum-desktop/ui` green | open Wiki → empty state → **Generate** → run visible → pages in TOC → select renders markdown → Architecture shows a **mermaid diagram** → an internal link navigates. Screenshot evidence per `browser-verification-loop`. |

---

## 5. Risks & boundary violations

- **One launch path (invariant #1).** Generate MUST go through
  `spawn_agent_into_pane` (`provision.rs:91`) — no bespoke `tmux`/argv. Copy the
  session-construction shape from `spawn_qa_agent` (`drive.rs:423-461`), including
  `flags: vec![YOLO_MARKER]`. A bespoke launch loses YOLO translation + loopback
  `pane_env` + MCP wiring.
- **Reuse seam — helper visibility (⚠️-B, the spec omits this).** The QA recipe is
  **not** all `pub(crate)`. Reusable as-is: `spawn_agent_into_pane`, `inject_prompt`
  (`drive.rs:903`). **Need widening to `pub(crate)`: `wait_for_settle`
  (`drive.rs:956`, currently `pub(super)`) and `teardown_session` (`drive.rs:931`,
  currently private).** Recommendation: widen both (one-word changes; they're
  pure-ish, well-tested). Alternative (defer): extract a generic
  `run_capture_agent(state, workdir, prompt, output_path)` in the harness module
  shared by QA + wiki — DRYer but touches the live gate path, so not for v1.
- **`gather_repo_context` is private (⚠️-A).** Widen to `pub(crate)` to seed the
  prompt (§3.3), or skip it and let the agent read the repo unaided. Either honors
  the spirit; widening honors AC-3's letter.
- **Inconclusive ≠ success (AC-9).** Missing/garbled `index.json` ⇒
  `.status.json{failed}` ⇒ the view shows an error, never a half-empty wiki. This
  is the `parse_qa_verdict` discipline (`helpers.rs:132`) — replicate it exactly.
- **Auth parity.** `/api/wiki` rides the global `require_token` layer
  (`lib.rs:298-301`); **do not** add it to `is_public` (`auth.rs:74`). Open on the
  loopback embedded server (which runs `no_auth`), token-gated on a networked
  daemon — automatic if you just `.merge` it.
- **Context-budget / big-repo degradation.** `CONTEXT_BUDGET = 90_000` chars /
  `TREE_MAX_FILES = 1_500` (`chat.rs:133/142`) hard-truncate; today it does **not**
  "log which modules were skipped" (the tree appends `…(+N more files)`, the rest is
  silently cut). Mitigation lives in the **prompt**, not in `gather_repo_context`:
  instruct the agent to read beyond the seed and **list omitted modules on the
  Architecture page** (§2a). This is the spec's risk item, made actionable.
- **Path traversal on the slug.** `GET /api/wiki/{slug}` must reject any slug
  outside `^[a-z0-9][a-z0-9-]*$` before joining it to `wiki_dir` (else
  `../../etc/passwd`). Reuse the `sanitize` approach (`helpers.rs:170`).
- **`MarkdownPreview` standalone (⚠️-D, resolved favorably).** Reuse as-is with the
  5 props in §3.4; route `[[link]]`s via `onOpenDocument`. The only real failure
  mode is passing a `filePath` that triggers a file-stat path — avoid by keeping
  links as `[[Title]]` (doc-link resolver), not `file://`/relative-path hrefs.
- **`.agentum/` is not ignored (⚠️, §2c).** Must write `.agentum/.gitignore` on
  generate; relying on the existing `.agentum-uploads/` ignore (`.gitignore:32`)
  would not cover it.
- **Folder-project ids carry a UUID suffix.** Use `splitWorktreeIdForFilesystem`
  (`worktree-id.ts:31`), not `splitWorktreeId`, to derive the on-disk workdir.

---

## Handoff to Developer (sdd-developer)

- **Completed (architecture):** citations validated; all 4 open questions resolved
  with code-grounded recommendations; artifact shapes, build order, and risks defined.
- **Pending (implementation):** the 3 slices in §4.
- **Key decisions:** job-model generate (return `session_id`); on-disk
  `.agentum/wiki/` + `.status.json`; reuse `MarkdownPreview` as-is; git-ignored-by-
  default via `.agentum/.gitignore`; full-replace regen.
- **Risks/required seams:** widen `wait_for_settle` + `teardown_session` (and
  optionally `gather_repo_context`) to `pub(crate)`; slug traversal guard; YOLO
  mandatory; AC-9 fail-loud via status file.
- **Spec corrections to fold in (⚠️ A-D):** `gather_repo_context` is private + is a
  *seed* not the mechanism; QA-recipe helpers aren't all reusable (visibility); the
  `activeView` change is 6 unions + a field; `MarkdownPreview` needs **no**
  extraction; `.agentum/` isn't currently ignored.
- **Recommended next step:** start slice 1 (`wiki-contract`) — the `index.json`
  parser + prompt builder are pure and unit-testable, and they pin the contract the
  other two slices depend on.

**Files of record (all in the `autowiki` worktree):** new —
`crates/agentum-server/src/routes/wiki.rs`,
`crates/agentum-desktop/ui/src/components/wiki/WikiPage.tsx`,
`crates/agentum-desktop/ui/src/runtime/wiki-client.ts`. Edited —
`crates/agentum-server/src/lib.rs` (~:277),
`crates/agentum-server/src/routes/chat.rs:207` + `harness/drive.rs:931/956`
(visibility), `crates/agentum-desktop/ui/src/store/slices/ui.ts` (:440-445,
:1033-1042), `crates/agentum-desktop/ui/src/App.tsx` (:221, :1755),
`crates/agentum-desktop/ui/src/components/sidebar/SidebarNav.tsx` (~:201).
