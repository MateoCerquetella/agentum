export type PreflightIssue = {
  id: string
  title: string
  description: string
  fixLabel: string
  fixUrl: string
}

export function getPreflightIssues(status: {
  git: { installed: boolean }
  gh: { installed: boolean; authenticated: boolean }
}): PreflightIssue[] {
  const issues: PreflightIssue[] = []

  if (!status.git.installed) {
    issues.push({
      id: 'git',
      title: 'Git is not installed',
      description: 'Git is required for Git projects, source control, and workspace management.',
      fixLabel: 'Install Git',
      fixUrl: 'https://git-scm.com/downloads'
    })
  }

  if (!status.gh.installed) {
    issues.push({
      id: 'gh',
      title: 'GitHub CLI is not installed',
      description: 'Agentum uses the GitHub CLI (gh) to show pull requests, issues, and checks.',
      fixLabel: 'Install GitHub CLI',
      fixUrl: 'https://cli.github.com'
    })
  } else if (!status.gh.authenticated) {
    issues.push({
      id: 'gh-auth',
      title: 'GitHub CLI is not authenticated',
      description: 'Run "gh auth login" in a terminal to connect your GitHub account.',
      fixLabel: 'Learn more',
      fixUrl: 'https://cli.github.com/manual/gh_auth_login'
    })
  }

  return issues
}
