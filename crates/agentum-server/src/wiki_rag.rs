//! Vector RAG over the per-repo AutoWiki (spec 003, Phase 1).
//!
//! The AutoWiki (`wiki.rs`) is the one distilled, structured description of a repo
//! we generate. This module turns it into a retrieval corpus so the Chat
//! interviewer can pull the pages relevant to the user's *actual question* instead
//! of relying only on the static whole-repo dump in `routes::chat`.
//!
//! Pipeline: chunk each `<slug>.md` → embed each chunk → persist a
//! `.agentum/wiki/.embeddings.json` sidecar co-located with the pages (so the
//! wiki's `remove_dir_all`-then-regenerate wipes stale vectors for free, and it
//! rides the existing `.agentum/.gitignore`). At query time we embed the question
//! and brute-force cosine over the sidecar — the corpus is tiny (a handful of
//! pages, dozens–~100 chunks), so an ANN index (HNSW/sqlite-vec) would be pure
//! overhead.
//!
//! The embedding backend is behind the [`Embedder`] trait. Phase 1 ships a
//! dependency-free [`HashingEmbedder`] (signed feature hashing over words +
//! char-trigrams) so the whole pipeline is real, testable, and cross-platform
//! with zero new deps; Phase 2 adds a `candle` transformer backend behind the
//! same trait (pure Rust — deliberately NOT onnxruntime, which would collide with
//! the desktop's `sherpa-rs` onnxruntime). Vectors are only comparable within one
//! backend, so the sidecar records the model id and a mismatch is treated as
//! "skip" (a reindex rebuilds it) rather than nonsense cosine.
//!
//! Everything here is synchronous std-fs + CPU math, meant to run under
//! `tokio::task::spawn_blocking` from the async routes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::wiki::{parse_wiki_index, wiki_key, wiki_store_dir};

/// Default number of chunks injected into a chat turn. Small: a few focused
/// excerpts beat a wall of text and stay well within the context budget.
pub(crate) const DEFAULT_TOP_K: usize = 6;

/// Max chars per chunk. Wiki pages are short prose/tables; ~1k keeps a chunk to a
/// coherent section without splitting mid-thought too often.
const MAX_CHUNK_CHARS: usize = 1_000;

/// Cap on the assembled retrieval block injected into the prompt (chars).
const MAX_RETRIEVE_CHARS: usize = 6_000;

/// Dimensionality of the baseline hashing embedder.
const HASH_DIM: usize = 1_024;

// ---- embedder trait + baseline backend --------------------------------------

/// A text→vector backend. Object-safe so backends are swappable behind a
/// `Box<dyn Embedder>`. Vectors from different `id()`s are NOT comparable.
pub(crate) trait Embedder: Send + Sync {
    /// Embed a batch. Returns one `dim()`-length vector per input, same order.
    fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>>;
    /// Vector dimensionality.
    fn dim(&self) -> usize;
    /// Stable model id, persisted in the sidecar to gate cosine (a mismatch means
    /// the vectors were built by a different backend and must be rebuilt).
    fn id(&self) -> String;
}

/// The Phase-1 backend for the current build. A free function so callers don't
/// hard-code a concrete type — Phase 2 swaps in candle-if-available here.
pub(crate) fn default_embedder() -> Box<dyn Embedder> {
    Box::new(HashingEmbedder::new(HASH_DIM))
}

/// Dependency-free embedder: signed feature hashing over lowercased words and
/// their char-trigrams, L2-normalized. Not semantic like a transformer, but a
/// real vector space where cosine tracks lexical/morphological overlap — enough
/// to rank a handful of wiki pages, and a safe cross-platform baseline + fallback.
pub(crate) struct HashingEmbedder {
    dim: usize,
}

impl HashingEmbedder {
    pub(crate) fn new(dim: usize) -> Self {
        Self { dim }
    }

    fn embed_one(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0f32; self.dim];
        for word in tokenize(text) {
            // The word itself…
            accumulate(&mut v, self.dim, "w:", &word);
            // …plus its char-trigrams, so near-matches (plurals, typos, compound
            // terms) still land in shared buckets.
            let chars: Vec<char> = word.chars().collect();
            if chars.len() >= 3 {
                for w in chars.windows(3) {
                    let tri: String = w.iter().collect();
                    accumulate(&mut v, self.dim, "t:", &tri);
                }
            }
        }
        l2_normalize(&mut v);
        v
    }
}

