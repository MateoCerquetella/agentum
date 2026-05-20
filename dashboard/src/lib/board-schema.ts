// SOURCE OF TRUTH: crates/agentum-core/src/board_schema.rs::required_fields_for.
// Keep in sync. The server is authoritative — a drift here degrades UX (the
// form lets through a payload the server then rejects) but never corrupts
// data: the dashboard reacts to the 400's {missing, status} shape.

/// Field keys the server uses in its `missing[]` payload. The
/// `session_id_or_comment` entry is synthetic — it represents the `done`
/// transition's OR-clause (either `session_id IS NOT NULL` OR `>=1 row in
/// board_comments`). The client can't know about existing comments
/// authoritatively (no per-card "did anyone comment" probe), so the
/// dialog only checks `session_id` for the `done` gate and falls back
/// to the server's 400 + missing[] hint if the comment fallback would
/// have rescued the row.
export type RequiredField =
  | 'title'
  | 'lbl'
  | 'workdir'
  | 'tool'
  | 'claimed_by'
  | 'session_id_or_comment';

/// Empty array => no gate (custom columns passthrough).
export function requiredFieldsFor(status: string): RequiredField[] {
  switch (status) {
    case 'todo':
      return ['title', 'lbl'];
    case 'doing':
      return ['title', 'lbl', 'workdir', 'tool', 'claimed_by'];
    case 'done':
      return ['title', 'lbl', 'session_id_or_comment'];
    default:
      return [];
  }
}

/// Snapshot the validator needs. Mirrors the Rust `TransitionCtx`.
export interface TransitionCtx {
  title?: string | null;
  lbl?: string | null;
  workdir?: string | null;
  tool?: string | null;
  claimed_by?: string | null;
  session_id?: string | null;
  /** Only available authoritatively on the server. Client-side this stays
   *  false; the server-side 400 carries the truth when the dialog needs
   *  to react. */
  has_comment?: boolean;
}

function isSet(v: string | null | undefined): boolean {
  return typeof v === 'string' && v.trim().length > 0;
}

/// Mirrors `agentum_core::validate_transition`. Returns the list of
/// missing keys for the target column, or an empty array on pass.
/// Custom columns (anything outside the matrix) always pass.
export function validateTransition(
  targetStatus: string,
  ctx: TransitionCtx
): RequiredField[] {
  const required = requiredFieldsFor(targetStatus);
  if (required.length === 0) return [];

  const missing: RequiredField[] = [];
  for (const field of required) {
    let present = false;
    switch (field) {
      case 'title':
        present = isSet(ctx.title);
        break;
      case 'lbl':
        present = isSet(ctx.lbl);
        break;
      case 'workdir':
        present = isSet(ctx.workdir);
        break;
      case 'tool':
        present = isSet(ctx.tool);
        break;
      case 'claimed_by':
        present = isSet(ctx.claimed_by);
        break;
      case 'session_id_or_comment':
        // Comment-existence isn't reachable from the client. Server
        // remains authoritative for the OR-clause's right side.
        present = isSet(ctx.session_id) || ctx.has_comment === true;
        break;
    }
    if (!present) missing.push(field);
  }
  return missing;
}

/// Display-friendly label for a `RequiredField`. Used by the dialog's
/// "Required for *<status>*" hint and the snap-back toast.
export function requiredFieldLabel(field: RequiredField): string {
  switch (field) {
    case 'title':
      return 'title';
    case 'lbl':
      return 'label';
    case 'workdir':
      return 'workdir';
    case 'tool':
      return 'agent';
    case 'claimed_by':
      return 'claim';
    case 'session_id_or_comment':
      return 'session or a comment';
  }
}

/// Parse the server's `400 { missing, status }` body. Returns `null`
/// when the body isn't the expected shape — caller should fall back to
/// the generic error path.
export interface GateRejection {
  missing: RequiredField[];
  status: string;
}

export function parseGateRejection(body: unknown): GateRejection | null {
  if (typeof body !== 'object' || body === null) return null;
  const b = body as Record<string, unknown>;
  const missing = b.missing;
  const status = b.status;
  if (!Array.isArray(missing) || typeof status !== 'string') return null;
  // Only adopt known keys — defensive against the server adding a new
  // synthetic field before the dashboard catches up.
  const filtered = missing.filter(
    (k): k is RequiredField =>
      k === 'title' ||
      k === 'lbl' ||
      k === 'workdir' ||
      k === 'tool' ||
      k === 'claimed_by' ||
      k === 'session_id_or_comment'
  );
  return { missing: filtered, status };
}
