import { describe, expect, it } from 'vitest'
import {
  BOARD_PHASES,
  EMPTY_SELECTION,
  fallbackHints,
  mappingComplete,
  optionNamesForSelection,
  reduceBindingSelection,
  selectionForRebind,
  selectionFromResolved,
  type ResolvedMappingDto
} from './github-projects-binding'

// Spec 010 F1 (AC 3): pin the pure halves of the binding editor — the
// select-state reducer, the mappingComplete Save gate, and the D5
// fallback-hint derivation. All pure so they run without a DOM.

const resolvedDefault: ResolvedMappingDto = {
  todo: { optionId: 'o1', name: 'Todo', via: 'matched' },
  inProgress: { optionId: 'o2', name: 'In Progress', via: 'matched' },
  // #379: no Review/PR column here → in_review falls back to In Progress.
  inReview: { optionId: 'o2', name: 'In Progress', via: 'fell_back' },
  readyToTest: { optionId: 'o2', name: 'In Progress', via: 'fell_back' },
  done: { optionId: 'o3', name: 'Done', via: 'matched' },
  blocked: { optionId: 'o2', name: 'In Progress', via: 'fell_back' }
}

describe('reduceBindingSelection (select-state reducer)', () => {
  it('sets one phase without touching the others', () => {
    const next = reduceBindingSelection(EMPTY_SELECTION, {
      type: 'set',
      phase: 'readyToTest',
      optionId: 'qa-1'
    })
    expect(next.readyToTest).toBe('qa-1')
    expect(next.todo).toBe('')
    // Immutability: the shared EMPTY_SELECTION constant is never mutated.
    expect(EMPTY_SELECTION.readyToTest).toBe('')
  })

  it('reset replaces the whole selection (fresh discovery / stored binding)', () => {
    const seeded = reduceBindingSelection(EMPTY_SELECTION, {
      type: 'reset',
      selection: selectionFromResolved(resolvedDefault)
    })
    expect(seeded.todo).toBe('o1')
    expect(seeded.blocked).toBe('o2')
  })
})

describe('selectionFromResolved', () => {
  it('pre-selects every phase from the resolved mapping, fallbacks included', () => {
    const sel = selectionFromResolved(resolvedDefault)
    expect(sel).toEqual({
      todo: 'o1',
      inProgress: 'o2',
      // #379: a fell_back in_review is left EMPTY (optional/unmapped), not
      // pre-picked to the In Progress option.
      inReview: '',
      readyToTest: 'o2',
      done: 'o3',
      blocked: 'o2'
    })
  })

  it('#379: a MATCHED in_review is pre-selected to its Review/PR column', () => {
    const withReview: ResolvedMappingDto = {
      ...resolvedDefault,
      inReview: { optionId: 'pr1', name: 'In Review', via: 'matched' }
    }
    expect(selectionFromResolved(withReview).inReview).toBe('pr1')
  })

  it('a refusal (resolved: null) yields empty selects — never a partial pre-selection', () => {
    const sel = selectionFromResolved(null)
    for (const phase of BOARD_PHASES) {
      expect(sel[phase]).toBe('')
    }
  })
})

describe('selectionForRebind (re-discover on a bound repo)', () => {
  const options = [
    { id: 'o1', name: 'Todo' },
    { id: 'o2', name: 'In Progress' },
    { id: 'o3', name: 'Done' },
    { id: 'qa', name: 'QA' }
  ]

  it('prefers stored ids that still exist so manual edits survive re-discovery', () => {
    const stored = { todo: 'o1', inProgress: 'o2', readyToTest: 'qa', done: 'o3', blocked: 'o2' }
    const sel = selectionForRebind(stored, resolvedDefault, options)
    expect(sel.readyToTest).toBe('qa')
  })

  it('falls back to resolved when a stored option was deleted on GitHub', () => {
    const stored = {
      todo: 'deleted',
      inProgress: 'o2',
      readyToTest: 'qa',
      done: 'o3',
      blocked: 'o2'
    }
    const sel = selectionForRebind(stored, resolvedDefault, options)
    expect(sel.todo).toBe('o1')
  })
})

describe('mappingComplete (the Save gate)', () => {
  it('is false until every phase has an option id', () => {
    expect(mappingComplete(EMPTY_SELECTION)).toBe(false)
    const partial = { ...selectionFromResolved(resolvedDefault), blocked: '' }
    expect(mappingComplete(partial)).toBe(false)
    expect(mappingComplete({ ...partial, blocked: '  ' })).toBe(false)
    expect(mappingComplete(selectionFromResolved(resolvedDefault))).toBe(true)
  })

  it('#379: stays true with In Review unmapped — it is optional', () => {
    const sel = { ...selectionFromResolved(resolvedDefault), inReview: '' }
    expect(mappingComplete(sel)).toBe(true)
  })
})

describe('fallbackHints (D5: fallbacks are VISIBLE)', () => {
  it('derives a hint per fell_back phase naming the fallback option and the recovery', () => {
    const hints = fallbackHints(resolvedDefault)
    expect(Object.keys(hints).sort()).toEqual(['blocked', 'readyToTest'])
    expect(hints.readyToTest).toContain('Ready to Test')
    expect(hints.readyToTest).toContain('In Progress')
    expect(hints.readyToTest).toContain('re-discover')
  })

  it('is empty for a fully matched mapping and for a refusal', () => {
    const allMatched: ResolvedMappingDto = {
      ...resolvedDefault,
      readyToTest: { optionId: 'qa', name: 'QA', via: 'matched' },
      blocked: { optionId: 'bl', name: 'Blocked', via: 'matched' }
    }
    expect(fallbackHints(allMatched)).toEqual({})
    expect(fallbackHints(null)).toEqual({})
  })
})

describe('optionNamesForSelection', () => {
  it('maps the selected ids to display names, blank for unknown ids', () => {
    const options = [
      { id: 'o1', name: 'Todo' },
      { id: 'o2', name: 'In Progress' },
      { id: 'o3', name: 'Done' }
    ]
    const names = optionNamesForSelection(selectionFromResolved(resolvedDefault), options)
    expect(names.todo).toBe('Todo')
    expect(names.readyToTest).toBe('In Progress')
    const missing = optionNamesForSelection({ ...EMPTY_SELECTION, todo: 'nope' }, options)
    expect(missing.todo).toBe('')
  })
})