impl Embedder for HashingEmbedder {
    fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.embed_one(t)).collect())
    }
    fn dim(&self) -> usize {
        self.dim
    }
    fn id(&self) -> String {
        format!("hash-v1-{}", self.dim)
    }
}

/// Add a signed, hashed feature into the bucket vector. Sign hashing (one hash
/// bit picks +1/−1) cancels some collisions in expectation.
fn accumulate(v: &mut [f32], dim: usize, ns: &str, token: &str) {
    let h = fnv1a64(ns.as_bytes(), token.as_bytes());
    let bucket = (h % dim as u64) as usize;
    let sign = if (h >> 63) & 1 == 0 { 1.0 } else { -1.0 };
    v[bucket] += sign;
}

/// Lowercase, split on non-alphanumeric, drop tiny tokens + a small stopword set
/// (common words add equal noise to every doc and query; dropping them sharpens
/// cosine on the terms that matter).
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_lowercase())
        .filter(|t| !is_stopword(t))
        .collect()
}

fn is_stopword(t: &str) -> bool {
    matches!(
        t,
        "the" | "and" | "for" | "are" | "but" | "not" | "you" | "with" | "this" | "that"
            | "from" | "have" | "has" | "was" | "were" | "its" | "it's" | "into" | "your"
            | "our" | "their" | "they" | "them" | "then" | "than" | "can" | "will" | "all"
            | "any" | "how" | "what" | "why" | "when" | "who" | "which" | "does" | "did"
    )
}

/// FNV-1a over `ns || token` — a small, fully deterministic hash (std's
/// `DefaultHasher` seed is not contractually stable across releases, and the
/// sidecar persists across app updates within one `id()`).
fn fnv1a64(ns: &[u8], token: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in ns.iter().chain(token) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Cosine similarity of two equal-length vectors. Inputs from this module are
/// already L2-normalized so this is a dot product; we divide by norms anyway to
/// stay correct if a caller passes un-normalized vectors.
pub(crate) fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0f32;
    let mut na = 0f32;
    let mut nb = 0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

// ---- chunking ----------------------------------------------------------------

/// One retrievable unit: a slice of a wiki page, tagged with its page + section
/// so the retrieval block can cite it and the embedding carries that context.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Chunk {
    pub slug: String,
    pub title: String,
    pub heading: String,
    pub text: String,
}

impl Chunk {
    /// The string actually embedded — the body prefixed with page + section so
    /// the vector reflects where the text lives, not just its words.
    fn embed_input(&self) -> String {
        let mut s = self.title.clone();
        if !self.heading.is_empty() {
            s.push_str(" — ");
            s.push_str(&self.heading);
        }
        s.push('\n');
        s.push_str(&self.text);
        s
    }
}

/// Split one page's markdown into chunks. Headings (`#…`) are section boundaries;
/// lines are packed up to [`MAX_CHUNK_CHARS`] within a section. Cheap and
/// markdown-aware enough for wiki prose (mermaid/code fences ride along as text).
pub(crate) fn chunk_page(slug: &str, title: &str, markdown: &str) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut heading = String::new();
    let mut cur = String::new();
    let mut cur_heading = String::new();

    for line in markdown.lines() {
        let line = line.trim_end();
        if line.starts_with('#') {
            push_chunk(&mut chunks, slug, title, &cur_heading, &cur);
            cur.clear();
            heading = line.trim_start_matches('#').trim().to_string();
            continue;
        }
        // A single line longer than the cap can't be packed — flush and hard-split
        // it into cap-sized pieces so no chunk ever blows the budget.
        if line.chars().count() > MAX_CHUNK_CHARS {
            push_chunk(&mut chunks, slug, title, &cur_heading, &cur);
            cur.clear();
            for piece in split_char_windows(line, MAX_CHUNK_CHARS) {
                push_chunk(&mut chunks, slug, title, &heading, &piece);
            }
            continue;
        }
        if cur.len() + line.len() + 1 > MAX_CHUNK_CHARS && !cur.trim().is_empty() {
            push_chunk(&mut chunks, slug, title, &cur_heading, &cur);
            cur.clear();
        }
        if cur.is_empty() {
            cur_heading = heading.clone();
        }
        cur.push_str(line);
        cur.push('\n');
    }
    push_chunk(&mut chunks, slug, title, &cur_heading, &cur);
    chunks
}

