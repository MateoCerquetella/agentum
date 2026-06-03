// Why: single source of truth for the commit trailer Agentum appends when the
// "Agentum Attribution" toggle (`enableGitHubAttribution`) is on. Used by both
// the terminal git/gh shim and the AI commit-message generator so the two
// code paths agree on the exact string.

export const AGENTUM_GIT_COMMIT_TRAILER = 'Co-authored-by: Agentum <mateocerquetella@hotmail.com>'
