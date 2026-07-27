#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

if [[ -e .agentum-migration-journal.json || -L .agentum-migration-journal.json ]]; then
  echo "an incomplete SDD migration journal blocks release" >&2
  exit 1
fi

# These files change a provider's behavior merely by existing in a customer
# checkout. SDD adapters receive policy through CommandSpec and must never
# generate ambient provider configuration in a project.
for forbidden in \
  opencode.json \
  .opencode \
  .cursor \
  .codex \
  .gemini \
  .hermes \
  .aider.conf.yml \
  .aiderignore; do
  if [[ -e "$forbidden" || -L "$forbidden" ]] && \
    git ls-files --error-unmatch -- "$forbidden" >/dev/null 2>&1; then
    echo "forbidden tracked provider configuration: $forbidden" >&2
    exit 1
  fi
done

# The v2 cutover has no compatibility window. These tracked project-owned
# authoring surfaces must be migrated or archived before a release; normal
# product docs and contributor-owned .claude settings are intentionally not in
# this list.
retired_found=0
for retired in ai .agentum-harness spec.md architecture.md execution-plan.json examples/harness-demo; do
  if [[ -e "$retired" || -L "$retired" ]] && \
    git ls-files --error-unmatch -- "$retired" >/dev/null 2>&1; then
    echo "retired SDD surface is still tracked: $retired" >&2
    retired_found=1
  fi
done
if [[ $retired_found -ne 0 ]]; then
  exit 1
fi

if [[ -e .agentum || -L .agentum ]]; then
  python3 scripts/check-agentum-artifacts.py .agentum
fi

# Tracker work must enter through New Spec. Keep the remaining manual
# workspace creator available, but make every new direct creator an explicit
# review event by failing on files outside this closed allowlist.
ui_root="crates/agentum-desktop/ui/src"
if rg -n --glob '*.{ts,tsx}' --glob '!**/*.test.*' \
  'launch-work-item-direct|launchWorkItemDirect' "$ui_root" >/dev/null; then
  echo "retired tracker-direct launcher is present in the production UI source" >&2
  exit 1
fi

direct_workspace_pattern="createWorktree[[:space:]]*\\(|openModal[[:space:]]*\\([[:space:]]*['\\\"]new-workspace-composer"
direct_workspace_callers="$({
  rg -l --glob '*.{ts,tsx}' --glob '!**/*.test.*' \
    "$direct_workspace_pattern" "$ui_root" || true
} | sort)"
while IFS= read -r caller; do
  [[ -z "$caller" ]] && continue
  case "$caller" in
    "$ui_root/App.tsx" | \
    "$ui_root/components/WorktreeJumpPalette.tsx" | \
    "$ui_root/components/mission-control/MissionControlPage.tsx" | \
    "$ui_root/components/sidebar/AddProjectFromFolderDialog.tsx" | \
    "$ui_root/components/sidebar/AddRepoDialog.tsx" | \
    "$ui_root/components/sidebar/OperationalSidebarControls.tsx" | \
    "$ui_root/components/sidebar/ProjectAddedDialog.tsx" | \
    "$ui_root/components/sidebar/SidebarHeader.tsx" | \
    "$ui_root/components/sidebar/WorktreeList.tsx" | \
    "$ui_root/components/sidebar/use-workspace-kanban-create-worktree.ts" | \
    "$ui_root/components/terminal-pane/terminal-agent-session-fork.ts" | \
    "$ui_root/hooks/useComposerState.ts" | \
    "$ui_root/hooks/useIpcEvents.ts" | \
    "$ui_root/store/selectors.ts")
      ;;
    *)
      echo "unreviewed production workspace creator: $caller" >&2
      exit 1
      ;;
  esac
done <<<"$direct_workspace_callers"

tracker_surfaces=(
  "$ui_root/components/TaskPage.tsx"
  "$ui_root/components/GitHubItemDialog.tsx"
  "$ui_root/components/PullRequestPage.tsx"
  "$ui_root/components/LinearIssueWorkspace.tsx"
  "$ui_root/components/LinearItemDrawer.tsx"
  "$ui_root/components/github-project/ProjectViewWrapper.tsx"
  "$ui_root/components/project-hub/LockedGithubRepoTasks.tsx"
  "$ui_root/components/project-hub/LockedLinearProjectTasks.tsx"
)
for surface in "${tracker_surfaces[@]}"; do
  if rg -n "$direct_workspace_pattern" "$surface" >/dev/null; then
    echo "tracker surface bypasses New Spec: $surface" >&2
    exit 1
  fi
done

if rg -n --glob '*.{ts,tsx}' --glob '!**/*.test.*' \
  'Start workspace|Start new workspace|Start work from' "$ui_root" >/dev/null; then
  echo "retired tracker-direct CTA remains in production UI copy" >&2
  exit 1
fi

palette="$ui_root/components/WorktreeJumpPalette.tsx"
wizard="$ui_root/components/new-workspace/CreateWorkspaceWizard.tsx"
if ! rg -q 'requestNewSpecFromWorkItem' "$palette" || rg -q 'data\.linkedWorkItem' "$palette"; then
  echo "Cmd+J tracker intake must route only to New Spec" >&2
  exit 1
fi
if ! rg -q 'initialLinkedWorkItem: null' "$wizard" || \
  ! rg -q 'enableIssueAutomation: false' "$wizard" || \
  rg -q 'CanonicalTrackerSection|onCreateIssueSubmit' "$wizard"; then
  echo "manual New Workspace must remain tracker-neutral" >&2
  exit 1
fi

if [[ "${1:-}" == "--boundary-only" ]]; then
  exit 0
fi

patterns_file="${1:-${AGENTUM_RESTRICTED_PATTERNS_FILE:-}}"
if [[ -z "$patterns_file" ]]; then
  echo "usage: $0 /absolute/path/to/restricted-patterns" >&2
  echo "or set AGENTUM_RESTRICTED_PATTERNS_FILE" >&2
  exit 2
fi
if [[ "$patterns_file" != /* || ! -f "$patterns_file" ]]; then
  echo "restricted patterns must be supplied as an existing absolute file" >&2
  exit 2
fi

case "$patterns_file" in
  "$repo_root"/*)
    echo "restricted patterns must remain outside the repository" >&2
    exit 2
    ;;
esac

# Only file names are printed. The external deny patterns and matching content
# never enter the command line or logs. Build a private filtered pattern file
# so release owners can keep comments and blank lines in the external source.
scan_patterns="$(mktemp)"
chmod 600 "$scan_patterns"
trap 'rm -f "$scan_patterns"' EXIT
while IFS= read -r pattern || [[ -n "$pattern" ]]; do
  [[ -z "$pattern" || "$pattern" == \#* ]] && continue
  printf '%s\n' "$pattern" >>"$scan_patterns"
done <"$patterns_file"
if [[ ! -s "$scan_patterns" ]]; then
  echo "restricted pattern file contains no active patterns" >&2
  exit 2
fi

set +e
matches="$({
  rg --files-with-matches --no-messages --hidden \
    --glob '!.git' \
    --glob '!.git/**' \
    --glob '!target/**' \
    --glob '!**/node_modules/**' \
    --file "$scan_patterns" \
    .
} 2>/dev/null)"
scan_status=$?
set -e

if [[ $scan_status -eq 0 ]]; then
  echo "restricted content found in:" >&2
  printf '%s\n' "$matches" >&2
  exit 1
fi
if [[ $scan_status -ne 1 ]]; then
  echo "restricted-content scan failed" >&2
  exit "$scan_status"
fi
