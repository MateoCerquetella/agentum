# Orca → Agentum — Análisis Competitivo

> v2 · 2026-05-25 · Fuente: `/tmp/orca` @ `1.4.28-rc.5` · Stack Orca: Electron + React + TypeScript · ~27M en `src/` (111 módulos solo en `main/`)

---

## 1. Estructura de Orca

**`src/main/`** (11M, 111 entradas) — Proceso principal de Electron. Aquí vive *todo* el dominio: un subdirectorio por agente (`claude/`, `codex/`, `cursor/`, `gemini/`, `hermes/`, `grok/`, `droid/`, `antigravity/`, `copilot/`), uno por integración Git/forge (`git/`, `github/`, `gitlab/`, `bitbucket/`, `gitea/`, `azure-devops/`), módulos de cuentas y uso por agente (`claude-accounts/`, `claude-usage/`, `codex-accounts/`, `codex-usage/`), más `agent-hooks/` (loopback HTTP que los CLIs llaman para publicar eventos de ciclo de vida), `daemon/` (supervisor de procesos node-pty), `browser/`, `computer/` (computer-use), `automations/` y `crash-reporting/`. Es el equivalente exacto del workspace Rust de agentum (`crates/agentum-{server,executor,watchdog,store,tmux}`) pero monolítico en TypeScript dentro de un proceso Electron.

**`src/renderer/src/`** (15M) — SPA React. Estructura clásica: `App.tsx`, `components/`, `hooks/`, `lib/`, `store/`, `runtime/`, `web/`. Equivalente conceptual a `dashboard/` de agentum pero ~10× más grande porque incluye el visor de diff Monaco, IDE-like sidebars, vista de PR/issues integrada, panel de cuentas por agente y el editor de configuración. La carpeta `web/` apunta a que hay un build modo browser además del Electron.

**`src/shared/`** (1.6M) — Tipos y utilidades cruzadas main↔renderer. Contiene los contratos del sistema de hooks (`agent-hook-endpoint-file.ts`, `agent-hook-listener.ts`, `agent-hook-relay.ts`, `agent-hook-types.ts`), detección de agentes (`agent-detection.ts`, `agent-process-recognition.ts`), tipos de estado (`agent-status-types.ts`), y schemas de schedules/automations. Es el análogo de `crates/agentum-core/` — el "lingua franca" entre capas. Que esté en `shared/` (no en `main/`) confirma que el frontend orquesta lógica de agentes directamente, no solo la renderiza.

---

## 2. Feature Gap

| Feature | Orca | Agentum | Prioridad |
|---|---|---|---|
| Worktree por sesión (aislamiento git nativo) | ✅ first-class | ❌ usa workdir plano | **P0** |
| Diff viewer + commit UI in-app | ✅ Monaco | ❌ delegado al usuario | **P1** |
| Integración GitHub/GitLab (PRs, issues, checks) | ✅ full + Bitbucket/Gitea/Azure parcial | ❌ ninguna | **P1** |
| Agent hooks vía HTTP loopback | ✅ richer que tmux scraping | ⚠️ parsing de pane + heurísticas | **P2** |
| Cuentas por agente (multi-login OAuth) | ✅ `*-accounts/` | ❌ usa lo que esté instalado | P3 |
| Tracking de usage/costo por agente | ✅ `*-usage/` | ⚠️ solo Claude (transcript) | P2 |
| Catálogo de agentes soportados | ~22 incluidos `droid`/`grok`/`antigravity`/`copilot` | 8 (6 first-class + 2 passthrough) | P2 |
| Automations / schedules | ✅ `automations/` + `automation-schedules` | ❌ | P3 |
| Computer-use / browser-use | ✅ `computer/`, `browser/` | ❌ | P3 |
| Settings UI | ✅ panel React | ⚠️ TOML + flags CLI | P2 |
| Crash reporting | ✅ `crash-reporting/` | ❌ | P3 |
| Distribución | Electron ~150MB+ | Binario Rust único + SPA embebida | **agentum gana** |
| Remoto real (VPS) | ⚠️ SSH relay deployado por host | ✅ daemon nativo + PWA | **agentum gana** |
| Persistencia de sesiones cross-restart | ⚠️ node-pty muere con la app | ✅ tmux sobrevive al daemon | **agentum gana** |
| Multi-user / auth / profiles | ❌ single-user desktop | ✅ Argon2 + bearer + profiles | **agentum gana** |
| Acceso móvil | App companion separada | ✅ PWA sobre el mismo origen | **agentum gana** |

---

## 3. Top 3 features a copiar

### #1 — Worktrees como unidad de sesión (P0)

**Qué:** cada sesión opcionalmente crea un `git worktree add` aislado en lugar de usar el `workdir` raw. Permite ejecutar 5 agentes en paralelo sobre la misma repo sin pisarse ramas/stash.

**Plan concreto:**
- Migración `0015_session_worktrees.sql`: añadir a `sessions` columnas `worktree_path TEXT NULL`, `worktree_branch TEXT NULL`, `worktree_base_ref TEXT NULL` (nullables → backwards-compat).
- En `crates/agentum-executor/src/lib.rs`: añadir `WorktreeSpec { base_ref, branch_name }` al `Session` o al payload de creación. Si está presente, el server (no el adapter) ejecuta `git worktree add -b <branch> <path> <base_ref>` *antes* de invocar `adapter.launch(session)`.
- Helper nuevo `crates/agentum-server/src/git.rs` (módulo pequeño, sin nueva crate): `create_worktree`, `prune_worktree`, `worktree_status` shelling out a `git`.
- Route nueva `POST /api/sessions/{id}/worktree/prune` para cleanup tras `Status::Stopped`.
- Watchdog: al transicionar a `Stopped` o `Crashed`, si `worktree_path.is_some()` y el branch no tiene commits únicos → auto-prune (opt-in via flag en sessions).
- Dashboard: en `NewSessionDialog.svelte` añadir toggle "Aislar en worktree" + input para `base_ref` (default: `HEAD`). TUI: campo equivalente en el wizard de `app.rs`.

