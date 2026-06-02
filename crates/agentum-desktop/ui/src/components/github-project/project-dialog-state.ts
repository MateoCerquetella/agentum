type RepoBackedProjectDialogState = {
  repoId: string
}

type SlugProjectDialogState = {
  origin: {
    owner: string
    repo: string
  }
}

type RepoNotInAgentumDialogState = {
  owner: string
  repo: string
}

type LookupSlug = (slug: string) => readonly unknown[]

function hasRepoMatch(lookupSlug: LookupSlug, owner: string, repo: string): boolean {
  return lookupSlug(`${owner}/${repo}`).length > 0
}

export function resolveRepoBackedProjectDialogState<T extends RepoBackedProjectDialogState>(
  dialog: T | null,
  liveRepoIds: ReadonlySet<string>
): T | null {
  if (dialog && !liveRepoIds.has(dialog.repoId)) {
    return null
  }
  return dialog
}

export function resolveMissingRepoProjectDialogState<
  TSlugDialog extends SlugProjectDialogState,
  TRepoNotInAgentum extends RepoNotInAgentumDialogState
>(args: {
  slugIndexReady: boolean
  slugDialog: TSlugDialog | null
  repoNotInAgentum: TRepoNotInAgentum | null
  lookupSlug: LookupSlug
}): {
  slugDialog: TSlugDialog | null
  repoNotInAgentum: TRepoNotInAgentum | null
} {
  const { lookupSlug, repoNotInAgentum, slugDialog, slugIndexReady } = args
  if (!slugIndexReady) {
    return { slugDialog, repoNotInAgentum }
  }
  return {
    slugDialog:
      slugDialog && hasRepoMatch(lookupSlug, slugDialog.origin.owner, slugDialog.origin.repo)
        ? null
        : slugDialog,
    repoNotInAgentum:
      repoNotInAgentum && hasRepoMatch(lookupSlug, repoNotInAgentum.owner, repoNotInAgentum.repo)
        ? null
        : repoNotInAgentum
  }
}
