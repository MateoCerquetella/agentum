import { describe, expect, it, vi } from 'vitest';
import {
  canLaunchNewWork,
  deriveDefaultExecutionMode,
  deriveNewWorkEligibility,
  firstIncompleteNewWorkStage,
  initialNewWorkProgress,
  newWorkPrimaryLabel,
  resolveLaunchIssue,
  updateNewWorkProgress
} from './new-work-launch-model';

const issue = {
  type: 'issue' as const,
  number: 42,
  title: 'Add widget',
  url: 'https://github.com/acme/widgets/issues/42'
};

describe('new work launch model', () => {
  it('uses contextual final labels and defaults eligible work to Autopilot', () => {
    expect(newWorkPrimaryLabel('new')).toBe('Create issue & start work');
    expect(newWorkPrimaryLabel('existing')).toBe(
      'Create worktree & start work'
    );
    const eligible = deriveNewWorkEligibility({
      isLocal: true,
      isGit: true,
      source: 'new',
      selectedAgentInstalled: true
    });
    expect(eligible).toEqual({ eligible: true });
    expect(deriveDefaultExecutionMode(eligible)).toBe('autopilot');
  });

  it.each([
    [
      {
        isLocal: false,
        isGit: true,
        source: 'new' as const,
        selectedAgentInstalled: true
      },
      'remote-repo'
    ],
    [
      {
        isLocal: true,
        isGit: false,
        source: 'new' as const,
        selectedAgentInstalled: true
      },
      'non-git'
    ],
    [
      {
        isLocal: true,
        isGit: true,
        source: 'new' as const,
        selectedAgentInstalled: false
      },
      'agent-unavailable'
    ],
    [
      {
        isLocal: true,
        isGit: true,
        source: 'existing' as const,
        selectedAgentInstalled: true,
        linkedWorkItem: null
      },
      'non-github-issue'
    ]
  ])('reports an honest incompatibility for %j', (input, reason) => {
    expect(deriveNewWorkEligibility(input)).toMatchObject({
      eligible: false,
      reason
    });
  });

  it('checkpoints a created issue and never files it twice on retry', async () => {
    const createIssue = vi.fn(async () => issue);
    const first = await resolveLaunchIssue({
      source: 'new',
      checkpoint: {},
      createIssue
    });
    const retry = await resolveLaunchIssue({
      source: 'new',
      checkpoint: first.checkpoint,
      createIssue
    });
    expect(first.created).toBe(true);
    expect(retry.issue).toEqual(issue);
    expect(createIssue).toHaveBeenCalledTimes(1);
  });

  it('uses an existing issue without invoking issue creation', async () => {
    const createIssue = vi.fn();
    const result = await resolveLaunchIssue({
      source: 'existing',
      selectedIssue: issue,
      checkpoint: {},
      createIssue
    });
    expect(result.issue).toEqual(issue);
    expect(createIssue).not.toHaveBeenCalled();
  });

  it('tracks ordered completion and retry position', () => {
    let progress = initialNewWorkProgress({ linkedWorkItem: issue });
    expect(firstIncompleteNewWorkStage(progress)).toBe('worktree');
    progress = updateNewWorkProgress(progress, 'worktree', 'done');
    progress = updateNewWorkProgress(progress, 'spec', 'error');
    expect(firstIncompleteNewWorkStage(progress)).toBe('spec');
    expect(newWorkPrimaryLabel('new', true)).toBe('Retry from incomplete step');
  });

  it('uses one launch gate for mouse and keyboard submission', () => {
    const base = {
      source: 'new' as const,
      executionMode: 'manual' as const,
      hasSelectedAgent: true,
      canStageNewIssue: true,
      hasNewIssueTitle: true,
      hasSelectedIssue: false,
      hasIssueCheckpoint: false
    };
    expect(
      canLaunchNewWork({
        ...base,
        eligibility: {
          eligible: false,
          reason: 'agent-unavailable',
          message: 'Choose an installed agent.'
        }
      })
    ).toBe(false);
    expect(
      canLaunchNewWork({
        ...base,
        eligibility: {
          eligible: false,
          reason: 'setup-blocked',
          message: 'Resolve setup.'
        }
      })
    ).toBe(false);
    expect(
      canLaunchNewWork({
        ...base,
        source: 'existing',
        hasSelectedIssue: true,
        eligibility: {
          eligible: false,
          reason: 'remote-repo',
          message: 'Manual only.'
        }
      })
    ).toBe(true);
  });
});
