#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

# The replacement has no compatibility window. Immutable database migrations,
# the one-shot migration utility, and its recovery report are deliberately not
# listed: they account for retired state but are not runtime readers.
retired_paths=(
  crates/agentum-server/src/harness.rs
  crates/agentum-server/src/harness
  crates/agentum-server/src/harness_roles
  crates/agentum-server/src/routes/harness.rs
  crates/agentum-server/src/routes/sdd.rs
  crates/agentum-server/src/sdd.rs
  crates/agentum-server/src/sdd_playbooks
  crates/agentum-server/tests/harness_live_agent.rs
  crates/agentum-server/tests/harness_mcp_e2e.rs
  crates/agentum-server/tests/harness_start_work_live.rs
  crates/agentum-server/tests/harness_start_work_live_roles.rs
  crates/agentum-desktop/ui/src/components/HarnessSpecBanner.tsx
  crates/agentum-desktop/ui/src/components/HarnessSpecBanner.test.tsx
  crates/agentum-desktop/ui/src/components/gated-run
  crates/agentum-desktop/ui/src/components/harness
  crates/agentum-desktop/ui/src/components/sdd/SddBar.tsx
  crates/agentum-desktop/ui/src/components/sdd/SddBar.identity.test.ts
  crates/agentum-desktop/ui/src/hooks/useWorktreeHarnessRun.ts
  crates/agentum-desktop/ui/src/hooks/useWorktreeHarnessRun.test.ts
  crates/agentum-desktop/ui/src/lib/gated-run-ownership.ts
  crates/agentum-desktop/ui/src/lib/gated-run-ownership.test.ts
  crates/agentum-desktop/ui/src/lib/harness-run.ts
  crates/agentum-desktop/ui/src/lib/harness-run.test.ts
  crates/agentum-desktop/ui/src/lib/start-gated-run-precondition.ts
  crates/agentum-desktop/ui/src/lib/start-gated-run-precondition.test.ts
  crates/agentum-desktop/ui/src/lib/workspace-harness-detect.ts
  crates/agentum-desktop/ui/src/lib/workspace-harness-detect.test.ts
  crates/agentum-desktop/ui/src/lib/workspace-harness-offer.ts
  crates/agentum-desktop/ui/src/lib/workspace-harness-offer.test.ts
  crates/agentum-desktop/ui/src/runtime/harness-client.ts
  crates/agentum-desktop/ui/src/runtime/harness-client.test.ts
  crates/agentum-desktop/ui/src/store/slices/gated-run-starting.ts
  crates/agentum-desktop/ui/src/store/slices/workspace-harness-offer.ts
  crates/agentum-desktop/ui/src/components/tab-group/TabGroupPanel.sdd-bar.test.tsx
)

legacy_found=0
for retired in "${retired_paths[@]}"; do
  if [[ -e "$retired" || -L "$retired" ]]; then
    echo "retired SDD runtime surface remains: $retired" >&2
    legacy_found=1
  fi
done

set +e
matches="$(git grep --full-name --files-with-matches --extended-regexp \
  -e '/api/harness' \
  -e '/api/sdd/playbooks' \
  -e 'agentum_sdd_loop' \
  -e 'agentum_harness_' \
  -e 'HarnessEngine' \
  -e 'Harness[A-Z]' \
  -e 'harness_orchestration' \
  -e 'feature_list\.json' \
  -e '\.agentum-harness' \
  -e '(^|[^[:alnum:]_])\.harness/' \
  -e 'GatedRun' \
  -e 'HarnessSpecBanner' \
  -e 'SddBar' \
  -e 'harness-client' \
  -- \
  crates \
  README.md CLAUDE.md docs examples/sdd-demo .github/labels.sh \
  ':(exclude,glob)**/routes/sdd_v2.rs' \
  ':(exclude,glob)**/migrations/**' \
  ':(exclude,glob)docs/migrations/**' 2>/dev/null)"
scan_status=$?
set -e

if [[ $scan_status -eq 0 ]]; then
  echo "retired SDD runtime references remain in:" >&2
  printf '%s\n' "$matches" >&2
  legacy_found=1
fi
if [[ $scan_status -ne 0 && $scan_status -ne 1 ]]; then
  echo "retired SDD runtime scan failed" >&2
  exit "$scan_status"
fi

if [[ $legacy_found -ne 0 ]]; then
  exit 1
fi
