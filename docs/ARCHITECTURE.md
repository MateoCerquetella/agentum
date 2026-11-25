# Agentum: how it is built

Agentum is a self-hosted personal and coding workspace: mail, calendar, notes, a dashboard and
the Secretary (on your own model), plus coding agents (Claude Code, Codex, OpenCode, Cursor) running on
your machines, which you watch and steer. **All Rust.** No Node, no TypeScript, no WebAssembly
framework other than Leptos.

```
browser ──HTTP + one WebSocket (/api/ws)──▶ agentum (the hub, axum + SQLite)
                                               ▲ one WebSocket each (/agentd)
                         agentd on every machine ┘  (runs the agents, files, git, terminals)
```

## Crates

| Crate | What |
|---|---|
| `crates/agentum-proto` | Shared types: threads and events (`thread.rs`), hub ↔ agentd (`machine.rs`), the web API (`api.rs`); a small dotenv reader/editor (`dotenv.rs`). |
| `crates/agentum` | The hub. Personal space (from CERQ: `mail/`, `calendar/`, `notes.rs`, `chat/`, `today/`, `widgets.rs`, `settings.rs`, `accounts.rs`, `auth.rs`, `passkey.rs`, `push.rs`, `voice.rs`…), the Code section (`code/`), the plugin host (`plugins.rs`), serving the web app (`web.rs`). |
| `crates/agentd` | The machine daemon. |
| `crates/agentum-plugin` | The plugin API (`hub` and `web` features). |
| `crates/agentum-ui` | The web kit: `api` (JSON calls), `ws` (the WebSocket), `ui` (toasts, confirm, Modal), `nav` (pages, URL params), `icons`, `fmt`, `store` (localStorage), `global()`. |
| `crates/agentum-web` | The web app (Leptos, client-side rendered, compiled to wasm32). |
| `plugins/<id>` | One crate per plugin: `src/lib.rs` (manifest), `src/hub.rs` (feature `hub`), `src/web.rs` (feature `web`). |

## The web app (Leptos 0.8, CSR)

- `src/app.rs`: sign-in gate and the shell. URL = `/<left page>[?side=<right page>&params]`, two
  panes. `src/side.rs`: the sidebar (Personal / Coding). `src/palette.rs`: ⌘K.
- `src/pages/<page>.rs` (or `src/pages/<page>/mod.rs` with submodules): `pub fn page(pane: Pane) -> AnyView`.
  Registered in `src/pages/mod.rs` (`PAGES`).
- `src/code/`: the Code page (new thread, thread view, timeline, composer) and `store.rs` (live
  machines and thread summaries).
- `src/plugins.rs`: every plugin's web part and the slot helpers.
- Styles: plain CSS in `style/**/*.css`, concatenated by `scripts/build-web.sh` into one file.
  The class names are CERQ's (same look); keep using them. Two looks: `html[data-theme="glance"|"studio"]`
  and `html[data-scheme="light"|"dark"]`. Use the CSS variables (`--bg --sidebar --widget --hl
  --border --border-strong --hi --text --base --subdue --primary --positive --negative --warm --radius`).

### Rules that bite

