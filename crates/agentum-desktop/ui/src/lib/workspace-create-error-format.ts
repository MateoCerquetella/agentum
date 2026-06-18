export type WorkspaceCreateErrorDisplay = {
  title: string
  message: string
  help?: string
}

const MISSING_BASE_REF_ANCHOR = 'could not resolve a default base ref'

/** Pull the worktree name out of a git error like
 *  `Preparing worktree (checking out 'Test')` or a trailing path segment, so the
 *  friendly message can name the conflicting workspace. */
function extractWorktreeName(message: string): string | null {
  const checkingOut = message.match(/checking out '([^']+)'/i)
  if (checkingOut?.[1]) {
    return checkingOut[1]
  }
  // Fall back to the last path segment in the first quoted path.
  const quotedPath = message.match(/'([^']*\/[^'/]+)'/)
  if (quotedPath?.[1]) {
    const segment = quotedPath[1].split('/').filter(Boolean).at(-1)
    if (segment) {
      return segment
    }
  }
  return null
}

export function formatWorkspaceCreateError(error: unknown): WorkspaceCreateErrorDisplay {
  const message = error instanceof Error ? error.message : 'Failed to create worktree.'
  const lower = message.toLowerCase()

  if (lower.includes(MISSING_BASE_REF_ANCHOR)) {
    return {
      title: 'No base branch found',
      message: 'Agentum could not resolve a usable base ref for this workspace.',
      help: 'Create an initial commit (for example on main), or select an existing branch in Create From, then try again.'
    }
  }

  // `fatal: '/…/worktrees/Test' already exists` — a folder with this name is
  // already there (often a leftover from a half-deleted worktree).
  if (lower.includes('already exists')) {
    const name = extractWorktreeName(message)
    return {
      title: 'That name is already taken',
      message: name
        ? `A workspace named “${name}” already exists in this project.`
        : 'A workspace with this name already exists in this project.',
      help: 'Pick a different name, or remove the existing workspace (or its leftover folder under .claude/worktrees) and try again.'
    }
  }

  // `fatal: 'main' is already checked out at '…'` — the branch is in use by
  // another worktree; git won't check it out twice.
  if (lower.includes('is already checked out')) {
    const branch = message.match(/'([^']+)' is already checked out/i)?.[1]
    return {
      title: 'Branch already in use',
      message: branch
        ? `The branch “${branch}” is already checked out in another workspace.`
        : 'That branch is already checked out in another workspace.',
      help: 'Open the existing workspace, or create this one from a different branch.'
    }
  }

  // `fatal: invalid reference: …` / `not a valid object name` — bad base ref.
  if (lower.includes('invalid reference') || lower.includes('not a valid object name')) {
    return {
      title: 'Base branch not found',
      message: 'The branch or ref this workspace is based on doesn’t exist.',
      help: 'Choose an existing branch in Create From, or check the name and try again.'
    }
  }

  // Generic git `fatal:` — strip the noisy "Preparing worktree …\nfatal: …"
  // prefix and show just the reason.
  const fatal = message.match(/fatal:\s*(.+)/i)?.[1]?.trim()
  if (fatal) {
    return {
      title: 'Could not create workspace',
      message: fatal
    }
  }

  return {
    title: 'Could not create workspace',
    message
  }
}

export function getWorkspaceCreateErrorToastMessage(error: WorkspaceCreateErrorDisplay): string {
  return error.help ? error.title : error.message
}
