import assert from "node:assert/strict";
import test from "node:test";

import { SessionStore } from "../src/session-store.js";

test("a newly started customer session is active", () => {
  const sessions = new SessionStore();
  sessions.start("session-1", "access-token-1");

  assert.equal(sessions.isActive("session-1"), true);
  assert.equal(sessions.accessToken("session-1"), "access-token-1");
});
