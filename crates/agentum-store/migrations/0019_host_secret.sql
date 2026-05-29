-- SSH password auth for hosts.
--
-- Adds a `secret` column holding the SSH password for hosts whose
-- `auth_kind = 'password'`. The daemon feeds it to `ssh` via `sshpass`.
-- Key/agent hosts leave it NULL. Stored at rest on the local daemon's
-- SQLite DB only — never sent to remote machines or other clients.

ALTER TABLE hosts ADD COLUMN secret TEXT;
