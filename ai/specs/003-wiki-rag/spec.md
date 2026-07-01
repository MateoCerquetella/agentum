# Spec 003 — Wiki-as-RAG for Chat + dockable Wiki panel

- **Number:** 003
- **Status:** In progress
- **Surface:** `crates/agentum-server` (RAG) + `crates/agentum-desktop/ui` (dock)
- **Author:** Mateo Cerquetella
- **Date:** 2026-07-01

## Problem

The Chat interviewer grounds itself in a **static, whole-repo dump**
(`gather_repo_context`: guide + manifests + file tree) that ignores the user's
actual question and never uses the AutoWiki — the one distilled, structured
description of the repo we already generate. And there is no way to read the
Wiki *while* chatting: it's a separate full-screen view, so you lose the
conversation to consult it.

## Goal

Retrieve the most relevant AutoWiki content for each chat question (true vector
RAG) and inject it into the interview, and let the user dock the Wiki beside the
Chat (Obsidian-style) so it's readable without leaving the conversation.

## Users / personas

Someone using the Chat screen to scope a feature against a repo that already has
a generated Wiki — they want the interviewer to *know* the repo's architecture,
and to glance at the relevant wiki page as they talk.

## Acceptance criteria

**Phase 1 — RAG core (server, no heavy deps)**
1. A wiki generate run writes a `.agentum/wiki/.embeddings.json` sidecar next to
   the pages (chunk → embed → store), wiped+rebuilt in lockstep with the pages.
2. `POST /api/wiki/reindex?workdir=` rebuilds the sidecar for an already-present
   wiki without regenerating pages.
3. On each `/api/chat` and `/api/chat/stream` turn, the top-k wiki chunks for the
   latest user message are retrieved (cosine over the sidecar) and injected into
   the system prompt as a distinct `RELEVANT WIKI` block.
4. No wiki / no sidecar / no query / model-mismatch → retrieval is skipped and
   Chat behaves exactly as today (never a hard failure).
5. `cargo test -p agentum-server --lib` green (chunker, cosine, hashing embedder,
   retrieval formatting, round-trip).

**Phase 2 — neural embeddings (server)**
6. A `candle` backend (pure Rust, no onnxruntime) behind the same `Embedder`
   trait produces real transformer embeddings (bge-small / MiniLM, 384-dim),
   model downloaded once to the app data dir and cached (offline thereafter).
7. It is the default when the model is available; falls back to the Phase-1
   baseline embedder otherwise. Sidecar records the model id; a mismatch triggers
   a rebuild rather than bad cosine.

**Phase 3 — dockable Wiki panel (desktop UI)**
8. A toggle in the Chat header opens the Wiki pinned beside the Chat (a third
   flex column), resizable via `useSidebarResize`, width persisted.
9. The docked Wiki uses the Chat's selected workspace as its `workdir` (no
   separate project rail); reuses `MarkdownPreview` (mermaid + `[[links]]` free).

## Scope & non-goals (YAGNI)

- **In:** per-repo wiki RAG (tiny corpus → brute-force cosine, no ANN); local
  offline embeddings; a Chat-hosted dock.
- **Out:** cross-repo / global knowledge base; an ANN index (sqlite-vec/HNSW);
  RAG over non-wiki files; embedding the whole repo; a standalone RAG API.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild
- `wiki::wiki_dir` (`crates/agentum-server/src/wiki.rs:39`), `parse_wiki_index`
  (`wiki.rs:58`), `WikiIndex`/`WikiPageMeta` — the corpus locator + TOC.
- `routes/wiki.rs::generate` background success path (`routes/wiki.rs`) — where a
  valid `index.json` is confirmed; the natural place to also build the sidecar.
- `routes/chat.rs::interviewer_instructions` (`chat.rs:278`) — the single
  system-prompt builder for both interactive handlers (`chat:370`,
  `chat_stream:472`); add a `wiki_context` block here.
- `hooks/useSidebarResize.ts:51` — the drag-resize primitive the RightSidebar
  uses (`deltaSign:-1` = left-edge handle → dock on the right).
- `components/harness/ChatPage.tsx` content row (`:385`) + `DrillInHeader`
  `actions` slot (`:375`) — host + toggle location for the dock.
- `components/wiki/WikiPage.tsx` + `runtime/wiki-client.ts` +
  `components/editor/MarkdownPreview` — reuse for the docked render.

### Build new
- `crates/agentum-server/src/wiki_rag.rs` — chunker, `Embedder` trait, baseline
  `HashingEmbedder`, sidecar (`WikiEmbeddingIndex`), `build_index`, `retrieve`,
  `retrieve_context` (the chat entry), `cosine`.
- `POST /api/wiki/reindex` route.
- Phase 2: `wiki_rag::candle` backend module + model fetch/cache.
- Phase 3: `WikiPage` `variant:'docked'` prop path + a ChatPage dock column +
  `ui.ts` `wikiDockOpen`/`wikiDockWidth`.

## Risks & invariants

- **Never regress Chat.** Retrieval is additive + best-effort; any miss/error →
  skip and fall back to today's behavior (mirror `gather_repo_context`'s `None`).
- **No onnxruntime in the server crate.** The desktop already links onnxruntime
  1.17.1 via `sherpa-rs`; a second (fastembed/ort) risks duplicate dylibs /
  symbol collisions. Phase 2 uses `candle` (pure Rust) to stay collision-free and
  cross-compile clean on all four release targets.
- **Sidecar tracks the pages.** It lives under `.agentum/wiki/` so the wiki's
  `remove_dir_all`-then-regenerate wipes stale vectors for free; it is gitignored
  with the rest of `wiki/`.
- **Model id gates cosine.** Comparing vectors across embedder models is
  meaningless — store the model id and skip/rebuild on mismatch.

## Harness wiring (the gate)

- **feature_list.json entries:** one per phase (rag-core / candle / dock).
- **`verify.sh` asserts:** `cargo test -p agentum-server --lib` (chunk/cosine/
  retrieval) + `cargo build`.
- **`qa.sh` asserts:** (Phase 3) the dock button opens the Wiki beside Chat and
  the panel renders a page.

## Open questions

- Phase 2 model choice: bge-small-en-v1.5 vs all-MiniLM-L6-v2 (both 384-dim) —
  decide at Phase 2 by build weight + retrieval quality on a real wiki.
- Bundle the model in the installer vs download-on-first-index (lean: download +
  cache, matching the sherpa model-catalog pattern).
