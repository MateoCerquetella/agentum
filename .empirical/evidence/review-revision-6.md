# Independent review — revision 6

Reviewed the accepted decisions D-001 through D-003, the complete repository
diff, all new fixture/evidence files, and each acceptance criterion. Review
focused on filesystem confinement, time-of-check races, runtime/private-data
exclusion, revision completeness, mutation ordering, source-union closure,
remote fail-closed behavior, UI capability gating, and regression boundaries.

## Findings resolved during review

1. The derivative fixture originally linked to, but did not carry, the upstream
   MIT license. The exact pinned license bytes are now included and hash-bound
   at SHA-256 `1794687c291852aec63afe055288ff3d59742d091aaf35dad3e5ad5750429121`.
2. The golden no-runtime-dependency check now rejects whitespace-obfuscated
   `Command::new` and program declarations with regular expressions.
3. The empty-plan diagnostic no longer implies that ignored prose is imported.
4. Tests now explicitly prove sorted multiple capabilities and all required
   route preview fields, including bounded diagnostics and normalized Markdown.

## Review conclusions

- The importer accepts only `.empirical/specs/<safe-feature>`, traverses held
  no-follow directory handles, double-snapshots imported content, compares the
  reopened directory identity, and rejects locks, links, replacement races,
  unknown shapes, invalid schemas/text, and bounded-resource violations.
- Revision material includes config schema dependency, stable state identity,
  and every imported contract artifact. Volatile state, events, decisions, and
  evidence neither leak into normalized output nor destabilize the revision.
- Preview is pure. Creation reruns normalization and compares the expected
  revision before IDs, workspace materialization, provider attempts, approvals,
  runs, or aggregate persistence are allocated.
- The closed API union, stored immutable import, remote rejection, UI source
  model, capability gate, payload, visible preview, and revision-bound create
  path are exhaustive for Empirical.
- Agentum remains the only execution, approval, evidence, review, and delivery
  authority. No Empirical/Node process, package, MCP server, skill installation,
  mutation, remote adapter, or export path was introduced.
- Existing source and lifecycle paths remain intact. Server all-target clippy,
  20 source tests, 8 Empirical tests, 28 repository policy tests, the full
  6,472-test UI suite, typecheck, production build, boundary check, and Chromium
  evidence are green.

No unresolved review findings remain.
