---
schema: 1
id: SPC-1QT7Z2JY0NAG21RKEK7P1X4Y3F
revision: 1
title: Tracker plumbing works for SSH-host repos (host-aware slug resolution)
source: legacy-import:ai/specs/020-ssh-host-tracker-plumbing/spec.md@sha256:01e7e6e6b4712e3460ea3556e0fbb3e5650b47c8c1a39bc0b7dc24523fd18561
---

# Tracker plumbing works for SSH-host repos (host-aware slug resolution)

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

> # Spec 020 — Tracker plumbing works for SSH-host repos (host-aware slug resolution)
>
> - **Number:** 020
> - **Status:** Done       <!-- Draft | PM | Architect | In progress | Done -->
> - **Surface:** `crates/agentum-server` (routes: github_projects, github, provision slug-half) + `crates/agentum-desktop/ui` (clients, binding editor, tracker intake, repo-slug index)
> - **Author:** Mateo (via /sdd-spec)
> - **Date:** 2026-07-13
>
> > **Numbering note:** 015 is triple-claimed, 016–018 are claimed by the
> > `sdd-tracker-status` worktree, and **019 is burned by already-shipped code**
> > ("slug resolution + Chat-from-anywhere" — the spec that made
> > `resolve_github_slug` host-aware; its `(spec 019)` doc comments live in
> > `board_goals.rs`/`task_sink.rs`/`forge.rs` though no `ai/specs/019` dir
> > survives in any worktree) — next free is **020**. Anchors verified on
> > `fixes-new-workspace` @ `3ec6f028` (= develop `4f98453f` + spec 015).
> > Discovered while QA-ing spec 015: the screenshot bug "no GitHub repo resolved
> > for this project — its folder has no `origin` remote pointing at GitHub" on
> > the dyaus project's Tracker tab.
>
> ## Problem
>
> For a repo that lives on an SSH host, the whole GitHub tracker surface
> dead-ends even though the repo has a perfectly good GitHub origin: the Tracker
> tab's binding editor shows "no GitHub repo resolved" (Mateo's screenshot, the
> dyaus `agentum` project driven from his Mac), the spec-015 intake panel can't
> file issues for it, and the board's Start-work can't even see the repo as a
> candidate. The repo's folder exists only on its host, but the server resolves
> the GitHub slug by running `git` on the **local** machine at that path.
>
> ## Goal
>
> Resolve a repo's GitHub slug on the repo's own host so every slug-only tracker
> leg (binding editor, issue create/fetch/labels, intake filing, Start-work
> matching) works identically for local and SSH repos — and the folder-reading
> legs (draft grounding) degrade honestly instead of silently.
>
> ## Users / personas
>
> - **Mateo (multi-host solo operator)** — desktop app on his Mac, project on the
>   `dyaus` VPS: opening Project Hub → Tracker to bind the board (today: red
>   `no_github_repo` error), typing an intent in the 015 intake panel (today:
>   filing 422s), and clicking Start-work on a board item whose repo is SSH-only
>   (today: false "Repository isn't added to Agentum").
>
> ## Acceptance criteria
>
> Ordered increments F1 → F3.
>
> **F1 — server: slug resolution runs on the repo's host**
>
> 1. The `{workdir, slug?}` route family accepts an optional **`repoId`** and,
>    when present, resolves the git host via `load_host_for_repo`
>    (`routes/repos.rs:410-417`) instead of the hardcoded
>    `get_host(LOCAL_HOST_ID)` — at all four pinned sites:
>    `github_projects.rs::resolve_slug:63-67`, `github.rs::create_issue:232-236`,
>    `github.rs::fetch_github_issue:102-106`, `github.rs::list_labels:381-385`
>    (plus the copy in `provision.rs::resolve_slug:48-52`, slug-half only).
>    `resolve_github_slug(&host, …)` (`board_goals.rs:248`) is already
>    host-parameterized — no change to it. An absent `repoId` keeps today's
>    local behavior byte-for-byte; an unknown `repoId` is a 4xx, never a silent
>    local fallback.
> 2. **Slug-hint short-circuit pinned:** a request carrying a valid `owner/repo`
>    slug hint performs **zero** git I/O regardless of host or workdir
>    (`resolve_github_slug`'s existing `:255-261` precedence) — asserted by a
>    unit test with an unreadable workdir + valid hint succeeding. This is the
>    cheap path the UI uses once it has learned the slug.
> 3. The binding routes (`GET/PUT/DELETE /api/github/project-binding`) for an
>    SSH repo (threaded `repoId`, GitHub origin on the remote) return/persist
>    the slug-keyed binding (store stays keyed by lowercase slug,
>    `github_projects.rs:174-220` — no path keys) — no spurious
>    `no_github_repo`. `HostUnreachable` stays distinguishable from
>    `NoGithubRemote` in the 422 message (the `:74-84` classification is kept).
> 4. Rust unit tests: repoId→host threading (local repo id resolves and works),
>    unknown-repoId 4xx, hint short-circuit (AC 2), and the existing local
>    no-repoId paths unchanged (regression pins on the current tests).
>
> **F2 — Start-work sees SSH repos (slug index host-aware)**
>
> 5. The renderer's slug index resolves an SSH repo's slug **via the server**
>    (a host-aware slug resolution seam — shape is the architect's call) instead
>    of the local-only native `gh_repo_slug` (`commands/gh.rs:305-321`, which
>    runs local `git -C <repo.path>` and returns null for a remote path,
>    excluding the repo at `repo-slug-index.ts:78-90`). Local repos may keep the
>    native path.
> 6. With an SSH-only repo registered (GitHub origin on the remote), the board's
>    Start-work classifies it `direct` (spec 015's `classifyStartWorkRepoMatches`
>    pin) and launches on that repo — no false "Repository isn't added to
>    Agentum". A repo on both hosts classifies `choose` exactly as 015 defines.
> 7. A repo whose slug cannot be resolved (no origin, host down) stays excluded
>    from the index exactly as today — fail-closed, no phantom matches.
>
> **F3 — intake panel: SSH filing works, grounding degrades honestly**
>
> 8. The 015 intake panel (`use-tracker-intake.ts`) threads `repoId` (and the
>    cached slug once learned, `:108`) through its binding read (`:104`) and
>    file (`:212-217`) legs, and the **learned slug** through the draft leg
>    (`:189-193`) — *(amended at architect grounding 2026-07-13: the draft route
>    resolves no slug and touches no host, so a `repoId` there would be a dead
>    wire field; slug-first threading + the AC 9 grounding flag carry the
>    intent)* — filing a GitHub issue for an SSH repo succeeds (create is
>    slug-only: `gh` runs from `$HOME` with `--repo`, `github.rs:261-274`).
> 9. Drafting for an SSH repo still returns a draft, and the panel renders an
>    honest grounding note when repo/wiki context was skipped (the server's
>    `gather_repo_context` returns `None` for a non-local dir, `chat.rs:238`;
>    wiki sidecar likewise, `wiki_rag.rs:448`) — e.g. "drafted without
>    repo/wiki grounding — repo lives on <host>". Never a silent generic draft
>    presented as grounded, never a hard error.
> 10. Errors stay inline and non-fatal (015 AC 12 contract preserved); `filed`
>     still only ever set from a provider-confirmed response.
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** repoId threading + host-aware slug resolution across the slug-only
>   route family (F1); a server-backed slug seam for the renderer index (F2);
>   intake panel threading + the honest grounding note (F3).
> - **Out:**
>   - **No host-aware folder reads**: `gather_repo_context` / wiki RAG stay
>     local-only ("Chat never SSHes", `chat.rs:232`) — SSH drafts are honestly
>     ungrounded, not remotely grounded.
>   - **No provision/scaffold for SSH repos**: `provision.rs:292`'s
>     `workdir.is_dir()` hard gate stays (only its *slug* resolver is fixed for
>     consistency); remote scaffolding is its own future spec.
>   - **No Linear changes**, no runtime-environment RPC changes
>     (`github.repoSlug` env branch untouched).
>   - **No binding-store re-keying** (slug-keyed stays).
>   - **No slug caching layer server-side** beyond what exists — one git call
>     per resolution is fine at this scale (see Open questions).
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - **`board_goals::resolve_github_slug(host, workdir, hint)`** (`:248-279`) —
>   already host-aware via `host_runtime::git_in_dir` (`git_fs.rs:44-78`), with
>   `SlugReason::{NoGithubRemote, HostUnreachable}` (`:224-232`) and the
>   zero-I/O hint short-circuit (`:255-261`). The fix is caller-side only.
> - **Host resolution:** `load_host_for_repo` / `resolve_repo_host_id`
>   (`routes/repos.rs:397-417`); the host-threading precedent is the goal-cards
>   route (`board_goals.rs:56-60,135-140` — client `host_id`, doc comment
>   "path→host has no reliable mapping").
> - **Neutral-cwd gh:** `gh_in_dir(host, $HOME, [... "--repo", slug])` already
>   makes create/fetch/labels slug-only (`github.rs:123-139,261-274,405-418`).
> - **Dual-entry disambiguation (spec 015):** `findRepoByPathPreferLocal`
>   (`lib/find-repo-by-path.ts:7-22`), `scope_pairs_locals_first`
>   (`repos.rs:84-99`) — but F1's contract is *explicit `repoId` beats path
>   guessing*; use these only where no id is available.
> - **UI clients to widen (add-only fields):** `runtime/github-projects-client.ts`
>   (`:122-146,153-196,288-312`), `runtime/github-issue-client.ts`
>   (`:26-58,115-151,189-221`), `ProjectBindingEditor` props (`:60-65`) + its
>   three feeders (ProjectHubPage `:275`, IntegrationsPane `:266`,
>   CreateWorkspaceWizard `:1609`), `use-tracker-intake.ts` legs.
> - **`discover_binding` needs nothing** (`github_projects.rs:258-293` —
>   owner/number only, already SSH-clean).
>
> ### Build new
>
> - **F1** — the optional `repoId` field on the family's request DTOs + the
>   host-resolution swap at the five pinned sites; unify `provision.rs`'s
>   hand-copied resolver with `github_projects.rs`'s (they are admitted
>   duplicates, `provision.rs:35-38`) if the architect finds a clean shared home.
> - **F2** — a host-aware slug seam callable from the renderer (e.g.
>   `GET /api/repos/{id}/slug` over `load_host_for_repo` +
>   `resolve_github_slug`; exact shape = architect) + the `repo-slug-index.ts`
>   branch that uses it for `connectionId`-bearing repos.
> - **F3** — `repoId`/slug threading in the intake hook + a grounding-note
>   signal (server tells the client grounding was skipped, or the client infers
>   from `repo.connectionId` — architect picks; must be honest, not heuristic
>   guesswork presented as fact) + pure-model test coverage.
>
> ## Risks & invariants
>
> - **Local behavior is sacred:** absent `repoId` must reproduce today's local
>   path byte-for-byte — every existing caller keeps working un-migrated.
> - **Explicit identity beats path guessing:** post-015 a bare path can match
>   two repos; anywhere both are possible, the threaded `repoId` wins and a
>   path-only fallback prefers local (never a silent remote pick).
> - **Fail-closed matching:** F2 must not create phantom Start-work matches —
>   unresolvable slug = excluded, exactly as today.
> - **Honesty over capability (F3):** an ungrounded draft must say so; a
>   spurious "grounded" presentation is worse than the current silence.
> - **No new auth surface:** any new route sits behind `require_token` like its
>   siblings; no `is_public` additions.
> - **`HostUnreachable` ≠ `NoGithubRemote`:** an SSH transport failure must not
>   masquerade as "no origin" (the classification exists — keep it on every new
>   path).
> - **Coordination:** specs 016–018 (sdd-tracker-status worktree) touch tracker
>   surfaces — re-ground line numbers at architect time; 015 is unreleased and
>   ships first (this spec builds on its commits).
>
> ## Harness wiring (the gate)
>
> - **feature_list.json entries (ordered):**
>   1. `host-aware-slug-family` — repoId threading + host swap + tests (AC 1–4).
>   2. `slug-index-ssh` — server slug seam + renderer branch + Start-work e2e
>      pins (AC 5–7).
>   3. `intake-ssh-honest` — intake threading + grounding note + model tests
>      (AC 8–10).
> - **`verify.sh` asserts:** `cargo test -p agentum-server --lib` green (hint
>   short-circuit, repoId→host threading, unknown-repoId 4xx, local-path
>   regression pins) AND `npm run build --prefix crates/agentum-desktop/ui`
>   AND targeted `bunx vitest run` green (client payload shapes, slug-index
>   classification with a server-resolved slug, intake grounding-note model).
>   Full vitest / bare tsc stay non-gates (pre-broken baselines).
> - **`qa.sh` asserts (browser QA, staging, from a desktop ≠ repo host):**
>   Project Hub → Tracker for the SSH repo binds a board (no `no_github_repo`);
>   the intake panel drafts (with the grounding note) and files a real GitHub
>   issue; board Start-work on the SSH-only repo launches directly; a host-down
>   SSH repo shows the unreachable-flavored error, not "no origin".
>
> ## Open questions
>
> - **Wire identity: `repoId` vs `hostId`?** *Default: `repoId`* — the server
>   owns path/host consistency (`load_host_for_repo`), mirrors worktrees.rs
>   convention, and survives host edits; `hostId` would trust the client twice.
> - **F2 seam shape:** dedicated `GET /api/repos/{id}/slug` vs widening an
>   existing route. *Default:* dedicated tiny route (slug resolution is a
>   distinct capability the index needs standalone).
> - **Server-side slug cache?** The index calls once per repo per rebuild; SSH
>   round-trips are ~100ms. *Default:* no cache in this spec; add only if the
>   index rebuild proves noisy (015's ID_CACHE precedent exists if needed).
> - **Grounding-note transport (F3):** explicit flag in the draft response vs
>   client-side inference from `connectionId`. *Default:* server flag — the
>   server knows whether `gather_repo_context` returned `None`; the client
>   should not guess.
