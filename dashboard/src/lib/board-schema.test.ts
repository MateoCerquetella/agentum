// PARITY CONTRACT: assertions here MUST match crates/agentum-core/src/board_schema.rs::required_fields_for. If both fail together after a spec change, update both — never just one.

import { describe, expect, it } from 'vitest';
import { requiredFieldsFor, validateTransition } from './board-schema';

describe('board-schema parity', () => {
  it('todo requires title + lbl', () => {
    expect(requiredFieldsFor('todo')).toEqual(['title', 'lbl']);
  });

  it('doing requires title + lbl + workdir + tool + claimed_by', () => {
    expect(requiredFieldsFor('doing')).toEqual([
      'title',
      'lbl',
      'workdir',
      'tool',
      'claimed_by'
    ]);
  });

  it('done requires title + lbl + session_id_or_comment', () => {
    expect(requiredFieldsFor('done')).toEqual([
      'title',
      'lbl',
      'session_id_or_comment'
    ]);
  });

  it('custom columns passthrough — no required fields', () => {
    expect(requiredFieldsFor('blocked')).toEqual([]);
  });

  it('validateTransition into doing reports the exact 3 missing keys when only title+lbl are set', () => {
    const missing = validateTransition('doing', { title: 'x', lbl: 'feat' });
    expect(missing).toEqual(['workdir', 'tool', 'claimed_by']);
  });
});
