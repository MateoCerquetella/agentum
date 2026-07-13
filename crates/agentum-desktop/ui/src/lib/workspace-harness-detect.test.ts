// Spec 015 f1 — pins for the pure harness-spec detection model. The two
// load-bearing cases: a DIRECTORY named `feature_list.json` is not a spec, and
// a canonical dir WITHOUT the file beats a legacy dir WITH it (the
// `resolve_harness_dir` mirror — offering that run would hand the engine an
// unloadable project).
import { describe, expect, it } from 'vitest'
import type { FsFileEntry } from '@/runtime/server-fs-client'
import {
  FEATURE_LIST_FILE,
  HARNESS_DIR,
  LEGACY_HARNESS_DIR,
  decideHarnessOffer,
  detectHarnessSpec,
  hasFeatureList,
  normalizeWorkdir,
  shouldDetectHarnessSpec
} from './workspace-harness-detect'

function entry(name: string, kind: FsFileEntry['kind'] = 'file'): FsFileEntry {
  return { name, path: `/w/.agentum-harness/${name}`, kind }
}

const SPEC_FILE = entry(FEATURE_LIST_FILE)

describe('normalizeWorkdir', () => {
  it('strips a trailing slash', () => {
    expect(normalizeWorkdir('/workspace/feature/')).toBe('/workspace/feature')
  })

  it('strips repeated trailing slashes', () => {
    expect(normalizeWorkdir('/workspace/feature///')).toBe('/workspace/feature')
  })

  it('keeps a bare "/"', () => {
    expect(normalizeWorkdir('/')).toBe('/')
  })

  it('trims whitespace', () => {
    expect(normalizeWorkdir('  /workspace/feature ')).toBe('/workspace/feature')
  })

  it('is idempotent', () => {
    const once = normalizeWorkdir(' /workspace/feature/ ')
    expect(normalizeWorkdir(once)).toBe(once)
  })
})

describe('shouldDetectHarnessSpec', () => {
  it('gatedRun suppresses detection (D6)', () => {
    expect(shouldDetectHarnessSpec({ gatedRun: true, connectionId: null })).toBe(false)
  })

  it('SSH connectionId suppresses detection (D5)', () => {
    expect(shouldDetectHarnessSpec({ gatedRun: false, connectionId: 'ssh-1' })).toBe(false)
  })

  it('unknown worktree (undefined connectionId) fails closed', () => {
    expect(shouldDetectHarnessSpec({ gatedRun: false, connectionId: undefined })).toBe(false)
  })

  it('local (null connectionId) detects', () => {
    expect(shouldDetectHarnessSpec({ gatedRun: false, connectionId: null })).toBe(true)
  })
})

describe('hasFeatureList', () => {
  it('finds the spec file among entries', () => {
    expect(hasFeatureList([entry('AGENTS.md'), SPEC_FILE])).toBe(true)
  })

  it('a DIRECTORY named feature_list.json is not a spec', () => {
    expect(hasFeatureList([entry(FEATURE_LIST_FILE, 'dir')])).toBe(false)
  })

  it('empty listing has no spec', () => {
    expect(hasFeatureList([])).toBe(false)
  })
})

describe('detectHarnessSpec', () => {
  it('canonical dir with the file wins', () => {
    expect(detectHarnessSpec([SPEC_FILE], null)).toEqual({
      found: true,
      harnessDir: HARNESS_DIR
    })
  })

  it('falls back to legacy only when the canonical dir is absent', () => {
    expect(detectHarnessSpec(null, [SPEC_FILE])).toEqual({
      found: true,
      harnessDir: LEGACY_HARNESS_DIR
    })
  })

  it('both dirs absent means not found', () => {
    expect(detectHarnessSpec(null, null)).toEqual({ found: false })
  })

  it('canonical dir WITHOUT the file beats legacy WITH it (resolve_harness_dir mirror)', () => {
    // The engine prefers an existing .agentum-harness/; if that dir has no
    // feature_list.json the run cannot load — never offer the legacy spec then.
    expect(detectHarnessSpec([entry('AGENTS.md')], [SPEC_FILE])).toEqual({ found: false })
  })
})

describe('decideHarnessOffer', () => {
  const found = { found: true, harnessDir: HARNESS_DIR } as const

  it('already-registered workdir yields no offer (AC 5)', () => {
    expect(
      decideHarnessOffer({
        detection: found,
        worktreeId: 'repo-1::/workspace/feature',
        workdir: '/workspace/feature',
        registeredWorkdirs: ['/workspace/feature']
      })
    ).toBeNull()
  })

  it('trailing-slash spelling still matches a registered workdir', () => {
    expect(
      decideHarnessOffer({
        detection: found,
        worktreeId: 'repo-1::/workspace/feature',
        workdir: '/workspace/feature/',
        registeredWorkdirs: ['/workspace/feature']
      })
    ).toBeNull()
  })

  it('unregistered workdir yields the offer', () => {
    expect(
      decideHarnessOffer({
        detection: found,
        worktreeId: 'repo-1::/workspace/feature',
        workdir: '/workspace/feature',
        registeredWorkdirs: ['/somewhere/else']
      })
    ).toEqual({
      worktreeId: 'repo-1::/workspace/feature',
      workdir: '/workspace/feature',
      harnessDir: HARNESS_DIR
    })
  })

  it('no detection means no offer regardless of registrations', () => {
    expect(
      decideHarnessOffer({
        detection: { found: false },
        worktreeId: 'repo-1::/workspace/feature',
        workdir: '/workspace/feature',
        registeredWorkdirs: []
      })
    ).toBeNull()
  })
})
