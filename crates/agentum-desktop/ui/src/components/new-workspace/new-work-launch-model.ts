import type { LinkedWorkItemSummary } from '@/lib/new-workspace';
import type { CreateWorktreeResult } from '@/shared/types';
import type { CreateSpecResult } from '@/runtime/sdd-client';
import type { WizardWorkSource } from './create-workspace-wizard-model';

export type WorkSource = WizardWorkSource;
export type NewWorkStage = 'issue' | 'worktree' | 'sdd';
export type NewWorkStageStatus = 'pending' | 'active' | 'done' | 'error';

export type NewWorkProgress = Record<NewWorkStage, NewWorkStageStatus>;

export type NewWorkCheckpoint = {
  linkedWorkItem?: LinkedWorkItemSummary;
  worktreeResult?: CreateWorktreeResult;
  sddResult?: CreateSpecResult;
};

export const NEW_WORK_STAGES: readonly NewWorkStage[] = [
  'issue',
  'sdd',
  'worktree'
];

/** Stable footer copy for each durable launch checkpoint. Keeping these labels
 * in the model prevents the CTA from bouncing between incidental hook flags
 * while issue creation hands off to worktree creation. */
export const NEW_WORK_STAGE_ACTIVE_LABELS: Readonly<Record<NewWorkStage, string>> = {
  issue: 'Preparing issue…',
  worktree: 'Creating worktree…',
  sdd: 'Starting SDD run…'
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
    issue: source === 'none' || source === 'sdd' || checkpoint.linkedWorkItem ? 'done' : 'pending',
    sdd: source !== 'sdd' || checkpoint.sddResult ? 'done' : 'pending',
    worktree: checkpoint.worktreeResult ? 'done' : 'pending'
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
  if (source === 'new') return 'Create issue';
  if (source === 'sdd') return 'Create workspace & start SDD';
  return source === 'none' ? 'Create workspace' : 'Create worktree';
}

export function canLaunchNewWork(input: {
  source: WorkSource;
  hasSelectedAgent: boolean;
  canStageNewIssue: boolean;
  hasNewIssueTitle: boolean;
  hasSelectedIssue: boolean;
  hasIssueCheckpoint: boolean;
  hasSddDescription?: boolean;
}): boolean {
  if (!input.hasSelectedAgent) return false;
  if (input.source === 'sdd' && !input.hasSddDescription) return false;
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
  return true;
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
