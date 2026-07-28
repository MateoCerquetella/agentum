export class SessionStore {
  #sessions = new Map();

  start(sessionId, accessToken) {
    this.#sessions.set(sessionId, { accessToken, active: true });
  }

  isActive(sessionId) {
    return this.#sessions.get(sessionId)?.active === true;
  }

  accessToken(sessionId) {
    return this.#sessions.get(sessionId)?.accessToken ?? null;
  }
}
