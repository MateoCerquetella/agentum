# HTTP API

All JSON. Auth via `Authorization: Bearer <token>` (single token in
`$XDG_DATA_HOME/agentum/auth_token` on first run, mode 0600).
Bearer-token-only — no hash-based scheme, no OAuth, no multi-user.
Rotation = `agentum auth rotate` (overwrites the file with a new random
32-byte URL-safe token).

## Sessions

| Method | Path                              | Body / Query                            | Returns |
|--------|-----------------------------------|-----------------------------------------|---------|
| GET    | `/api/sessions`                   | `?status=running`                       | `Session[]` |
| POST   | `/api/sessions`                   | `{name, workdir, tool, model?, flags?}` | `Session` |
| GET    | `/api/sessions/:id`               | —                                       | `Session` |
| PATCH  | `/api/sessions/:id`               | partial                                 | `Session` |
| DELETE | `/api/sessions/:id`               | —                                       | 204 |
| POST   | `/api/sessions/:id/start`         | —                                       | `Session` |
| POST   | `/api/sessions/:id/stop`          | —                                       | `Session` |
| POST   | `/api/sessions/:id/send`          | `{text, keys?, append_enter?}`          | 204 |
| GET    | `/api/sessions/:id/peek?lines=30` | —                                       | `{lines: string[]}` |
| WS     | `/api/sessions/:id/stream`        | upgrade                                 | binary frames of pane bytes |

## Notes / Channels / Messages

Conventional REST per the data model.

## Server

- `GET /api/health` → `{version, uptime, sessions_running, db_size_mb}`
- `GET /api/version`
- `GET /api/cert` → self-signed cert PEM (cert-server on `:8823`)

## Events stream

- `WS /api/events` → broadcast bus (`session.started`, `watchdog.compact`, etc.)
