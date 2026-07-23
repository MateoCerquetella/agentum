import type { LinkedWorkItemSummary } from '@/lib/new-workspace';
import type { CreateWorktreeResult } from '../../../../shared/types';

export type WorkSource = 'new' | 'existing' | 'none';
export type ExecutionMode = 'autopilot' | 'manual';
export type NewWorkStage = 'issue' | 'worktree' | 'spec' | 'run';
export type NewWorkStageStatus = 'pending' | 'active' | 'done' | 'error';

export type NewWorkProgress = Record<NewWorkStage, NewWorkStageStatus>;

export type NewWorkCheckpoint = {
  linkedWorkItem?: LinkedWorkItemSummary;
  worktreeResult?: CreateWorktreeResult;
};

export type NewWorkEligibility =
  | { eligible: true }
  | {
      eligible: false;
      reason:
        | 'non-git'
        | 'non-github-issue'
        | 'agent-unavailable'
        | 'setup-blocked';
      message: string;
    };

export const NEW_WORK_STAGES: readonly NewWorkStage[] = [
  'issue',
  'worktree',
  'spec',
  'run'
];

/** Stable footer copy for each durable launch checkpoint. Keeping these labels
 * in the model prevents the CTA from bouncing between incidental hook flags
 * while issue creation hands off to worktree/spec/run orchestration. */
export const NEW_WORK_STAGE_ACTIVE_LABELS: Readonly<Record<NewWorkStage, string>> = {
  issue: 'Preparing issue…',
  worktree: 'Creating worktree…',
  spec: 'Preparing spec…',
  run: 'Starting run…'
};

export function activeNewWorkStage(progress: NewWorkProgress): NewWorkStage | null {
  return NEW_WORK_STAGES.find(stage => progress[stage] === 'active') ?? null;
}

export function newWorkBusyLabel(progress: NewWorkProgress): string | null {
  const stage = activeNewWorkStage(progress);
  return stage ? NEW_WORK_STAGE_ACTIVE_LABELS[stage] : null;
}

/** A retry is offered only after the pipeline is idle with a failed durable
 * stage. An attempted-but-still-running launch must retain its stage label. */
export function isNewWorkRetryAvailable(
  progress: NewWorkProgress,
  launchBusy: boolean
): boolean {
  return !launchBusy && NEW_WORK_STAGES.some(stage => progress[stage] === 'error');
}

export function canSelectWorkSource(input: {
  source: WorkSource
  trackerConfigured: boolean
  canStageNewIssue: boolean
  locked: boolean
}): boolean {
  if (input.locked) return false
  if (input.source === 'none') return true
  if (!input.trackerConfigured) return false
  return input.source !== 'new' || input.canStageNewIssue
}

export function initialNewWorkProgress(
  checkpoint: NewWorkCheckpoint = {},
  source: WorkSource = 'new'
): NewWorkProgress {
  return {
    issue: source === 'none' || checkpoint.linkedWorkItem ? 'done' : 'pending',
    worktree: checkpoint.worktreeResult ? 'done' : 'pending',
    spec: 'pending',
    run: 'pending'
  };
}

export function updateNewWorkProgress(
  progress: NewWorkProgress,
  stage: NewWorkStage,
  status: NewWorkStageStatus
): NewWorkProgress {
  return { ...progress, [stage]: status };
}

export function newWorkPrimaryLabel(
  source: WorkSource,
  retrying = false
): string {
  if (retrying) return 'Retry from incomplete step';
  if (source === 'new') return 'Create issue & start work';
  return source === 'none' ? 'Create workspace & start work' : 'Create worktree & start work';
}

export function deriveDefaultExecutionMode(
  eligibility: NewWorkEligibility
): ExecutionMode {
  return eligibility.eligible ? 'autopilot' : 'manual';
}

function isGitHubIssue(item: LinkedWorkItemSummary | null | undefined): boolean {
  return Boolean(
    item &&
      item.type === 'issue' &&
      item.url.toLowerCase().includes('github.com/')
  );
}

/** Whether a source is structurally incompatible with Autopilot. Transient
 * states (tracker loading, no existing issue selected yet, agent/setup probes)
 * deliberately do not demote the operator's execution-mode choice. */
