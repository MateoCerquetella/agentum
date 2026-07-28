# Agentum Jira OAuth broker

This directory deploys Agentum's self-hostable Jira Cloud OAuth broker behind an automatic-TLS Caddy reverse proxy. It is infrastructure you operate; the repository does not contain a client ID, client secret, live domain, or hosted Agentum endpoint.

The broker implements the desktop contract at:

- `POST /v1/jira/oauth/start`
- `GET /v1/jira/oauth/callback`
- `POST /v1/jira/oauth/redeem`
- `POST /v1/jira/oauth/refresh`
- `GET /healthz`

It requests exactly `read:jira-work`, `write:jira-work`, and `offline_access`. Pending state, authorization codes, access tokens, refresh tokens, and bounded replay responses remain in process memory. SQLite contains only the device public key, a SHA-256 refresh-token digest, credential revision, and timestamps. The service does not request, proxy, or persist Jira issue data.

## Atlassian setup

1. Create one OAuth 2.0 (3LO) integration in the Atlassian developer console. Add the Jira API and the three scopes above.
2. Register exactly `https://BROKER_DOMAIN/v1/jira/oauth/callback` as its callback URL. Atlassian requires the token-exchange `redirect_uri` to match this value.
3. Put the client secret in an absolute host file outside this repository. Make it owner-only where the platform permits; the container receives it as a read-only `/run/secrets` mount.
4. Copy `.env.example` to an operator-controlled environment file, replace every placeholder, then run:

   ```sh
   docker compose --env-file /secure/host/path/agentum-jira-broker.env \
     -f deploy/jira-oauth-broker/compose.yaml up --build -d
   ```

5. Configure each Agentum installation with `AGENTUM_JIRA_OAUTH_BROKER_URL=https://BROKER_DOMAIN/` and reconnect Jira.

Use an actual public DNS record. Ports 80 and 443 must reach Caddy for certificate issuance; port 8787 must not be exposed. Keep access logs disabled or redact the `code` and `state` query parameters before enabling them. Back up the broker volume as sensitive integrity metadata even though it contains no recoverable token.

## Fail-closed behavior

Startup fails if the public URL is not credential-free HTTPS, the database path is relative/linked/weakly owned, the client secret is absent or unsafe, the service binds beyond loopback without explicit TLS-proxy acknowledgement, or another process already owns the database. The image runs as UID/GID 10001 with no Linux capabilities, a read-only root filesystem, a private data volume, and no public broker port. The proxy serves only the configured host with TLS and does not produce OAuth callback access logs.

The broker intentionally runs as one replica per SQLite volume. This makes rotating-refresh compare-and-swap unambiguous. A lost in-memory authorization flow expires after 15 minutes and must be restarted; no code or token is recovered from disk.

## Verification

From the repository root:

```sh
cargo test -p agentum-jira-broker --all-targets
cargo clippy -p agentum-jira-broker --all-targets -- -D warnings
cargo build --locked --release -p agentum-jira-broker
docker compose --env-file /secure/host/path/agentum-jira-broker.env \
  -f deploy/jira-oauth-broker/compose.yaml config --quiet
```

The deterministic tests use a local mock Atlassian server. They cover the complete start/callback/redeem/refresh contract, exact scopes, device proof, callback and redemption replay, rotating refresh CAS/replay, multi-site normalization, malicious site URLs, expiry, symlink refusal, and plaintext-token absence from SQLite. A live Atlassian integration still requires operator-owned credentials and DNS; this repository does not claim that hosted infrastructure exists.
