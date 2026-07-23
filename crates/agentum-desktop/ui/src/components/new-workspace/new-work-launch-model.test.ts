import { describe, expect, it, vi } from 'vitest';
import {
  activeNewWorkStage,
  canLaunchNewWork,
  canSelectWorkSource,
  deriveDefaultExecutionMode,
  deriveNewWorkEligibility,
  firstIncompleteNewWorkStage,
  initialNewWorkProgress,
  isNewWorkRetryAvailable,
  newWorkBusyLabel,
  newWorkPrimaryLabel,
  resolveLaunchIssue,
  shouldDefaultNewWorkToManual,
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
    expect(newWorkPrimaryLabel('none')).toBe('Create workspace & start work');
    const eligible = deriveNewWorkEligibility({
      isGit: true,
      source: 'new',
      newIssueProvider: 'github',
      selectedAgentInstalled: true
    });
    expect(eligible).toEqual({ eligible: true });
    expect(deriveDefaultExecutionMode(eligible)).toBe('autopilot');
  });

  it.each([
    [
      {
        isGit: false,
        source: 'new' as const,
        selectedAgentInstalled: true
      },
      'non-git'
    ],
    [
      {
        isGit: true,
        source: 'new' as const,
        selectedAgentInstalled: false
      },
      'agent-unavailable'
    ],
    [
      {
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

  it('keeps host-aware SSH GitHub work eligible', () => {
    expect(
      deriveNewWorkEligibility({
        isGit: true,
        source: 'new',
        newIssueProvider: 'github',
        selectedAgentInstalled: true
      })
    ).toEqual({ eligible: true });
  });

  it('routes a canonical Linear new issue to manual execution', () => {
    const eligibility = deriveNewWorkEligibility({
      isGit: true,
      source: 'new',
      newIssueProvider: 'linear',
      selectedAgentInstalled: true
    });
    expect(eligibility).toMatchObject({
      eligible: false,
      reason: 'non-github-issue'
    });
    expect(deriveDefaultExecutionMode(eligibility)).toBe('manual');
    expect(
      shouldDefaultNewWorkToManual({
        isGit: true,
        source: 'new',
        trackerConfigLoaded: true,
        newIssueProvider: 'linear'
      })
    ).toBe(true);
  });

  it('does not demote Existing while issue selection is still transient', () => {
    expect(
      shouldDefaultNewWorkToManual({
        isGit: true,
        source: 'existing',
        trackerConfigLoaded: true,
        newIssueProvider: 'github',
        linkedWorkItem: null
      })
    ).toBe(false);
    expect(
      shouldDefaultNewWorkToManual({
        isGit: true,
        source: 'existing',
        trackerConfigLoaded: true,
        newIssueProvider: 'github',
        linkedWorkItem: issue
      })
    ).toBe(false);
    expect(
      shouldDefaultNewWorkToManual({
        isGit: true,
        source: 'existing',
        trackerConfigLoaded: true,
        linkedWorkItem: { ...issue, url: 'https://linear.app/acme/issue/ENG-42' }
      })
    ).toBe(true);
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

  it('supports untracked manual work without filing or selecting an issue', async () => {
    const createIssue = vi.fn();
    const result = await resolveLaunchIssue({
      source: 'none',
      checkpoint: {},
      createIssue
    });
    expect(result.issue).toBeNull();
    expect(result.created).toBe(false);
    expect(createIssue).not.toHaveBeenCalled();
    expect(initialNewWorkProgress({}, 'none').issue).toBe('done');
    expect(
      canLaunchNewWork({
        source: 'none',
        executionMode: 'manual',
        eligibility: deriveNewWorkEligibility({
          isGit: true,
          source: 'none',
          selectedAgentInstalled: true
        }),
        hasSelectedAgent: true,
        canStageNewIssue: false,
        hasNewIssueTitle: false,
        hasSelectedIssue: false,
        hasIssueCheckpoint: false
      })
    ).toBe(true);
  });

  it('keeps No issue available while tracker loading/error leaves tracked sources disabled', () => {
    for (const trackerConfigured of [false]) {
      expect(
        canSelectWorkSource({
          source: 'new',
          trackerConfigured,
          canStageNewIssue: false,
          locked: false
        })
      ).toBe(false);
      expect(
        canSelectWorkSource({
          source: 'existing',
          trackerConfigured,
          canStageNewIssue: false,
          locked: false
        })
      ).toBe(false);
      expect(
        canSelectWorkSource({
          source: 'none',
          trackerConfigured,
          canStageNewIssue: false,
          locked: false
        })
      ).toBe(true);
    }

    expect(
      canLaunchNewWork({
        source: 'none',
        executionMode: 'manual',
        eligibility: deriveNewWorkEligibility({
          isGit: true,
          source: 'none',
          selectedAgentInstalled: true
        }),
        hasSelectedAgent: true,
        canStageNewIssue: false,
        hasNewIssueTitle: false,
        hasSelectedIssue: false,
        hasIssueCheckpoint: false
      })
    ).toBe(true);
  });

  it('tracks ordered completion and retry position', () => {
    let progress = initialNewWorkProgress({ linkedWorkItem: issue });
    expect(firstIncompleteNewWorkStage(progress)).toBe('worktree');
    progress = updateNewWorkProgress(progress, 'worktree', 'done');
    progress = updateNewWorkProgress(progress, 'spec', 'error');
    expect(firstIncompleteNewWorkStage(progress)).toBe('spec');
    expect(newWorkPrimaryLabel('new', true)).toBe('Retry from incomplete step');
  });

  it('keeps stage-specific busy copy stable and exposes retry only while idle', () => {
    let progress = initialNewWorkProgress({}, 'new');
    progress = updateNewWorkProgress(progress, 'issue', 'active');
    expect(activeNewWorkStage(progress)).toBe('issue');
    expect(newWorkBusyLabel(progress)).toBe('Preparing issue…');
    expect(isNewWorkRetryAvailable(progress, true)).toBe(false);

    progress = updateNewWorkProgress(progress, 'issue', 'done');
    progress = updateNewWorkProgress(progress, 'worktree', 'active');
    expect(newWorkBusyLabel(progress)).toBe('Creating worktree…');

    progress = updateNewWorkProgress(progress, 'worktree', 'done');
    progress = updateNewWorkProgress(progress, 'spec', 'active');
    expect(newWorkBusyLabel(progress)).toBe('Preparing spec…');

    progress = updateNewWorkProgress(progress, 'spec', 'done');
    progress = updateNewWorkProgress(progress, 'run', 'error');
    expect(newWorkBusyLabel(progress)).toBeNull();
    expect(isNewWorkRetryAvailable(progress, true)).toBe(false);
    expect(isNewWorkRetryAvailable(progress, false)).toBe(true);
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
          reason: 'non-github-issue',
          message: 'Manual only.'
        }
      })
    ).toBe(true);
  });
});