export function shouldDefaultNewWorkToManual(input: {
  isGit: boolean;
  source: WorkSource;
  trackerConfigLoaded: boolean;
  newIssueProvider?: 'github' | 'linear' | null;
  linkedWorkItem?: LinkedWorkItemSummary | null;
}): boolean {
  if (!input.isGit || input.source === 'none') return true;
  if (input.source === 'new') {
    return input.trackerConfigLoaded && input.newIssueProvider !== 'github';
  }
  return Boolean(input.linkedWorkItem && !isGitHubIssue(input.linkedWorkItem));
}

export function canLaunchNewWork(input: {
  source: WorkSource;
  executionMode: ExecutionMode;
  eligibility: NewWorkEligibility;
  hasSelectedAgent: boolean;
  canStageNewIssue: boolean;
  hasNewIssueTitle: boolean;
  hasSelectedIssue: boolean;
  hasIssueCheckpoint: boolean;
}): boolean {
  if (!input.hasSelectedAgent) return false;
  if (
    !input.eligibility.eligible &&
    (input.eligibility.reason === 'agent-unavailable' ||
      input.eligibility.reason === 'setup-blocked')
  ) {
    return false;
  }
  if (
    input.source === 'new' &&
    !input.hasIssueCheckpoint &&
    (!input.canStageNewIssue || !input.hasNewIssueTitle)
  ) {
    return false;
  }
  if (
    input.source === 'existing' &&
    !input.hasIssueCheckpoint &&
    !input.hasSelectedIssue
  ) {
    return false;
  }
  return input.executionMode === 'manual' || input.eligibility.eligible;
}

export function deriveNewWorkEligibility(input: {
  isGit: boolean;
  source: WorkSource;
  /** Canonical provider used when `source` creates a new issue. `undefined`
   * means the repo-scoped config is still loading; `null` means it loaded
   * without a tracker. */
  newIssueProvider?: 'github' | 'linear' | null;
  linkedWorkItem?: LinkedWorkItemSummary | null;
  selectedAgentInstalled: boolean;
  setupBlocked?: boolean;
}): NewWorkEligibility {
  if (!input.isGit) {
    return {
      eligible: false,
      reason: 'non-git',
      message: 'SDD Autopilot requires a Git project.'
    };
  }
  if (!input.selectedAgentInstalled) {
    return {
      eligible: false,
      reason: 'agent-unavailable',
      message: 'Choose an installed agent before starting work.'
    };
  }
  if (input.setupBlocked) {
    return {
      eligible: false,
      reason: 'setup-blocked',
      message: 'Resolve the project setup requirement before starting work.'
    };
  }
  if (input.source === 'none') {
    return {
      eligible: false,
      reason: 'non-github-issue',
      message: 'SDD Autopilot needs a GitHub issue. Choose Open manually to work without one.'
    };
  }
  if (
    input.source === 'new' &&
    input.newIssueProvider !== undefined &&
    input.newIssueProvider !== 'github'
  ) {
    return {
      eligible: false,
      reason: 'non-github-issue',
      message: input.newIssueProvider === 'linear'
        ? 'SDD Autopilot requires a GitHub issue. Linear issues open manually.'
        : 'SDD Autopilot needs a configured GitHub issue tracker.'
    };
  }
  if (
    input.source === 'existing' &&
    !isGitHubIssue(input.linkedWorkItem)
  ) {
    return {
      eligible: false,
      reason: 'non-github-issue',
      message: 'SDD Autopilot requires a GitHub issue from this project.'
    };
  }
  return { eligible: true };
}

export function firstIncompleteNewWorkStage(
  progress: NewWorkProgress
): NewWorkStage | null {
  return NEW_WORK_STAGES.find(stage => progress[stage] !== 'done') ?? null;
}

export async function resolveLaunchIssue(input: {
  source: WorkSource;
  selectedIssue?: LinkedWorkItemSummary | null;
  checkpoint: NewWorkCheckpoint;
  createIssue: () => Promise<LinkedWorkItemSummary | null>;
}): Promise<{
  checkpoint: NewWorkCheckpoint;
  issue: LinkedWorkItemSummary | null;
  created: boolean;
}> {
  if (input.checkpoint.linkedWorkItem) {
    return {
      checkpoint: input.checkpoint,
      issue: input.checkpoint.linkedWorkItem,
      created: false
    };
  }
  const issue = input.source === 'new'
    ? await input.createIssue()
    : input.source === 'existing'
      ? (input.selectedIssue ?? null)
      : null;
  if (!issue)
    return { checkpoint: input.checkpoint, issue: null, created: false };
  return {
    checkpoint: { ...input.checkpoint, linkedWorkItem: issue },
    issue,
    created: input.source === 'new'
  };
}