- **App-wide signals in a `thread_local!` must be created with `agentum_ui::global(value)`**,
  never `RwSignal::new` (that one dies with the view being built when first touched → "Tried to
  access a reactive value that has already been disposed").
- Inside `view!`, an attribute expression containing `>` must be wrapped in braces:
  `style=move || { if a > 0 { … } else { … } }` (the macro reads `>` as the end of the tag).
- Non-`Send` values (gloo timers, closures) in a `StoredValue` need `StoredValue::new_local`.
- A function that returns a view built from a reference argument: `-> impl IntoView + use<>`.
- Talk to the hub with `agentum_ui::api::{api_get, api_post, api_put, api_delete, api}`; JSON in/out,
  paths under `/api` (`api_get::<Value>("/mail/list?…")`). Live updates: `subscribe(&["app"])` and
  `on_ws(|m| if let WsOut::App { payload } = m { … })` — the personal space's events (`{"kind":"mail"}`…)
  arrive on channel `app` (CERQ's old `/api/events` SSE also still works).
- Navigation: `navigate("mail", Where::Replace, &[("msg", id)])`; params: `use_param("msg")`, `set_param`.
- Toasts: `toast("Saved.")`, `toast_bad(e.message)`; `confirm(title, body, ok, danger).await`.

## The hub's API

The personal space's JSON API is CERQ's, unchanged (see `crates/agentum/src/*` routes; the old
React app's calls are in the reference copy at `/tmp/cerq-src/web/src`). The Code section:
`/api/code/*` and `/api/ws` (types in `agentum-proto/src/api.rs`); plugins: `/api/plugins`,
`/api/p/<plugin>/<method>`; a machine request from the web: `POST /api/code/call {machine, req}`
(files, git, terminals only).

## Plugins

Like BB's: a manifest, a hub part and a web part.

- Hub part: `impl HubPlugin` — `rpc(cx, method, input)`, `on_event(cx, thread, event)`,
  `start(cx)`, `before_turn(cx, thread, text)`, `instructions(cx, thread)`. `cx` (`HubCtx`) reaches
  machines (`cx.machine(id, Req)`), threads, the plugin's own key-value storage, its settings,
  `publish` (to pages subscribed to `plugin:<id>`) and `notify` (push).
- `instructions`: what every agent should know about the plugin (Generative UI: the ```ui language;
  Secrets: ask for keys in the Secrets tab). When an agent process starts, the hub joins the
  instructions of the plugins that are on into `ThreadSpec.instructions`, and agentd adds them to the
  system prompt: Claude Code `--append-system-prompt`, Codex `developerInstructions` (thread/start
  and thread/resume), OpenCode an instruction entry (`PUT /api/experimental/session/<id>/instructions/entries/agentum`),
  Cursor (`cursor-agent acp`, no system prompt over ACP) a text block after the first message of each process.
  Every turn carries them: keep them short. Genui's (`plugins/genui/src/instructions.md`) lists exactly
  the components its web part draws; its tests evaluate the examples.
- Web part: `impl WebPlugin` — slots: `rail_tabs` (the thread's side panel), `thread_header`,
  `thread_row` (sidebar badges), `thread_hover` (hover card), `message_meta`, `composer_actions`,
  `code_fence` (```lang), `settings` (Settings → Plugins), `overlay`, `nav` (a page at `/p/<id>`),
  `css`, `themes`.
- Settings → Plugins turns each on or off (`GET/PUT /api/plugins`).
- Secrets (`plugins/secrets`): its RPC takes a thread id (never a path) and sends `Req::Secrets`
  (`keys`, `set`, `delete` on `<cwd>/.env`) to that thread's machine. agentd answers with key names
  only; `SecretsReq`'s Debug/Display leave the value out. agentd lists `"secrets"` in
  `Hello.features` and the hub sends secrets only to an agentd that does (an older one would log the
  unknown frame, value and all).

## Subscription limits

The footer's meters (and each thread's limit state) are per account: agentd's `Req::Usage` answers a
`UsageReply` with the account the agent is signed in to (Claude Code: `.claude.json`'s
`oauthAccount` organization; Codex: app-server `account/read`; Cursor: `cursor-agent status` and its
dashboard API with the CLI's sign-in). The hub keeps one `AgentLimits` per agent and account (with
the machines on it; `put_limits`, `limits_for`), asks every machine whose account it doesn't know
and one machine per known account in turn every 5 minutes (providers throttle their usage
endpoints), attributes a thread's `RateLimits` to its machine's account, and stores them in the
usage plugin's `limits`. agentd reports only the agents its hub names in the `/agentd` upgrade's
`x-agentum-agents` header (an older hub can't read a newer agent kind).

## Env files

At start, before any thread exists, each binary loads a dotenv file into its environment without
overriding what is set (`agentum_proto::dotenv::load`): the hub `$AGENTUM_ENV_FILE`, else
`/etc/agentum.env`, else `<data dir>/.env`; agentd `$AGENTD_ENV_FILE`, else
`~/.config/agentum/agentd.env`. agentd's `AGENTD_HUB/TOKEN/NAME/CA/HUB_IP/ROOTS/PROJECTS` override
agentd.toml (`Config::apply_env`); agentd drops `AGENTD_TOKEN` from its environment once the config
is read, so agents and terminals don't inherit it.

## Building and running

```
export CARGO_TARGET_DIR=$HOME/.cache/agentum-target CARGO_BUILD_JOBS=4   # shared by every checkout
cargo check -p agentum                                       # the hub
cargo check -p agentum-web --target wasm32-unknown-unknown   # the web app
scripts/build-web.sh                                         # wasm → crates/agentum-web/dist
scripts/dev-hub.sh 7101                                      # build both, run a dev hub on :7101
```

`dev-hub.sh` runs on a neutered copy of the live data (no mail sync, no pushes, read-only calendar)
and prints a session token. Browser checks: `TOKEN=… BASE=http://127.0.0.1:7101 node /tmp/agentum-test/shot.mjs /mail out.png`
(headless Chromium; prints first paint and console errors) and `node /tmp/agentum-test/eval.mjs /path '<js expr>'`.
WebKit (Safari's engine) is in `~/.cache/ms-playwright/webkit-*` for Safari checks.

## Surf: each project's browser

`plugins/browser` + `crates/agentd/src/surf.rs` + `crates/agentum/src/code/surf.rs`. A project's
browser is Steel (Chromium with a live viewer) in a Docker container on the project's machine,
`agentum-surf-<12 hex of sha256(project path)>`, with a profile volume of its own (sign-ins
survive). Both ports (Steel's API/viewer → 3000, Chromium's DevTools → 9223) are published on
the machine's **127.0.0.1 only**; nothing is opened to the network.

- Agents: agentd gives an agent starting in that project `AGENTUM_SURF_CDP=ws://127.0.0.1:<api>/`
  (Playwright's `connectOverCDP`; Steel's own CDP proxy, stable across restarts) and
  `AGENTUM_SURF_DEVTOOLS=http://127.0.0.1:<cdp>`; the plugin's `instructions` say how to use it
  and that the person types passwords in the viewer, never in the chat.
- The person: the hub serves the viewer at `/api/p/browser/view/<machine>/<id>/v1/sessions/debug`
  (an /api route: the session cookie is checked). agentd fetches the page (`SurfReq::Get`; only that
  path) and the hub rewrites Steel's `ws://127.0.0.1:<port>` to itself; the screencast socket
  (`…/v1/sessions/cast`, `SurfReq::Open`) is relayed message by message as `Sock` frames over
  agentd's own connection. Behind nginx, `/api/p/browser/view/` needs a WebSocket location
  (`deploy/nginx-locations.conf`).
- Life: a container that exists is the project's browser being on. Unused for `idle_hours`
  (no live agent in the project, no open viewer) it is `docker stop`ped; the project's next agent
  starts it again. The person's Stop removes the container (the profile volume stays).