/// Split a string into `n`-char windows (char-safe, never mid-codepoint).
fn split_char_windows(s: &str, n: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    chars.chunks(n).map(|c| c.iter().collect()).collect()
}

fn push_chunk(chunks: &mut Vec<Chunk>, slug: &str, title: &str, heading: &str, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    chunks.push(Chunk {
        slug: slug.to_string(),
        title: title.to_string(),
        heading: heading.to_string(),
        text: text.to_string(),
    });
}

// ---- sidecar (the on-disk index) --------------------------------------------

/// One stored chunk + its vector. Mirrors [`Chunk`] plus `vec`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredChunk {
    pub slug: String,
    pub title: String,
    pub heading: String,
    pub text: String,
    pub vec: Vec<f32>,
}

/// The `.agentum/wiki/.embeddings.json` payload. `model` + `dim` gate cosine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WikiEmbeddingIndex {
    pub model: String,
    pub dim: usize,
    pub generated_at: u64,
    pub chunks: Vec<StoredChunk>,
}

/// Sidecar filename inside the wiki dir.
const SIDECAR: &str = ".embeddings.json";

/// Build the embedding index from an on-disk wiki (`index.json` + `<slug>.md`).
/// Synchronous — run under `spawn_blocking`. Errors on a missing/garbled index or
/// an empty corpus; the caller treats that as "no RAG", never a wiki failure.
pub(crate) fn build_index(dir: &Path, embedder: &dyn Embedder) -> anyhow::Result<WikiEmbeddingIndex> {
    let raw = std::fs::read_to_string(dir.join("index.json"))?;
    let index = parse_wiki_index(&raw)?;

    let mut chunks: Vec<Chunk> = Vec::new();
    for page in &index.pages {
        // Slugs are validated by `parse_wiki_index`, so this join can't traverse.
        let md = match std::fs::read_to_string(dir.join(format!("{}.md", page.slug))) {
            Ok(m) => m,
            Err(_) => continue, // a listed page whose file is missing → skip it
        };
        chunks.extend(chunk_page(&page.slug, &page.title, &md));
    }
    if chunks.is_empty() {
        anyhow::bail!("wiki has no chunkable content");
    }

    let inputs: Vec<String> = chunks.iter().map(Chunk::embed_input).collect();
    let vecs = embedder.embed(&inputs)?;
    if vecs.len() != chunks.len() {
        anyhow::bail!(
            "embedder returned {} vectors for {} chunks",
            vecs.len(),
            chunks.len()
        );
    }

    let stored = chunks
        .into_iter()
        .zip(vecs)
        .map(|(c, vec)| StoredChunk {
            slug: c.slug,
            title: c.title,
            heading: c.heading,
            text: c.text,
            vec,
        })
        .collect();

    Ok(WikiEmbeddingIndex {
        model: embedder.id(),
        dim: embedder.dim(),
        generated_at: now_millis(),
        chunks: stored,
    })
}

/// Persist the sidecar. Synchronous — run under `spawn_blocking`.
pub(crate) fn save_index(dir: &Path, index: &WikiEmbeddingIndex) -> anyhow::Result<()> {
    let json = serde_json::to_string(index)?;
    std::fs::write(dir.join(SIDECAR), json)?;
    Ok(())
}

/// Load the sidecar, or `None` when it's absent/garbled (both mean "no RAG").
pub(crate) fn load_index(dir: &Path) -> Option<WikiEmbeddingIndex> {
    let raw = std::fs::read_to_string(dir.join(SIDECAR)).ok()?;
    serde_json::from_str(&raw).ok()
}

// ---- retrieval ---------------------------------------------------------------

