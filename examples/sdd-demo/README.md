# SDD demo shop

This tiny repository fixture is the release-gate project for Agentum SDD.
It models active customer sessions and refresh-token rotation with Node's
built-in test runner.

Use it through **New Spec** with this goal:

> Refresh access tokens without interrupting active sessions.

The checked-in fixture intentionally contains no `.agentum` directory and no
agent-provider configuration. Tests copy it into a temporary Git repository.
Canceling the unsaved New Spec draft leaves that repository byte-for-byte
unchanged. Saving creates `.agentum` only in Agentum's external authoritative
worktree, never in this source checkout.

Run the fixture tests with:

```sh
npm test
```
