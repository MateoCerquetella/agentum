- phase: entered authoring (from executing)
- PM gate (2026-07-13): spec 358 bundled two unrelated slices (SDD-loop MCP
  check-in + issue-hover Project-status chip) — goal had a literal "and",
  zero shared code path or verification surface. Split at the gate: spec 358
  narrowed to the SDD-loop slice (5 testable criteria, non-goals added,
  persona = Mateo mid-run); the chip rider moved to spec
  358b-issue-hover-project-status-chip, pending its own PM gate.
- authoring gate PASS (attempt 2): Narrowed to one slice at the gate: spec 358 = SDD loop stops on agentum_sdd_loop check-in (5 testable criteria, non-goals + persona added, grounded in routes/sdd.rs on develop); the unrelated issue-hover Project-status chip rider was split out to spec 358b (pending its own PM gate).
- phase: entered architecture (from authoring)
- architecture gate PASS (attempt 1): Plan grounded line-by-line at origin/develop tip (253173ad; all spec citations re-verified): 3 files in agentum-server — agentum_sdd_loop tool as thin view over a new agent_checkin seam reusing the toggle-off stop path, prompt-carried generation token for staleness, STATE.md belt before every injection, and a test-demanded deliver seam so all four AC tests run without tmux; sacred inject/settle mechanics and DEFAULT_MAX_STEPS untouched. Open question pinned: dedicated tool, not a report_status op. One flagged additive deviation: optional `generation` tool field, required by the spec's own stale-generation constraint.
- phase: entered decompose (from architecture)
- phase: entered executing (from decompose)