/// Rank the index against `query` with `embedder`, returning `(score, chunk)`
/// top-`k`, best first. Empty when the index was built by a different backend
/// (`model`/`dim` mismatch) — the caller falls back to no RAG (a reindex fixes it).
pub(crate) fn retrieve<'a>(
    index: &'a WikiEmbeddingIndex,
    query: &str,
    embedder: &dyn Embedder,
    k: usize,
) -> anyhow::Result<Vec<(f32, &'a StoredChunk)>> {
    if index.model != embedder.id() || index.dim != embedder.dim() {
        return Ok(Vec::new());
    }
    let qv = embedder
        .embed(&[query.to_string()])?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("embedder returned no vector for the query"))?;

    let mut scored: Vec<(f32, &StoredChunk)> = index
        .chunks
        .iter()
        .map(|c| (cosine(&qv, &c.vec), c))
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.truncate(k);
    Ok(scored)
}

/// Resolve the central wiki store dir for a LOCAL workdir (Chat retrieval always
/// runs against a local checkout). Mirrors the routes' git-identity keying so
/// retrieval finds the SAME dir the generation agent wrote to: local
/// `git remote get-url origin` → [`wiki_key`] → [`wiki_store_dir`]. A worktree or
/// a re-clone of the same repo therefore hits the one shared wiki.
fn central_wiki_dir_for_local(workdir: &str) -> Option<PathBuf> {
    let remote = std::process::Command::new("git")
        .arg("-C")
        .arg(workdir)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    wiki_store_dir(&wiki_key(remote.as_deref(), workdir)).ok()
}

