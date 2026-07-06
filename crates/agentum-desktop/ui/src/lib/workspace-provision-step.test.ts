import { describe, expect, it } from 'vitest'
import {
  DEFAULT_TEMPLATE_REPO,
  deriveTemplateRepoName,
  firstTemplateModeBlocker,
  isTemplateModeReady,
  provisionCommitFileList,
  summarizeProvisionReport,
  type ProvisionReport
} from './workspace-provision-step'

// Spec 010 F3: pin the pure behaviors of the wizard provision step — the D8
// consent's exact committed-file list, template-mode gating, the goal → repo
// name seed, and the per-step report summary (failures = warnings).

describe('provisionCommitFileList (D8 "lists exactly what will be committed")', () => {
  it('names exactly the five contract paths, branch-agnostic', () => {
    expect(provisionCommitFileList()).toEqual([
      '.agentum-harness/.gitignore',
      '.agentum-harness/AGENTS.md',
      '.agentum-harness/init.sh',
      '.agentum-harness/verify.sh',
      '.agentum-harness/qa.sh'
    ])
  })

  it('never lists engine-written state (it stays gitignored)', () => {
    const list = provisionCommitFileList()
    for (const state of ['feature_list.json', 'handoff.md', 'qa/']) {
      expect(list.some((p) => p.includes(state))).toBe(false)
    }
  })
})

describe('deriveTemplateRepoName (goal → repo-name seed)', () => {
  it('slugifies the goal like the workspace-name seed', () => {
    expect(deriveTemplateRepoName('Add OAuth login')).toBe('add-oauth-login')
    expect(deriveTemplateRepoName('Fix the 🔥 login bug')).toBe('fix-the-login-bug')
  })

  it('returns empty for a blank goal (field stays editable, never garbage)', () => {
    expect(deriveTemplateRepoName('   ')).toBe('')
  })
})

describe('template-mode readiness gating', () => {
  const ready = {
    goal: 'Build a thing',
    owner: 'acme',
    name: 'build-a-thing',
    templateRepo: DEFAULT_TEMPLATE_REPO,
    directory: '/Users/me/projects'
  }

  it('is ready when goal + owner + valid name + owner/repo template + directory are set', () => {
    expect(isTemplateModeReady(ready)).toBe(true)
    expect(firstTemplateModeBlocker(ready)).toBeNull()
  })

  it('defaults the template constant to the D4 starter', () => {
    expect(DEFAULT_TEMPLATE_REPO).toBe('goempirical/empirical-sdd-ddd-starter')
  })

  it('names the first unmet input, goal first — never silent', () => {
    expect(firstTemplateModeBlocker({ ...ready, goal: ' ' })).toMatch(/goal/i)
    expect(firstTemplateModeBlocker({ ...ready, owner: '' })).toMatch(/owner/i)
    expect(firstTemplateModeBlocker({ ...ready, name: '' })).toMatch(/name/i)
    expect(firstTemplateModeBlocker({ ...ready, directory: '' })).toMatch(/folder/i)
  })

  it('rejects repo names with separators/traversal and malformed templates', () => {
    for (const bad of ['a/b', 'a b', '..', '.', 'a\\b']) {
      expect(isTemplateModeReady({ ...ready, name: bad })).toBe(false)
    }
    for (const bad of ['starter', 'o/', '/r', 'o/r/x', 'o r/t']) {
      expect(isTemplateModeReady({ ...ready, templateRepo: bad })).toBe(false)
    }
  })
})

describe('summarizeProvisionReport (per-step inline report; failures = warnings)', () => {
  const step = (ok: boolean, changed: boolean, detail: string) => ({ ok, changed, detail })
  const green: ProvisionReport = {
    labels: step(true, false, 'ensured the five status labels'),
    project: step(true, true, 'created project "Board" (#7)'),
    binding: step(true, true, 'bound acme/widgets → Board'),
    scaffold: step(true, true, 'wrote .agentum-harness/AGENTS.md'),
    commit: { committed: true, pushed: true, branch: 'main', error: null }
  }

  it('renders five lines with the step details; a green commit names the branch', () => {
    const lines = summarizeProvisionReport(green)
    expect(lines.map((l) => l.id)).toEqual(['labels', 'project', 'binding', 'scaffold', 'commit'])
    expect(lines.every((l) => l.ok)).toBe(true)
    expect(lines[1]!.text).toContain('created project')
    expect(lines[4]!.text).toContain('pushed to main')
  })

  it('a red push is a WARNING that surfaces the error and says push manually', () => {
    const lines = summarizeProvisionReport({
      ...green,
      commit: { committed: true, pushed: false, branch: 'main', error: 'no upstream' }
    })
    const commit = lines[4]!
    expect(commit.ok).toBe(false)
    expect(commit.text).toContain('no upstream')
    expect(commit.text).toMatch(/push manually/i)
  })

  it('a failed step keeps its detail and warns without hiding the rest', () => {
    const lines = summarizeProvisionReport({
      ...green,
      binding: step(false, false, 'not bound — discovery failed: gh auth refresh -s project')
    })
    expect(lines[2]!.ok).toBe(false)
    expect(lines[2]!.text).toContain('gh auth refresh -s project')
    expect(lines[3]!.ok).toBe(true)
  })

  it('no commit + no error reads as "no new commit" (consent off or nothing new)', () => {
    const lines = summarizeProvisionReport({
      ...green,
      commit: { committed: false, pushed: false, branch: '', error: null }
    })
    expect(lines[4]!.ok).toBe(true)
    expect(lines[4]!.text).toBe('no new commit')
  })
})
