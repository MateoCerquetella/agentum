# Deep verification: Project Hub board isolation

## Confirmed root causes

1. `openProjectHub` changed the active repo and TaskPage seed without
   synchronously invalidating the target repo's session binding. A stale or
   poisoned Freebee cache entry could therefore paint Agentum's identity/table
   before asynchronous revalidation completed.
2. Missing embedded cache entries were treated as verified-unbound instead of
   pending, conflating an unverified repo with a completed lookup.
3. The legacy `tracker` deep link rendered TaskPage but did not run the binding
   loader.
4. An unbound embedded repo was routed away from the Project surface instead
   of rendering its scoped picker.

## Fix

- Project Hub navigation now atomically sets `activeRepoId`, TaskPage's repo
  seed, and the target repo binding to `loading`, preserving other repo caches.
- Embedded readers treat missing binding entries as pending; standalone/global
  behavior retains its intentional legacy fallback.
- Both `tasks` and `tracker` run the repo binding lifecycle.
- Bound and unbound embedded repos stay on the Project surface; unbound renders
  the picker.

## Regression evidence

The component-level switch test renders Agentum's cached table, transitions
Freebee through pending, then verifies loaded/unbound Freebee renders only its
picker. At neither Freebee stage may Agentum's project identity or cached table
appear. A store test separately proves the navigation invalidation is atomic
and affects only the target repo.

Focused result: 20 passed, 0 failed. `git diff --check` passed.
