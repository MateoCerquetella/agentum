import type { LinkedWorkItemSummary } from '@/lib/new-workspace';
import type { CreateWorktreeResult } from '../../../../shared/types';

export type WorkSource = 'new' | 'existing';
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
        | 'remote-repo'
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

export function initialNewWorkProgress(
  checkpoint: NewWorkCheckpoint = {}
): NewWorkProgress {
  return {
    issue: checkpoint.linkedWorkItem ? 'done' : 'pending',
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
  return source === 'new'
    ? 'Create issue & start work'
    : 'Create worktree & start work';
}

export function deriveDefaultExecutionMode(
  eligibility: NewWorkEligibility
): ExecutionMode {
  return eligibility.eligible ? 'autopilot' : 'manual';
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
  isLocal: boolean;
  isGit: boolean;
  source: WorkSource;
  linkedWorkItem?: LinkedWorkItemSummary | null;
  selectedAgentInstalled: boolean;
  setupBlocked?: boolean;
}): NewWorkEligibility {
  if (!input.isLocal) {
    return {
      eligible: false,
      reason: 'remote-repo',
      message: 'SDD Autopilot is not available for SSH projects.'
    };
  }
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
  if (
    input.source === 'existing' &&
    (!input.linkedWorkItem ||
      input.linkedWorkItem.type !== 'issue' ||
      !input.linkedWorkItem.url.toLowerCase().includes('github.com/'))
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
  const issue =
    input.source === 'new'
      ? await input.createIssue()
      : (input.selectedIssue ?? null);
  if (!issue)
    return { checkpoint: input.checkpoint, issue: null, created: false };
  return {
    checkpoint: { ...input.checkpoint, linkedWorkItem: issue },
    issue,
    created: input.source === 'new'
  };
}
