-- Native git-worktree isolation per session.
--
-- Lets the user opt a new session into "own branch, own checkout" so
-- five agents can run on the same repo without stomping each other's
-- stash/branch. Columns are nullable for backwards compatibility:
-- existing sessions and any new session that opts out keeps the
-- pre-worktree flat-workdir behaviour.
--
--  worktree_path     — absolute path of the worktree's checkout
--                      (a sibling like `<repo>-worktrees/<branch>`).
--                      When non-NULL, this is the cwd handed to the
--                      agent at `tmux new-session` time, not `workdir`.
--  worktree_branch   — branch the worktree has checked out. Used by
--                      the prune route + future "uncommitted changes?"
--                      preflight before tearing down.
--  worktree_base_ref — ref the branch forked from (usually `HEAD` at
--                      create time, but can be `main`, a sha, a tag).
--                      Stored for provenance + future "rebase onto
--                      base" actions.
ALTER TABLE sessions ADD COLUMN worktree_path     TEXT NULL;
ALTER TABLE sessions ADD COLUMN worktree_branch   TEXT NULL;
ALTER TABLE sessions ADD COLUMN worktree_base_ref TEXT NULL;