**Esfuerzo:** ~2-3 días. **Riesgo:** medio (manejo de paths absolutos, permisos, repos sin git inicializado → degradar grácil).

### #2 — Agent hooks loopback HTTP (P2 alto, multiplica todo lo demás)

**Qué:** Orca expone un endpoint local que los agentes llaman vía `curl` para reportar eventos (`tool_started`, `awaiting_input`, `task_done`, `cost_delta`). Reemplaza/complementa el scraping de pane regex-based del watchdog — más confiable, sin falsos positivos.

**Plan concreto:**
- Nuevo route `POST /api/sessions/{id}/hook` en `crates/agentum-server/src/routes/sessions.rs`. Body: `{ kind: string, payload: json }`. Auth: token de sesión efímero (no el bearer del usuario), inyectado como `AGENTUM_HOOK_TOKEN` en el env del proceso al `launch()`.
- En `crates/agentum-executor/src/lib.rs`: añadir a `LaunchCommand.env` las vars `AGENTUM_HOOK_URL=https://127.0.0.1:8822/api/sessions/<id>/hook` + `AGENTUM_HOOK_TOKEN=<random>`. Tokens viven en memoria en `AppState` (no persistir).
- Para los agentes que soportan hooks nativos (Claude Code tiene `--hook-pre-tool-use`/`--hook-post-tool-use`): el adapter inyecta argumentos que llaman al endpoint. Para los que no: el usuario lo configura manual; el endpoint sigue ahí.
- El handler convierte el body en `Event::new("agent.hook.<kind>")` y lo emite por `state.bus` → toda la UI reacciona sin tocar `agent-watchdog`.
- Mantener el watchdog actual como fallback; los hooks son aditivos.

**Esfuerzo:** ~1-2 días. **Riesgo:** bajo (additive). **Payoff:** elimina toda la fragilidad del regex-matching de paneles cuando los agentes cambian su output.

### #3 — Diff viewer + commit UI mínimo (P1)

**Qué:** Panel en el dashboard que muestra `git status` + `git diff` de la worktree de una sesión + botón "commit" con mensaje sugerido por el agente. No reemplazar a `lazygit`; ser el complemento "leído desde el móvil".

**Plan concreto:**
- Routes nuevas (`crates/agentum-server/src/routes/git.rs`):
  - `GET /api/sessions/{id}/git/status` → `{ staged: [...], unstaged: [...], untracked: [...] }`
  - `GET /api/sessions/{id}/git/diff?path=...&staged=bool` → unified diff text
  - `POST /api/sessions/{id}/git/commit` → `{ message, paths: [...] }`
- Resuelven el cwd como `session.worktree_path ?? session.workdir`. Shell-out a `git`; sin libgit2 (mantiene el principio "sin nueva dep nativa").
- Dashboard: nuevo `SessionGitPanel.svelte` debajo de `SessionRail`. Render del diff con un componente simple (no Monaco — agentum es lightweight). `prismjs` o `shiki` opcional.
- TUI: omitir en v1 (el usuario tiene `lazygit` via la pane embebida).
- Sin permisos especiales: la auth bearer cubre todo.

**Esfuerzo:** ~3-4 días (mayoría es UI del diff). **Riesgo:** bajo si no se mete edición de archivos.

---

## 4. Conclusión

Orca y agentum compiten en el mismo espacio pero **desde extremos opuestos del stack**: Orca es un IDE desktop pesado con todo en proceso; agentum es un daemon liviano con clientes desacoplados. Los wedges defendibles de agentum (binario único, daemon remoto real, tmux para persistencia, PWA móvil, multi-user con auth) **no son copiables por Orca sin reescribir la arquitectura** — Electron no corre headless en un VPS, node-pty no sobrevive un reinicio, y la app es single-user por diseño.

Las features que sí hay que copiar son **ortogonales al stack**: worktrees, hooks HTTP y diff viewer no atan a agentum a Electron ni rompen el modelo daemon-céntrico. Empezar por **worktrees** desbloquea el caso de uso "5 agentes en la misma repo" que hoy obliga al usuario a `git clone` manual. Los **hooks HTTP** son baratos y reducen permanentemente la fragilidad del watchdog. El **diff viewer** cierra el gap "no puedo revisar lo que hizo el agente desde el teléfono".

**Lo que NO copiar:** integraciones GitHub/GitLab full-blown (alcance enorme, ROI bajo vs delegar a `gh`/`glab` CLI dentro de la pane), computer-use, browser-use, cuentas multi-OAuth (mantener "usa el agente que tengas instalado"), automations/schedules (cron del sistema basta), Monaco (peso brutal para una SPA embebida).

**Roadmap sugerido:** v0.9.0 = worktrees + hooks HTTP (juntos en una sola fase). v0.10.0 = diff viewer. Después, reevaluar contra el uso real antes de tocar nada más de la lista.
