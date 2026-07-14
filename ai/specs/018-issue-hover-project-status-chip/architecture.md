# Spec 018 — Architecture Blueprint: Issue hover card Project-status chip

**Self-check passed.** Every load-bearing cite re-verified line-by-line on this
worktree (`issue-hover-card-…`, fast-forwarded to develop `d31314b3`,
2026-07-14). **Pre-design collision sweep ran clean**: no `gh_issue_project_status`
command exists (`grep` over `crates/agentum-desktop/src` + `ui/src/tauri`), no
Project-status chip exists in `WorktreeCardMeta.tsx` (only `IssueStateBadge` +
`TrackerPhaseChip`), and no single-issue Status read exists in
`gh_projects.rs` (only `gh_get_project_view_table`, whole-table). Nothing here
was already built.

- **Status:** Architect → ready for Developer.
- **Order:** one slice, built bottom-up in four commits (see §5): Rust command →
  Tauri client wiring → pure UI model → chip + props threading. Each compiles
  and is independently reviewable; the chip renders only after all four land.

---

## 0. TL;DR — one slice, one sentence

On hover-card open, derive `{owner, repo, number}` from `issue.url`, read the
repo's Projects v2 binding (existing `getProjectBinding`, cached per slug), and
— when bound — call a new thin desktop Tauri command `gh_issue_project_status`
(one `gh api graphql` read for the issue's Status option on that project,
cached per issue) to render a small chip beside `IssueStateBadge`; every miss,
unbound repo, or error resolves to **no chip** (silent absence).

---

## 1. Open questions — resolved

### D1. Read path: **desktop Tauri command** (not a server route)

The spec's open Q1 (carried from 016 Q2 / 358b Q1). **Decision: a new desktop
Tauri command `gh_issue_project_status` beside `gh_get_project_view_table`.**

- `gh` auth already lives desktop-side — every `gh_projects.rs` command shells
  local `gh api graphql` via the injection-safe `graphql()` runner
  (`crates/agentum-desktop/src/commands/gh_projects.rs:136`).
- The hover popover is a desktop-only surface; there is no TUI consumer, so a
  server route would add a network hop and an auth-header concern
  (`/mcp`-style bearer) for zero reuse.
- The server route only wins if the chip must render for a project the **local**
  `gh` token cannot see. That is not a requirement — AC 2 makes an
  unreadable/absent status a silent no-chip, so "local token can't see it" is
  already the acceptable degraded case.

### D2. Binding source: **fresh `getProjectBinding`, cached per slug for the app session**

The spec's open Q2. **Decision: the chip's hook calls the existing
`getProjectBinding` client (`ui/src/runtime/github-projects-client.ts:144`)
and caches the result per repo slug in an app-session cache; it does NOT reuse
Project Hub store state.**

- Project Hub binding state is only populated when that page has been opened —
  unreliable for a sidebar that renders on boot. A miss there would need a
  fallback fetch anyway, so a single owned fetch is simpler.
- `getProjectBinding` is cheap: with a valid `slug` hint the server's
  `resolve_github_slug` short-circuits with **zero git I/O** (util.rs:108
  comment — "A valid hint still performs zero git I/O"), reading only the JSON
  registry + host row, then `binding_for_slug(&slug)`.
- Cache lifetime = app session (module-level `Map`), keyed by slug. AC 3's
  "cached per issue" is the *status* cache; the binding cache is per repo (a
  strict superset of the AC — one repo has many issues).

---

## 2. Data flow (the whole feature)

```
hover card opens (HoverCard `open` → true, WorktreeCardMeta.tsx:254)
  │
  ├─ parseIssueRef(issue.url)  ──►  null  ──►  no chip           (pure)
  │        │ {owner, repo, number, slug}
  │        ▼
  ├─ bindingCache.get(slug)  ── hit ─►  use it
  │        │ miss
  │        ▼
  │   getProjectBinding({ workdir, slug, repoId })   (existing server route)
  │        │  null / error  ──►  cache null  ──►  no chip
  │        │  BindingDto { projectId, statusFieldId, projectOwner, … }
  │        ▼  cache it
  ├─ statusCache.get(`${slug}#${number}`)  ── hit ─►  use it
  │        │ miss
  │        ▼
  │   gh.issueProjectStatus({ owner, repo, number,               (NEW command)
  │                           projectId, statusFieldId })
  │        │  { status: null } / error  ──►  cache null  ──►  no chip
  │        │  { status: "In Progress" }
  │        ▼  cache it
  └─ render <IssueProjectStatusChip status="In Progress" />