/// The Chat entry point: retrieve the top wiki excerpts for `query` under
/// `workdir` and format them as an injectable prompt block, or `None` when
/// there's no wiki / no sidecar / no query / a model mismatch / nothing scores.
/// Synchronous — run under `spawn_blocking`.
pub(crate) fn retrieve_context(workdir: Option<&str>, query: &str, k: usize) -> Option<String> {
    let wd = workdir.map(str::trim).filter(|s| !s.is_empty())?;
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let dir = central_wiki_dir_for_local(wd)?;
    let index = load_index(&dir)?;
    let embedder = default_embedder();
    let hits = retrieve(&index, query, embedder.as_ref(), k).ok()?;

    // Drop non-positive matches — with L2-normalized signed-hash vectors a score
    // ≤ 0 means no meaningful overlap; injecting it would be noise.
    let mut block = String::new();
    for (_score, c) in hits.into_iter().filter(|(s, _)| *s > 0.0) {
        let heading = if c.heading.is_empty() {
            String::new()
        } else {
            format!(" › {}", c.heading)
        };
        block.push_str(&format!("### {}{}\n{}\n\n", c.title, heading, c.text));
        if block.len() >= MAX_RETRIEVE_CHARS {
            break;
        }
    }
    let block = block.trim();
    if block.is_empty() {
        return None;
    }
    Some(truncate_chars(block, MAX_RETRIEVE_CHARS))
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Char-safe truncation (never split a multibyte char).
fn truncate_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("\n…[truncated]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_drops_stopwords_and_short_tokens() {
        let toks = tokenize("The Watchdog tails a pane!!");
        assert!(toks.contains(&"watchdog".to_string()));
        assert!(toks.contains(&"tails".to_string()));
        assert!(toks.contains(&"pane".to_string()));
        assert!(!toks.contains(&"the".to_string())); // stopword
        assert!(!toks.contains(&"a".to_string())); // < 2 chars
    }

    #[test]
    fn embedder_is_deterministic_and_normalized() {
        let e = HashingEmbedder::new(HASH_DIM);
        let a = e.embed(&["the tmux watchdog".into()]).unwrap();
        let b = e.embed(&["the tmux watchdog".into()]).unwrap();
        assert_eq!(a[0], b[0]); // deterministic
        let norm: f32 = a[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "expected unit norm, got {norm}");
    }

    #[test]
    fn cosine_ranks_related_text_higher() {
        let e = HashingEmbedder::new(HASH_DIM);
        let q = &e.embed(&["how does the watchdog detect a crashed agent".into()]).unwrap()[0];
        let related =
            &e.embed(&["The watchdog tails panes and emits AgentCrashed when an agent crashes.".into()]).unwrap()[0];
        let unrelated = &e.embed(&["Tailwind color tokens for the settings theme picker.".into()]).unwrap()[0];
        assert!(
            cosine(q, related) > cosine(q, unrelated),
            "related {} should beat unrelated {}",
            cosine(q, related),
            cosine(q, unrelated)
        );
    }

    #[test]
    fn chunk_page_splits_on_headings() {
        let md = "# Overview\nintro line\n\n## Details\nmore detail here\n";
        let chunks = chunk_page("overview", "Overview", md);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].heading, "Overview");
        assert!(chunks[0].text.contains("intro line"));
        assert_eq!(chunks[1].heading, "Details");
        assert!(chunks[1].text.contains("more detail"));
    }

    #[test]
    fn chunk_page_packs_long_sections_under_cap() {
        let big_para = "word ".repeat(600); // ~3000 chars, one heading
        let md = format!("## Big\n{big_para}");
        let chunks = chunk_page("p", "P", &md);
        assert!(chunks.len() >= 2, "long section should split into multiple chunks");
        for c in &chunks {
            assert!(c.text.len() <= MAX_CHUNK_CHARS + 16, "chunk over cap: {}", c.text.len());
            assert_eq!(c.heading, "Big");
        }
    }

    fn temp_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wiki-rag-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_wiki(dir: &Path) {
        std::fs::write(
            dir.join("index.json"),
            r#"{"schemaVersion":1,"pages":[{"slug":"overview","title":"Overview"},{"slug":"watchdog","title":"Watchdog"}]}"#,
        )
        .unwrap();
        std::fs::write(dir.join("overview.md"), "# Overview\nagentum is a control plane for coding agents.\n").unwrap();
        std::fs::write(
            dir.join("watchdog.md"),
            "# Watchdog\nThe watchdog tails tmux panes and emits AgentCrashed when an agent crashes or exits.\n",
        )
        .unwrap();
    }

    #[test]
    fn build_and_retrieve_round_trips() {
        let d = temp_dir();
        write_wiki(&d);
        let e = default_embedder();
        let idx = build_index(&d, e.as_ref()).unwrap();
        assert!(idx.chunks.len() >= 2);
        assert_eq!(idx.dim, HASH_DIM);
        save_index(&d, &idx).unwrap();

        // The watchdog question should surface the watchdog page first.
        let hits = retrieve(&idx, "how does the watchdog detect a crashed agent", e.as_ref(), 5).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].1.slug, "watchdog");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn retrieve_context_formats_a_block_and_is_none_on_miss() {
        // The store is git-keyed under data_dir(), so isolate it with AGENTUM_HOME
        // and write the fixture where retrieve_context will actually look (never
        // the real data dir). AGENTUM_HOME is process-global → take the crate lock.
        let _guard = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = temp_dir();
        // SAFETY: serialised by TEST_ENV_LOCK — no other thread mutates env here.
        unsafe {
            std::env::set_var("AGENTUM_HOME", &home);
        }

        // A checkout with no git remote ⇒ path-keyed central dir. Write there.
        let workdir = temp_dir();
        let dir = wiki_store_dir(&wiki_key(None, workdir.to_str().unwrap())).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        write_wiki(&dir);
        let e = default_embedder();
        let idx = build_index(&dir, e.as_ref()).unwrap();
        save_index(&dir, &idx).unwrap();

        let block = retrieve_context(
            Some(workdir.to_str().unwrap()),
            "watchdog crashed agent detection",
            DEFAULT_TOP_K,
        )
        .expect("expected a retrieval block");
        assert!(block.contains("Watchdog"));

        // No workdir / empty query → None (graceful).
        assert!(retrieve_context(None, "x", 5).is_none());
        assert!(retrieve_context(Some(workdir.to_str().unwrap()), "   ", 5).is_none());
        // A different checkout with no wiki → None.
        let empty = temp_dir();
        assert!(retrieve_context(Some(empty.to_str().unwrap()), "anything", 5).is_none());

        // SAFETY: still under the lock; restore env before releasing it.
        unsafe {
            std::env::remove_var("AGENTUM_HOME");
        }
        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&workdir).ok();
        std::fs::remove_dir_all(&empty).ok();
    }

    #[test]
    fn retrieve_skips_on_model_mismatch() {
        let d = temp_dir();
        write_wiki(&d);
        let e = default_embedder();
        let mut idx = build_index(&d, e.as_ref()).unwrap();
        idx.model = "some-other-model".to_string();
        let hits = retrieve(&idx, "watchdog", e.as_ref(), 5).unwrap();
        assert!(hits.is_empty(), "mismatched model must yield no hits");
        std::fs::remove_dir_all(&d).ok();
    }
}
