/** Shared types for dashboard sub-components. */

export type DiffLineKind = 'ctx' | 'add' | 'del' | 'blank';

export interface DiffLine {
  kind: DiffLineKind;
  num: string;
  text: string;
}

export interface DiffHunk {
  path: string;
  added: number;
  deleted: number;
  lines: DiffLine[];
}