```

Nothing fetches until a card is opened (AC 3: no fetch for never-hovered
cards). A second open of the same issue hits both caches → no network (AC 3).
Every `null` / `error` edge collapses to no chip (AC 2).

---

## 3. Build new (three units)

### 3a. Rust — `gh_issue_project_status` (desktop command)

New command in `crates/agentum-desktop/src/commands/gh_projects.rs`, one
`graphql()` call, reusing the file's `Scalar` / `classify_*` / error-envelope
machinery verbatim.

**Args** (camelCase over the Tauri wire, `#[serde(rename_all = "camelCase")]`):
`owner: String, repo: String, number: i64, project_id: String,
status_field_id: String`.

**Query** (owner/repo/number as bound `$vars` — never interpolated, per the
`graphql()` injection contract; `number` via `Scalar::Int` so `$number:Int!`
binds numeric):

```graphql
query($owner:String!, $repo:String!, $number:Int!) {
  repository(owner:$owner, name:$repo) {
    issue(number:$number) {
      projectItems(first:20) {
        nodes {
          project { id }
          fieldValues(first:50) {
            nodes {
              ... on ProjectV2ItemFieldSingleSelectValue {
                name
                field { ... on ProjectV2SingleSelectField { id } }
              }
            }
          }
        }
      }
    }
  }
}
```

**Mapping** — extract a **pure** `fn issue_project_status(data: &Value,
project_id: &str, status_field_id: &str) -> Option<String>` (unit-testable
without the crate's Tauri deps): walk `repository.issue.projectItems.nodes`,
pick the node whose `project.id == project_id`, then within its
`fieldValues.nodes` pick the single-select value whose `field.id ==
status_field_id`, return its `name`. Any missing hop → `None`.

**Return shape:** `{ status: Option<String> }` on success; on `graphql()` /
`gh` error return the existing `ProjectError::envelope()` (`{ error: {kind,
message,…} }`). The renderer treats **both** `{status:null}` and any error
envelope as "no chip" — see D6, so the Rust side never needs to distinguish
"not on project" from "fetch failed".

Register in `crates/agentum-desktop/src/lib.rs` `invoke_handler!` beside
`gh_projects::gh_get_project_view_table` (:511).

### 3b. TS — Tauri client + contract

- `ui/src/tauri/gh.ts`: add `issueProjectStatus: (...args) =>
  call('gh_issue_project_status', args)` (alphabetical, matches the
  auto-generated style).
- `ui/src/tauri/contract.ts`: add the method to the `gh` namespace type so
  `satisfies AgentumApi['gh']` still holds.

### 3c. TS — pure model `lib/issue-project-status.ts` (the vitest target)

Pure, dependency-injected (mirrors `lib/tracker-phase.ts`):

- `parseIssueRef(url: string | undefined): { owner, repo, number, slug } | null`
  — parse `https://github.com/<owner>/<repo>/issues/<n>`; anything else → null.
- `statusCacheKey(slug: string, number: number): string` → `` `${slug}#${number}` ``.
- `resolveIssueProjectStatus(input, deps): Promise<string | null>` — the
  orchestration: binding-cache lookup → `deps.getBinding` → status-cache lookup
  → `deps.getStatus`; writes both caches; **never throws** (every rejection is
  caught → null). Caches passed in as `Map`s so tests assert hit/miss and
  no-double-fetch behaviour deterministically.

The React hook `useIssueProjectStatus({ open, issueUrl, workdir, repoId })`
(thin, in the component file or `hooks/`) owns the two module-level `Map`
caches and calls `resolveIssueProjectStatus` in an effect gated on `open &&
!cached`. Returns `string | null`.

### 3d. TSX — the chip + props threading

- New `IssueProjectStatusChip` (small, beside `TrackerPhaseChip` in
  `WorktreeCardMeta.tsx`, or its own file) — a `Badge` styled as a sibling of
  `IssueStateBadge`, visually distinct (e.g. a subtle "board" affordance /
  differing variant) per AC 1. Renders `null` when `status == null`.
- Slot it into the badges row (`WorktreeCardMeta.tsx:314–321`), inside the
  `issue &&` block, next to `IssueStateBadge` (:316).
- **Props threading:** `WorktreeCardDetailsHoverProps` gains `workdir?: string`
  and `repoId?: string`; `WorktreeCard.tsx` passes `workdir={repo.path}` and
  `repoId={repo.id}` at the two `<WorktreeCardDetailsHover>` call sites
  (:586, :602) — both already in scope (`repo.path`/`repo.id` used at
  WorktreeCard.tsx:358). `issue.url` supplies owner/repo/number; `workdir` +
  `repoId` + parsed `slug` feed `getProjectBinding`.

---

## 4. Reuse — do NOT rebuild

| Need | Reuse | Cite |
| ---- | ----- | ---- |
| GraphQL exec (injection-safe, gh-auth) | `graphql()` + `Scalar` + `classify_*` + `ProjectError::envelope` | `gh_projects.rs:136,127,66,98,54` |
| Binding read (server, host-aware) | `getProjectBinding` → `GET /api/github/project-binding` → `get_binding` → `binding_for_slug` | `github-projects-client.ts:144`, `github_projects.rs:273` |
| Binding fields (projectId, statusFieldId, owner, number) | `BindingDto` | `github_projects.rs:137` |
| Hover open-state trigger | `HoverCard open/onOpenChange` | `WorktreeCardMeta.tsx:254` |
| Badge look | `IssueStateBadge` / `LinearStateBadge` | `WorktreeCardMetadataStatusBadges.tsx` |
| Pure-model + hook precedent | `lib/tracker-phase.ts` + `TrackerPhaseChip` | `sidebar/TrackerPhaseChip.tsx` |
| Command registration | `invoke_handler!` list | `lib.rs:511` |

---

## 5. Commit plan (developer)

1. **Rust command** — `gh_issue_project_status` + pure `issue_project_status`
   mapper + `#[cfg(test)] mod` cases (found / not-on-project / missing-field /
   empty) + `lib.rs` registration.
2. **Tauri client** — `gh.ts` + `contract.ts` method.
3. **Pure model** — `lib/issue-project-status.ts` + `issue-project-status.test.ts`
   (parse variants, cache hit/miss, no-double-fetch, error→null).
4. **Chip + threading** — `IssueProjectStatusChip`, hook, badges-row slot,
   `WorktreeCard` props, `WorktreeCardMeta.test.tsx` presence/absence cases.

---

## 6. Risks / invariants (protect these)

- **Silent absence (AC 2) — D6:** the chip hook must **never throw** into the
  badges row (a throw takes the whole hover down). Every fetch rejection and
  unexpected-shape payload → `null` → no chip. Encode this in
  `resolveIssueProjectStatus` (try/catch → null) and in the chip's
  `status == null → return null`.
- **No poll (AC 3):** fetch only in the `open`-gated effect + module caches.
  Do NOT subscribe to `/api/events` or add an interval — that's
  `TrackerPhaseChip`'s job (live phase), deliberately separate here (snapshot
  at open). Non-goal in the spec.
