# Handoff 01 — PM → Architect

- **Spec:** 020-ssh-host-tracker-plumbing *(renumbered from 019 at PM gate:
  a shipped spec 019 — "slug resolution + Chat-from-anywhere" — already owns
  that number in code comments; no ai/specs/019 dir survives anywhere)*
- **Date:** 2026-07-13
- **From:** PM (autonomous /sdd-orchestrate; MCP server down — playbook from
  the session's verbatim copy)
- **To:** Architect
- **Artifact:** `ai/specs/020-ssh-host-tracker-plumbing/spec.md` (PM-gated;
  D1–D5 locked below)

## Gate result

PM gate: **PASS** (one-slice borderline, 3 increments — 015 precedent).
Keystones re-verified in-tree @ `3ec6f028`:

- `board_goals.rs:248-280` `resolve_github_slug(host, workdir, hint)` — the
  hint short-circuit is real (`:255-261`, zero I/O, malformed hint falls
  through — never errors), `git_in_dir` read at `:264`,
  `HostUnreachable`/`NoGithubRemote` split at `:267/:270`. Doc comment credits
  the OLD spec 019 — this helper and its semantics are the foundation, not
  work to redo.
- `github.rs:217-274` `create_issue` — `get_host(LOCAL_HOST_ID)` `:232-236`,
  BUT `body.slug` is ALREADY passed as the hint `:242` and the sink runs `gh`
  from `$HOME` with an explicit slug (`:261-274`; task_sink.rs's explicit-slug
  arm exists precisely because "workdir may not exist locally — the spec 019
  bug", `task_sink.rs:1125`).

## Material PM findings

1. **The hint is a live fast path TODAY.** `create_issue`/`list_labels`/
   `fetch_github_issue` already accept a slug and short-circuit on it — a
   client that knows the slug never triggers the local git read. The UI just
   has no way to LEARN the slug for an SSH repo (the binding GET dead-ends on
   the same hardcode, and the renderer index is local-native). So F2's slug
   seam + slug-first threading in the UI does much of F1's user-visible work;
   F1's `repoId` threading remains necessary for the no-hint robustness path
   and for the binding routes themselves.
2. **The old spec 019 already decoupled the sink from the filesystem**
   (`create_feature_for_goal` "decoupled from the local filesystem",
   `board_goals.rs:308`; SinkCtx.slug `task_sink.rs:56`). Reuse; zero sink
   changes expected.
3. **Research map is fresh and complete** (see the Spec-020 decision-log entry
   and spec Reuse section): 4 hardcode sites + the provision copy; binding
   store slug-keyed (`github_projects.rs:174-220`); `discover_binding`
   path-free; renderer index local-native (`commands/gh.rs:305-321`,
   `repo-slug-index.ts:77-90`); grounding local-by-design (`chat.rs:238`,
   `wiki_rag.rs:448`).

## Decisions locked (D1–D5)

- **D1 — wire identity = `repoId`** (optional, add-only on the
  `{workdir, slug?}` DTOs). Server resolves host via `load_host_for_repo`.
  Absent `repoId` = today's local behavior byte-for-byte; unknown = 4xx,
  never silent-local. `hostId` rejected (trusts the client twice).
- **D2 — F2 seam = a dedicated small host-aware slug route** (e.g.
  `GET /api/repos/{id}/slug`), behind `require_token`, fail-closed. Renderer
  uses it for `connectionId`-bearing repos; local repos may keep the native
  `gh_repo_slug` path.
- **D3 — no server-side slug cache this spec.** One git/SSH read per
  resolution; revisit only with evidence (015's ID_CACHE precedent if ever).
- **D4 — grounding note = server flag** in the draft response (the server
  knows `gather_repo_context`/wiki returned `None`); the client renders the
  honest note from the flag, never infers from `connectionId`.
- **D5 — old-019 machinery is foundation.** No changes to
  `resolve_github_slug`, `SlugReason`, the sink's explicit-slug arm, or the
  hint semantics. 020 is caller-side + one new route + UI threading.

## What to blueprint (F1 → F3)

1. **F1** — `repoId` on the family DTOs; swap the five pinned host sites
   (`github_projects.rs:63`, `github.rs:232/:102/:381`, `provision.rs:48`
   slug-half); decide whether to unify provision's admitted copy
   (`provision.rs:35-38`) into one shared resolver home; tests: hint
   short-circuit with unreadable workdir, repoId→host threading, unknown-repoId
   4xx, local no-repoId regression pins.
2. **F2** — the D2 route (shape + error contract: distinguish
   HostUnreachable vs NoGithubRemote on the wire); `repo-slug-index.ts` branch
   for SSH repos (keep exclusion-on-failure); pin Start-work e2e: SSH-only →
   `direct`, both-hosts → `choose` (015's classifier untouched).
3. **F3** — `repoId`+learned-slug threading through `use-tracker-intake.ts`
   legs and `ProjectBindingEditor`'s three feeders; the D4 grounded/ungrounded
   flag through `draft_issue_body` (`github.rs:311-335` → `chat.rs:1871-1903`)
   and the panel note; model tests (add-only to the 015 intake model).

## Open architect calls

- Exact DTO field names + which UI client layers to widen (two layers exist,
  runtime clients + hooks — mirror 015's add-only convention).
- Where the shared slug resolver lives if provision's copy is unified
  (`routes/util.rs` precedent per repo convention?).
- The F2 route's response shape (slug only vs `{slug, source}`), and whether
  the renderer caches it in the existing module-scope `slugByRepoId` map
  (`repo-slug-index.ts:33` — likely yes, keyed the same way).
- Coordination: 016–018 (sdd-tracker-status worktree) touch tracker surfaces;
  015 ships first (this builds on `3ec6f028`). Re-locate all UI line numbers.

## Expected architect artifact

`architecture.md` (per-increment design + build/test plan, ~250-400 lines)
then `handoffs/02-architect-to-developer.md`.