- **Crate boundaries:** `agentum-server` is untouched — the binding read reuses
  the existing route; the new GraphQL read is desktop-side where `gh` auth
  lives (every `gh_projects.rs` command precedent).
- **GraphQL injection:** owner/repo/number stay `$vars` (the `graphql()`
  contract, gh_projects.rs:133 comment). Do not `format!` them into the query.
- **SSH repos (spec 020):** pass `repoId` to `getProjectBinding` so a bound SSH
  repo resolves its binding on its own host. The Status GraphQL read runs on
  local `gh` — if the local token can't see the project, that's a fetch error
  → silent absence (documented degrade, acceptable per AC 2).

---

## 7. ⚠️ Environment constraint for Developer + Tester (build gate)

**This dev env has no `webkitgtk`, so `cargo build/test -p agentum-desktop`
cannot compile locally** (see project memory "agentum dev env constraints").
Consequences for the gate:

- The Rust unit tests on `issue_project_status` (§3a) are authored but run in
  **CI / a webkitgtk machine**, not locally. Structure the mapper as a pure
  `fn(&Value, &str, &str) -> Option<String>` so its `#[cfg(test)]` cases are
  standalone (no Tauri runtime) and CI-runnable.
- Local Developer/Tester verification covers the **UI side**: `bun run build`
  (`crates/agentum-desktop/ui`) + targeted `bunx vitest run` on
  `issue-project-status.test.ts` and `WorktreeCardMeta.test.tsx`. The full
  vitest suite + full `tsc` are a **known pre-broken baseline** on develop
  (project memory "agentum UI test toolchain") — the gate pins the two
  targeted files, not the whole suite.
- `verify.sh` therefore asserts: UI `bun run build` green + the two targeted
  vitest files green locally; `cargo test -p agentum-desktop --lib`
  (incl. the mapper cases) + `cargo fmt --check` gated in CI. Flag any local
  cargo attempt's webkitgtk failure as environmental, not a code defect.

---

## 8. Handoff

To Developer: build §5's four commits in order; honor D6 (never throw) and the
§7 build-gate reality. Handoff note: `handoffs/02-architect-to-developer.md`.
