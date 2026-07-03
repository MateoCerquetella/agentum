//! `POST /api/chat` — the Socratic feature-intake chat behind the Mission
//! Control "Chat" screen.
//!
//! This replaces the old one-shot "describe → create an issue" form with a real
//! multi-turn conversation: the model interviews the user to flesh out a feature
//! and, once it's clear, proposes a task breakdown the user can confirm and file
//! into their tracker (GitHub Issues or Linear).
//!
//! **Auth:** prefers an explicit `ANTHROPIC_API_KEY` (a real `sk-ant-api…` key —
//! clean, terms-safe, pay-per-token); otherwise falls back to the user's Claude
//! Code OAuth token (the `sk-ant-oat…` credential `usage.rs` already reads for
//! plan stats) — the user explicitly opted into reusing their `claude` login.
//! The API-key path is the robust default; the OAuth path is zero-setup.
//!
//! **OAuth gotcha (load-bearing):** an OAuth (subscription) token is only
//! accepted by `/v1/messages` when the request presents the Claude Code identity.
//! We send `anthropic-beta: oauth-2025-04-20` + the Claude Code user-agent AND
//! lead the `system` array with the exact Claude Code identity block; the actual
//! interviewer instructions follow in a second block. Dropping the identity block
//! makes Anthropic reject the token (401, "only authorized for Claude Code").
//!
//! `/api/chat` is **non-streaming** (request → full reply); `/api/chat/stream`
//! proxies Anthropic's token-by-token SSE through to the desktop and supports
//! **extended thinking** (the reasoning is streamed as `thinking` deltas). Both
//! accept an optional `model` override and share the auth/system/sanitize logic.

use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::post;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::error::ApiError;
use crate::task_sink::{NewFeature, SinkCtx, TaskSink};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/chat", post(chat))
        // Token-streaming variant of `/api/chat`: re-emits Anthropic's SSE deltas
        // (answer text + extended-thinking) as our own compact `{type,text}`
        // events so the desktop renders the reply live, reasoning included.
        .route("/api/chat/stream", post(chat_stream))
        // Spec 003: return the extracted feature plan as an editable DRAFT without
        // filing anything — the UI shows it, the user edits/regenerates, then
        // `/api/chat/issues` files the (edited) plan verbatim.
        .route("/api/chat/issues/preview", post(chat_issues_preview))
        .route("/api/chat/issues", post(chat_issues))
}

const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";
const CLAUDE_CODE_USER_AGENT: &str = "claude-code/2.1.0";
/// The identity block an OAuth token requires (see module docs). Must be exact.
const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
/// Default interview model — a fast, capable Sonnet for back-and-forth.
const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// `max_tokens` for a plain (non-thinking) chat reply — interview turns are short.
const MAX_TOKENS_REPLY: u32 = 1024;
/// Extended-thinking reasoning budget. Anthropic requires `budget_tokens >= 1024`
/// and `max_tokens > budget_tokens` — see [`MAX_TOKENS_THINKING`].
const THINKING_BUDGET_TOKENS: u32 = 2048;
/// `max_tokens` when thinking is on: must exceed the thinking budget (it caps
/// reasoning **plus** answer), so leave headroom above [`THINKING_BUDGET_TOKENS`].
const MAX_TOKENS_THINKING: u32 = 8192;
/// Shown when neither credential is present — names BOTH recovery paths.
const NO_CREDS_MSG: &str = "No LLM credentials for chat: set ANTHROPIC_API_KEY, or sign in to Claude (run `claude` once) so the chat can use your login.";

/// How the chat authenticates to Anthropic.
enum Auth {
    /// A real `sk-ant-api…` key (env `ANTHROPIC_API_KEY`) — clean API billing.
    ApiKey(String),
    /// The Claude Code subscription OAuth token (`sk-ant-oat…`).
    Oauth(String),
}

/// Resolve chat credentials: prefer an explicit `ANTHROPIC_API_KEY` (terms-safe,
/// pay-per-token); else fall back to the Claude Code OAuth token the user is
/// already signed in with. `None` when neither is available.
fn resolve_auth() -> Option<Auth> {
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
        let k = k.trim();
        if !k.is_empty() {
            return Some(Auth::ApiKey(k.to_string()));
        }
    }
    crate::usage::read_claude_oauth_token().map(Auth::Oauth)
}

#[derive(Deserialize)]
struct ChatMessage {
    /// "user" | "assistant".
    role: String,
    content: String,
}

/// Which intake experience the Chat composer picked for THIS turn (spec 008 F2).
/// Absent ⇒ [`IntakeMode::Fast`], so old clients and the Fast button stay
/// byte-identical on the wire. `snake_case` ⇒ the wire values are `"fast"` /
/// `"socratic"`.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
enum IntakeMode {
    /// Today's single-prompt interviewer — one system prompt, no staging.
    Fast,
    /// The staged five-pass Socratic interview (WHO → WHAT → WHY → done → risks).
    Socratic,
}

#[derive(Deserialize)]
struct ChatRequest {
    /// Full turn history, oldest first (user/assistant only — the server owns
    /// the system prompt).
    messages: Vec<ChatMessage>,
    /// Optional repo context to ground the interview.
    #[serde(default)]
    workdir: Option<String>,
    #[serde(default)]
    repo_slug: Option<String>,
    /// Optional model override.
    #[serde(default)]
    model: Option<String>,
    /// Enable extended thinking (streaming route only). The reasoning is streamed
    /// to the client as `thinking` deltas before the answer.
    #[serde(default)]
    thinking: Option<bool>,
    /// Spec 008 F2: which intake this turn uses — `"fast"` (default, today's
    /// single-prompt interviewer) or `"socratic"` (the staged five-pass
    /// interview). Absent ⇒ Fast, so the Fast path and old clients are unchanged.
    #[serde(default)]
    mode: Option<IntakeMode>,
    /// Spec 008 F2: the Socratic pass (1..=5), clamped server-side. Meaningful
    /// only when `mode == socratic`; the CLIENT owns advancement (one pass per
    /// user turn) so the server stays a pure `(mode, stage) → prompt` function
    /// (D-B/D1) with NO stage state of its own.
    #[serde(default)]
    stage: Option<u8>,
}

#[derive(Serialize)]
struct ChatResponse {
    /// Always "assistant".
    role: &'static str,
    content: String,
}

/// Total budget (chars) for the repo+harness snapshot inlined into the system
/// prompt. Generous on purpose — the spec must fit the ACTUAL repo, so we want
/// (near-)full context: the whole guide + the whole file tree for normal repos.
/// The cap (~22k tokens) only clips a pathological monorepo so it can't blow the
/// 200k window. ~90k chars.
const CONTEXT_BUDGET: usize = 90_000;
/// Per-section caps inside [`CONTEXT_BUDGET`] — sized to hold a full CLAUDE.md /
/// AGENTS.md guide, the full harness contract, and the root manifests.
const GUIDE_BUDGET: usize = 40_000;
const HARNESS_AGENTS_BUDGET: usize = 20_000;
const FEATURE_LIST_BUDGET: usize = 12_000;
const MANIFEST_BUDGET: usize = 8_000;
/// Cap on the git-tracked file tree (lines). High enough to be the full tree for
/// a normal repo; bounds a huge monorepo.
const TREE_MAX_FILES: usize = 1_500;

/// Char-safe truncation with a marker (byte slicing would split a multi-byte
/// char). Returns `s` untouched when already within `max`.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("\n…[truncated]");
    out
}

/// Read the first existing, non-empty file among `candidates` (relative to
/// `root`), truncated to `budget`. Returns `(name, content)`.
fn read_first_file(
    root: &std::path::Path,
    candidates: &[&str],
    budget: usize,
) -> Option<(String, String)> {
    for name in candidates {
        if let Ok(content) = std::fs::read_to_string(root.join(name)) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                return Some(((*name).to_string(), truncate_chars(trimmed, budget)));
            }
        }
    }
    None
}

/// The git-tracked file tree (so the interviewer knows what already exists),
/// capped at [`TREE_MAX_FILES`]. `None` when `root` isn't a git repo.
fn git_tracked_tree(root: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("ls-files")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let total = text.lines().count();
    if total == 0 {
        return None;
    }
    let mut joined = text
        .lines()
        .take(TREE_MAX_FILES)
        .collect::<Vec<_>>()
        .join("\n");
    if total > TREE_MAX_FILES {
        joined.push_str(&format!("\n…(+{} more files)", total - TREE_MAX_FILES));
    }
    Some(joined)
}

/// Read a real snapshot of the selected workspace so the interviewer grounds its
/// questions and the task breakdown in the ACTUAL repo + harness — not blind
/// Q&A. This is agentum's whole point: agents work with repo context. Reads
/// (best-effort, all LOCAL — Chat never SSHes): the repo guide
/// (CLAUDE.md/AGENTS.md), the `.harness/` contract (AGENTS.md + the feature
/// backlog), and a git-tracked file tree. A missing/remote/empty workdir → None.
pub(crate) fn gather_repo_context(workdir: Option<&str>) -> Option<String> {
    let wd = workdir.map(str::trim).filter(|s| !s.is_empty())?;
    let root = std::path::Path::new(wd);
    if !root.is_dir() {
        return None;
    }

    let mut out = String::new();

    // The curated repo guide — CLAUDE.md is the codebase guide; AGENTS.md the
    // agent-instructions equivalent; README a fallback.
    if let Some((name, body)) =
        read_first_file(root, &["CLAUDE.md", "AGENTS.md", "README.md"], GUIDE_BUDGET)
    {
        out.push_str(&format!("## Repo guide ({name})\n{body}\n\n"));
    }

    // The harness contract — so the breakdown fits the verification-gated
    // pipeline: the harness AGENTS.md + the current feature backlog.
    if root.join(".harness").is_dir() {
        if let Some((_, body)) =
            read_first_file(root, &[".harness/AGENTS.md"], HARNESS_AGENTS_BUDGET)
        {
            out.push_str(&format!("## .harness/AGENTS.md\n{body}\n\n"));
        }
        if let Some((_, body)) =
            read_first_file(root, &[".harness/feature_list.json"], FEATURE_LIST_BUDGET)
        {
            out.push_str(&format!(
                "## .harness/feature_list.json (current backlog)\n{body}\n\n"
            ));
        }
    }

    // Root build manifests — so the spec imitates the real stack + deps.
    let mut manifests = String::new();
    for name in [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "Gemfile",
        "pom.xml",
        "build.gradle",
        "composer.json",
        "requirements.txt",
        "tsconfig.json",
    ] {
        if let Some((n, body)) = read_first_file(root, &[name], MANIFEST_BUDGET) {
            manifests.push_str(&format!("### {n}\n{body}\n\n"));
        }
    }
    if !manifests.is_empty() {
        out.push_str("## Root manifests\n");
        out.push_str(&manifests);
    }

    // The file tree so it can reference real files/areas.
    if let Some(tree) = git_tracked_tree(root) {
        out.push_str(&format!("## Repo file tree (git-tracked)\n{tree}\n"));
    }

    let out = truncate_chars(out.trim(), CONTEXT_BUDGET);
    if out.is_empty() { None } else { Some(out) }
}

/// The grounding blocks BOTH intake modes prepend (spec 008 F2): the
/// slug/workdir context line, the repo+harness snapshot block with its matching
/// access rule, and the semantically-retrieved wiki block. Extracted VERBATIM
/// from `interviewer_instructions` so Fast and every Socratic pass ground
/// identically — the block strings are byte-for-byte the same in both, which is
/// what keeps the Fast byte-identical pin (AC 6) honest while Socratic reuses the
/// same context. Returns `(ctx, repo_block, access_rule, wiki_block)`.
fn intake_grounding_blocks(
    workdir: Option<&str>,
    repo_slug: Option<&str>,
    repo_context: Option<&str>,
    wiki_context: Option<&str>,
) -> (String, String, &'static str, String) {
    let mut ctx = String::new();
    if let Some(slug) = repo_slug {
        ctx.push_str(&format!("\nThe user's GitHub repo is `{slug}`."));
    }
    if let Some(wd) = workdir {
        ctx.push_str(&format!("\nThe project lives at `{wd}`."));
    }

    // The repo + harness snapshot (when a local workspace is selected) plus the
    // matching access rule — grounded when present, honest-blind when not.
    let (repo_block, access_rule) = match repo_context {
        Some(c) => (
            format!(
                "\n\n=== REPO & HARNESS CONTEXT (a real snapshot of the user's project — USE IT) ===\n\
{c}\n=== END CONTEXT ===\n"
            ),
            "- You HAVE the repo + harness snapshot above (the guide, the .harness/ contract, and the \
file tree). GROUND every question and the final breakdown in it: reference real files, modules, and \
existing patterns; fit the project's architecture and (if present) its harness pipeline. Don't ask \
about anything the snapshot already answers. It is a STATIC snapshot — you can't run commands or read \
files beyond it, so never claim to have executed anything.",
        ),
        None => (
            String::new(),
            "- You have no repo snapshot for this chat (no local workspace selected). Work from what \
the user tells you and don't claim to have inspected the project.",
        ),
    };

    // Query-relevant excerpts from the project's AutoWiki (spec 003 RAG), when a
    // wiki exists and something scored. A distinct block from the static snapshot
    // above: this is the retrieved, most-relevant slice for THIS question.
    let wiki_block = match wiki_context {
        Some(w) => format!(
            "\n\n=== RELEVANT WIKI (excerpts from the project's generated wiki, semantically \
retrieved for the user's latest message — prefer these as ground truth about the repo) ===\n\
{w}\n=== END WIKI ===\n"
        ),
        None => String::new(),
    };

    (ctx, repo_block, access_rule, wiki_block)
}

/// The interviewer instructions (the second `system` block). Kept separate from
/// the Claude Code identity block so the identity stays byte-exact. When a
/// `repo_context` snapshot is present the interviewer is told to GROUND
/// everything in it (agentum's philosophy: agents work with repo context); when
/// absent (no local workspace) it falls back to honest blind Q&A.
///
/// Spec 008 F2: this is the **Fast** intake prompt, kept byte-identical (the
/// grounding assembly moved into [`intake_grounding_blocks`] with no change to
/// the emitted string — pinned by `build_intake_instructions_fast_*`).
fn interviewer_instructions(
    workdir: Option<&str>,
    repo_slug: Option<&str>,
    repo_context: Option<&str>,
    wiki_context: Option<&str>,
) -> String {
    let (ctx, repo_block, access_rule, wiki_block) =
        intake_grounding_blocks(workdir, repo_slug, repo_context, wiki_context);

    // "Preview issues" below is the UI button label (ChatPage.tsx composer
    // strip) — if that button is renamed again, rename it here too, or the
    // model directs users at a button that doesn't exist.
    format!(
        "You are running inside agentum (a control plane for AI coding agents) as the \
feature-intake interviewer on the Chat screen.{ctx}{repo_block}{wiki_block}\n\n\
Your job: through a short Socratic conversation, help the user turn a rough idea into a \
clear, buildable feature THAT FITS THIS REPO, then propose a concrete task breakdown for \
their issue tracker.\n\n\
Rules:\n\
- Ask ONE focused clarifying question at a time (two only if tightly related). Keep each \
turn short and concrete, like a sharp staff engineer who knows this codebase — no filler, \
no \"great question!\".\n\
- Write like a person, not a product brief: plain sentences, no marketing adjectives, no \
\"This feature will empower/streamline…\" openers, no bullet lists where a sentence does, \
no restating the user's words back as filler, and no closing summaries of what you just \
said. Vary how you phrase things; if a template is creeping in, break it.\n\
- Cover only what's genuinely unclear: the problem and who it's for, the desired outcome, \
scope boundaries (in/out), hard constraints, and acceptance criteria. Never re-ask what \
the user — or the repo context — already answers.\n\
- When the feature is defined well enough to build, STOP asking questions and propose a \
breakdown: a one-line feature title, then exactly as many concrete tasks as the scope \
needs — a trivial fix is ONE task, a small feature two or three, and only a genuinely \
broad feature more. Never pad to a fixed count. Each task is an issue-style title plus \
one sentence of detail, pointing at the real files/areas it touches. Then tell \
the user to click the \"Preview issues\" button below the chat to review and file them.\n\
{access_rule}\n\
- You do not create the issues yourself, and no other agent will: the \"Preview issues\" \
button opens a review of the drafted issues, and confirming there files them directly. \
When the user is ready, point them at that button — never tell them to \"confirm with \
the system\" or that someone else will take it from there."
    )
}

/// Route the chat intake to the right system prompt (spec 008 F2). `Fast` is
/// [`interviewer_instructions`] VERBATIM (the byte-identical pin, AC 6 — the
/// router only delegates); `Socratic` is the per-stage single-topic pass. Both
/// share the same grounding blocks and both converge on `compose_issue_body` at
/// "Preview issues" (D8) — the mode changes ONLY the questioning, never the
/// credential gate (which runs upstream) nor the draft path.
fn build_intake_instructions(
    mode: IntakeMode,
    stage: u8,
    workdir: Option<&str>,
    repo_slug: Option<&str>,
    repo_context: Option<&str>,
    wiki_context: Option<&str>,
) -> String {
    match mode {
        IntakeMode::Fast => {
            interviewer_instructions(workdir, repo_slug, repo_context, wiki_context)
        }
        IntakeMode::Socratic => {
            socratic_stage_instructions(stage, workdir, repo_slug, repo_context, wiki_context)
        }
    }
}

/// One Socratic pass (spec 008 F2, made adaptive by #257). Reuses the SAME
/// grounding blocks as [`interviewer_instructions`] (context line / repo
/// snapshot / access rule / wiki) but swaps the "job/Rules" body for a
/// SINGLE-topic pass: the model (a) validates the user's previous answer
/// against the pass topic (re-asking when it was vague — depth adapts to
/// answer quality), then (b) asks ONLY this stage's question. Every reply ends
/// with a machine-read control marker (`[[socratic:advance|stay|done]]`) the
/// CLIENT moves the stage machine on; `done` is gated on the spec actually
/// being well-defined (the convergence gate), and only stage 5 may emit it and
/// point the user at "Preview issues" (the same convergence Fast uses). The
/// server owns NO stage state — this is a pure `(stage, grounding) → prompt`
/// function (D-B/D1). `stage` is clamped defensively.
fn socratic_stage_instructions(
    stage: u8,
    workdir: Option<&str>,
    repo_slug: Option<&str>,
    repo_context: Option<&str>,
    wiki_context: Option<&str>,
) -> String {
    let (ctx, repo_block, access_rule, wiki_block) =
        intake_grounding_blocks(workdir, repo_slug, repo_context, wiki_context);
    let stage = stage.clamp(1, 5);
    let pass = socratic_pass_body(stage);

    // The frame + Rules are shared across passes; `pass` is the one thing this
    // turn does. "Preview issues" (in pass 5) is the UI button label — keep it in
    // sync with the ChatPage composer, same as the Fast prompt above. The
    // control-marker spelling is parsed by the client's socratic-intake.ts —
    // keep the two in sync.
    format!(
        "You are running inside agentum (a control plane for AI coding agents) as the \
feature-intake interviewer on the Chat screen, running an ADAPTIVE Socratic interview — one \
focused pass per turn across five topics (WHO → WHAT → WHY → done-criteria → risks). This is \
pass {stage} of 5.{ctx}{repo_block}{wiki_block}\n\n\
Your job THIS TURN, and nothing else:\n\
{pass}\n\n\
Rules:\n\
- First judge whether the user's previous answer actually covered THIS pass's topic. If it \
was vague, contradictory, or missing, re-ask this topic more concretely (offer a sharp \
candidate answer to react to) instead of moving on — depth adapts to answer quality, not a \
fixed script.\n\
- Ask ONE question (two only if tightly related), short and concrete \
like a sharp staff engineer who knows this codebase — no filler, no \"great question!\".\n\
- Never re-ask what the user — or the repo context — already answered. Do NOT jump ahead to a \
later pass, and do NOT draft the task breakdown before the interview converges.\n\
{access_rule}\n\
- End EVERY reply with exactly one control line, alone on the final line with nothing after \
it: [[socratic:advance]] when this pass's topic is now well covered, [[socratic:stay]] when \
it still needs another round, or [[socratic:done]] ONLY on the final pass AND only when the \
problem, outcome, acceptance criteria, and scope boundaries are all concrete enough to draft \
from — that is the convergence gate. The line is machine-read and stripped from the UI; never \
mention or explain it."
    )
}

/// The single-topic instruction for one Socratic pass (spec 008 F2). Each pass
/// (except the first, which has nothing to reflect yet) begins by reflecting the
/// previous answer back in one sentence, then asks only its own topic. The five:
/// 1 WHO, 2 WHAT, 3 WHY, 4 done-criteria, 5 risks+scope (then converge on
/// "Preview issues"). Pinned per-stage by unit test (AC 7). `stage` is already
/// clamped to 1..=5 by the caller, so the `_` arm is pass 5.
fn socratic_pass_body(stage: u8) -> &'static str {
    match stage {
        1 => {
            "PASS 1 — WHO. This is the opening pass, so there is nothing to reflect back yet. \
Open the interview: draw out WHO this feature is for (the specific persona / user) and the \
concrete problem they hit today. Don't propose a solution yet."
        }
        2 => {
            "PASS 2 — WHAT. First, in ONE sentence, reflect the user's previous answer back (the \
WHO and their problem) so they know you heard it. Then ask about WHAT: the desired outcome or \
behavior — what the feature actually does when it works."
        }
        3 => {
            "PASS 3 — WHY. First, in ONE sentence, reflect the previous answer back (the desired \
WHAT / outcome). Then ask about WHY it matters: the value, why now, and what breaks or stays \
painful without it."
        }
        4 => {
            "PASS 4 — DONE CRITERIA. First, in ONE sentence, reflect the previous answer back (the \
WHY / value). Then draw out the acceptance criteria: how we'll know it's done, as concrete, \
testable, checkbox-shaped statements."
        }
        _ => {
            "PASS 5 — RISKS & SCOPE. First, in ONE sentence, reflect the previous answer back (the \
acceptance criteria). Then draw out the risks and the scope boundaries / non-goals (what is \
explicitly OUT). This is the FINAL pass, and it is GATED: when — and only when — the problem, \
outcome, acceptance criteria, and scope are all concrete, STOP asking questions, tell the user \
to click the \"Preview issues\" button below the chat to review and file the drafted issues, and \
end with [[socratic:done]]. If real gaps remain, keep clarifying and end with [[socratic:stay]]."
        }
    }
}

/// The single credential gate BOTH intake handlers ([`chat`], [`chat_stream`])
/// run FIRST — before any per-mode/stage prompt is built. Extracted so the spec
/// 008 F2 invariant is unit-pinnable: Complex (socratic) rides the SAME gate as
/// Fast (there is no separate Complex endpoint), so the loud both-paths
/// [`NO_CREDS_MSG`] surfaces on Complex's first turn by construction — never a
/// silent dead button. Pure: maps a resolved credential (`None` ⇒ the 400),
/// independent of `{mode, stage}`.
fn chat_auth_gate(auth: Option<Auth>) -> Result<Auth, ApiError> {
    auth.ok_or_else(|| ApiError::BadRequest(NO_CREDS_MSG.into()))
}

/// Retrieve query-relevant AutoWiki excerpts (spec 003 RAG) for the latest user
/// turn, off the async runtime (`retrieve_context` is blocking fs + CPU math).
/// Best-effort by contract: no user turn / no wiki / no sidecar / a model
/// mismatch → `None`, and the interview grounds on the static snapshot alone.
async fn retrieve_wiki(workdir: Option<&str>, messages: &[ChatMessage]) -> Option<String> {
    let query = messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())?;
    let workdir = workdir.map(str::to_string);
    tokio::task::spawn_blocking(move || {
        crate::wiki_rag::retrieve_context(
            workdir.as_deref(),
            &query,
            crate::wiki_rag::DEFAULT_TOP_K,
        )
    })
    .await
    .ok()
    .flatten()
}

async fn chat(
    State(_state): State<AppState>,
    Json(body): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, ApiError> {
    if body.messages.is_empty() {
        return Err(ApiError::BadRequest(
            "chat: messages cannot be empty".into(),
        ));
    }

    // Auth: prefer an explicit ANTHROPIC_API_KEY (clean API billing, terms-safe);
    // else reuse the Claude Code OAuth token (user-authorized). Absent → loud,
    // actionable error naming BOTH paths. Shared gate (spec 008 F2) so Fast and
    // Complex surface the no-creds message identically, upstream of the mode.
    let auth = chat_auth_gate(resolve_auth())?;

    let model = body
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_MODEL);

    let messages: Vec<serde_json::Value> = body
        .messages
        .iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();

    // Spec 008 F2: pick the intake prompt from the request's (mode, stage). Absent
    // mode ⇒ Fast (byte-identical to today); the client owns stage advancement.
    let mode = body.mode.unwrap_or(IntakeMode::Fast);
    let stage = body.stage.unwrap_or(1);
    let repo_context = gather_repo_context(body.workdir.as_deref());
    let wiki_context = retrieve_wiki(body.workdir.as_deref(), &body.messages).await;
    let instructions = build_intake_instructions(
        mode,
        stage,
        body.workdir.as_deref(),
        body.repo_slug.as_deref(),
        repo_context.as_deref(),
        wiki_context.as_deref(),
    );
    let system = build_system(&auth, &instructions);

    let text = call_anthropic(&auth, model, system, &messages, MAX_TOKENS_REPLY).await?;

    Ok(Json(ChatResponse {
        role: "assistant",
        content: text,
    }))
}

/// Apply the auth-specific headers to an Anthropic request and return the secret
/// (kept so it can be scrubbed from any error). An API key goes in `x-api-key`; an
/// OAuth (subscription) token needs bearer + the oauth beta + the Claude Code UA
/// (the identity gate itself lives in [`build_system`]). Shared by the
/// non-streaming [`call_anthropic`] and the streaming [`chat_stream`].
fn apply_auth(req: reqwest::RequestBuilder, auth: &Auth) -> (reqwest::RequestBuilder, String) {
    match auth {
        Auth::ApiKey(k) => (req.header("x-api-key", k), k.clone()),
        Auth::Oauth(t) => (
            req.bearer_auth(t)
                .header("anthropic-beta", OAUTH_BETA_HEADER)
                .header(reqwest::header::USER_AGENT, CLAUDE_CODE_USER_AGENT),
            t.clone(),
        ),
    }
}

/// Build the `/v1/messages` body for the streaming route. With thinking on we add
/// the `thinking` block and raise `max_tokens` above the budget (it caps reasoning
/// **plus** answer); off, the small reply cap. `stream` is always true here, and
/// temperature is never set (extended thinking requires the default).
fn build_stream_payload(
    model: &str,
    system: serde_json::Value,
    messages: &[serde_json::Value],
    thinking: bool,
) -> serde_json::Value {
    let max_tokens = if thinking {
        MAX_TOKENS_THINKING
    } else {
        MAX_TOKENS_REPLY
    };
    let mut payload = json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": messages,
        "stream": true,
    });
    if thinking {
        payload["thinking"] = json!({ "type": "enabled", "budget_tokens": THINKING_BUDGET_TOKENS });
    }
    payload
}

/// `POST /api/chat/stream` — the same Socratic interview as [`chat`], streamed.
/// Re-emits Anthropic's SSE as compact one-line `data:` JSON the desktop consumes
/// incrementally: `{type:"text",text}` (answer), `{type:"thinking",text}`
/// (reasoning, when `thinking` is on), then exactly one terminal event —
/// `{type:"error",message}` or `{type:"done"}`. An upstream non-2xx (bad/expired
/// credential, bad model) is returned as a normal typed error BEFORE the stream
/// opens, so the client sees a clean failure rather than a 200 that errors.
async fn chat_stream(
    State(_state): State<AppState>,
    Json(body): Json<ChatRequest>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    if body.messages.is_empty() {
        return Err(ApiError::BadRequest(
            "chat: messages cannot be empty".into(),
        ));
    }
    // Shared credential gate (spec 008 F2): Complex (socratic) rides the SAME gate
    // as Fast, so NO_CREDS_MSG surfaces on Complex's first turn by construction.
    let auth = chat_auth_gate(resolve_auth())?;

    let model = body
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_MODEL)
        .to_string();
    let thinking = body.thinking.unwrap_or(false);

    let raw: Vec<serde_json::Value> = body
        .messages
        .iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();
    // Same normalization the non-streaming path applies (open on user, alternate,
    // end on user) so a converged transcript can never 400 the upstream call.
    let messages = sanitize_messages(&raw);
    if messages.is_empty() {
        return Err(ApiError::BadRequest(
            "chat: no user message to send to the model".into(),
        ));
    }

    // Spec 008 F2: (mode, stage) selects the intake prompt — Fast is byte-identical
    // to today; Socratic runs the staged pass. The client owns stage advancement.
    let mode = body.mode.unwrap_or(IntakeMode::Fast);
    let stage = body.stage.unwrap_or(1);
    let repo_context = gather_repo_context(body.workdir.as_deref());
    let wiki_context = retrieve_wiki(body.workdir.as_deref(), &body.messages).await;
    let instructions = build_intake_instructions(
        mode,
        stage,
        body.workdir.as_deref(),
        body.repo_slug.as_deref(),
        repo_context.as_deref(),
        wiki_context.as_deref(),
    );
    let system = build_system(&auth, &instructions);
    let payload = build_stream_payload(&model, system, &messages, thinking);

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| ApiError::Internal(format!("build http client: {e}")))?;

    let (req, secret) = apply_auth(
        client
            .post(MESSAGES_URL)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&payload),
        &auth,
    );

    let resp = req.send().await.map_err(|e| {
        ApiError::Internal(format!(
            "anthropic request failed: {}",
            redact(&e.to_string(), &secret)
        ))
    })?;

    let status = resp.status();
    if !status.is_success() {
        // Resolve the credential complaint to an actionable hint BEFORE opening the
        // stream — the client gets a clean typed error, not a 200 SSE that errors.
        let raw = resp.text().await.unwrap_or_default();
        let hint = if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            match auth {
                Auth::ApiKey(_) => " (check ANTHROPIC_API_KEY)",
                Auth::Oauth(_) => {
                    " (your Claude login may have expired — run `claude` to refresh it, or set ANTHROPIC_API_KEY)"
                }
            }
        } else {
            ""
        };
        let detail = redact(raw.trim(), &secret);
        let detail = detail.chars().take(300).collect::<String>();
        return Err(ApiError::Custom(
            StatusCode::BAD_GATEWAY,
            json!({ "error": { "code": "llm_failed", "message": format!("chat model returned {status}{hint}: {detail}") } }),
        ));
    }

    // On the OAuth (subscription/login) path, extended-thinking reasoning is
    // returned ENCRYPTED — Anthropic emits a `signature_delta` (a redacted
    // thinking block), never the plaintext `thinking_delta` we forward — so the
    // reasoning trace would be silently blank. Surface why, once, in the trace
    // itself, and how to get it. The API-key path returns plaintext and is
    // unaffected (so no notice there).
    let redacted_thinking_notice: Option<String> = if thinking && matches!(auth, Auth::Oauth(_)) {
        Some(
            "_Extended reasoning ran, but Claude subscription (login) tokens return it \
encrypted — the reasoning text can't be shown here. Set `ANTHROPIC_API_KEY` to view the \
model's thinking._"
                .to_string(),
        )
    } else {
        None
    };

    // Proxy: parse Anthropic's SSE frames as they arrive and re-emit our compact
    // events. We buffer raw bytes and decode only whole frames (delimited by the
    // ASCII `\n\n`) so a multi-byte char split across a chunk is never mangled.
    // `resp`/`secret` are moved into the generator, making the stream `'static`.
    let stream = async_stream::stream! {
        // Lead with the redacted-thinking notice (OAuth path) so the reasoning
        // panel explains the blank trace instead of showing nothing.
        if let Some(note) = redacted_thinking_notice {
            yield Ok(Event::default().data(json!({ "type": "thinking", "text": note }).to_string()));
        }
        let mut bytes = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = bytes.next().await {
            match chunk {
                Ok(c) => buf.extend_from_slice(&c),
                Err(e) => {
                    let message = redact(&e.to_string(), &secret);
                    yield Ok(Event::default().data(json!({ "type": "error", "message": message }).to_string()));
                    return;
                }
            }
            while let Some(pos) = find_double_newline(&buf) {
                let frame = String::from_utf8_lossy(&buf[..pos]).into_owned();
                buf.drain(..pos + 2);
                for line in frame.lines() {
                    let Some(data) = line.trim_start().strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();
                    if data.is_empty() {
                        continue;
                    }
                    if let Some(ev) = parse_sse_delta(data) {
                        let terminal = matches!(
                            ev.get("type").and_then(|t| t.as_str()),
                            Some("done") | Some("error")
                        );
                        yield Ok(Event::default().data(ev.to_string()));
                        if terminal {
                            return;
                        }
                    }
                }
            }
        }
        // Stream closed without an explicit terminal — emit `done` so the client
        // finalizes the turn without depending on socket-close detection.
        yield Ok(Event::default().data(json!({ "type": "done" }).to_string()));
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// Index of the first `\n\n` (SSE frame separator) in `buf`, or `None`. `\n` is
/// ASCII, so the returned offset is always a valid UTF-8 char boundary.
fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

/// Map one Anthropic SSE `data:` JSON to our compact event, or `None` for frames
/// we don't forward (`message_start`, `ping`, `content_block_start`/`_stop`,
/// `message_delta`, `signature_delta`). Pure → unit-tested without a live stream.
fn parse_sse_delta(data: &str) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    match v.get("type").and_then(|t| t.as_str())? {
        "content_block_delta" => {
            let delta = v.get("delta")?;
            match delta.get("type").and_then(|t| t.as_str())? {
                "text_delta" => {
                    Some(json!({ "type": "text", "text": delta.get("text")?.as_str()? }))
                }
                "thinking_delta" => {
                    Some(json!({ "type": "thinking", "text": delta.get("thinking")?.as_str()? }))
                }
                // signature_delta / input_json_delta carry no user-visible text.
                _ => None,
            }
        }
        "error" => {
            let message = v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("the model stream errored");
            Some(json!({ "type": "error", "message": message }))
        }
        "message_stop" => Some(json!({ "type": "done" })),
        _ => None,
    }
}

/// Build the `system` value for a request. An OAuth (subscription) token requires
/// the Claude Code identity to lead the `system` (see module docs); a real API
/// key does NOT (and must not spoof it). Factored out so the chat interviewer and
/// the issue-extraction call share the byte-exact identity-prefix logic.
fn build_system(auth: &Auth, instructions: &str) -> serde_json::Value {
    match auth {
        Auth::Oauth(_) => json!([
            { "type": "text", "text": CLAUDE_CODE_IDENTITY },
            { "type": "text", "text": instructions },
        ]),
        Auth::ApiKey(_) => json!(instructions),
    }
}

/// Coerce a `{role, content}` history into a shape Anthropic's `/v1/messages`
/// accepts: it must begin with a `user` turn, alternate user/assistant, and —
/// because the default model has no prefill support — END on a `user` turn. A
/// converged chat transcript ends on the assistant's breakdown proposal, which
/// the API rejects with "does not support assistant message prefill". We drop
/// leading non-user turns, merge consecutive same-role turns (joining their
/// text), and drop any trailing assistant turn (every call in this module wants
/// a fresh reply to the latest user input, never assistant prefill). An empty or
/// all-assistant history collapses to an empty vec, which the caller rejects.
fn sanitize_messages(messages: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let role_of = |m: &serde_json::Value| {
        m.get("role")
            .and_then(|r| r.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let content_of = |m: &serde_json::Value| {
        m.get("content")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string()
    };

    let mut out: Vec<(String, String)> = Vec::new();
    for m in messages {
        let role = role_of(m);
        let content = content_of(m);
        // The array must open on a `user` turn — skip anything before the first.
        if out.is_empty() && role != "user" {
            continue;
        }
        match out.last_mut() {
            // Consecutive same-role turns aren't allowed — fold the text in.
            Some((last_role, last_content)) if *last_role == role => {
                if !content.is_empty() {
                    if !last_content.is_empty() {
                        last_content.push_str("\n\n");
                    }
                    last_content.push_str(&content);
                }
            }
            _ => out.push((role, content)),
        }
    }

    // Never end on an assistant turn (the assistant prefill the model rejects).
    while out.last().is_some_and(|(role, _)| role != "user") {
        out.pop();
    }

    out.into_iter()
        .map(|(role, content)| json!({ "role": role, "content": content }))
        .collect()
}

/// POST `/v1/messages` and return the concatenated assistant text. Owns the
/// auth-specific headers (x-api-key for an API key; bearer + the oauth beta +
/// Claude Code UA for an OAuth token), the redacted/typed non-2xx handling, and
/// the empty-reply guard — so every Anthropic call in this module behaves
/// identically. The caller supplies the already-built `system` (via
/// [`build_system`]) and the user/assistant `messages`.
async fn call_anthropic(
    auth: &Auth,
    model: &str,
    system: serde_json::Value,
    messages: &[serde_json::Value],
    max_tokens: u32,
) -> Result<String, ApiError> {
    // Normalize the history at this single choke point so a malformed transcript
    // (leading/trailing assistant turn, repeated roles) can never 400 the
    // upstream call — most importantly the trailing-assistant "prefill" reject
    // that the default model returns for a converged interview transcript.
    let messages = sanitize_messages(messages);
    if messages.is_empty() {
        return Err(ApiError::BadRequest(
            "chat: no user message to send to the model".into(),
        ));
    }
    let payload = json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": messages,
    });

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| ApiError::Internal(format!("build http client: {e}")))?;

    // Common headers + the auth-specific headers (x-api-key vs bearer+oauth-beta+UA),
    // applied by `apply_auth` — shared byte-for-byte with the streaming route.
    let (req, secret) = apply_auth(
        client
            .post(MESSAGES_URL)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&payload),
        auth,
    );
    let resp = req.send().await.map_err(|e| {
        ApiError::Internal(format!(
            "anthropic request failed: {}",
            redact(&e.to_string(), &secret)
        ))
    })?;

    let status = resp.status();
    let raw = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        // 401/403: the credential was rejected. Name both recovery paths.
        let hint = if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            match auth {
                Auth::ApiKey(_) => " (check ANTHROPIC_API_KEY)",
                Auth::Oauth(_) => {
                    " (your Claude login may have expired — run `claude` to refresh it, or set ANTHROPIC_API_KEY)"
                }
            }
        } else {
            ""
        };
        let detail = redact(raw.trim(), &secret);
        let detail = detail.chars().take(300).collect::<String>();
        return Err(ApiError::Custom(
            StatusCode::BAD_GATEWAY,
            json!({ "error": { "code": "llm_failed", "message": format!("chat model returned {status}{hint}: {detail}") } }),
        ));
    }

    let v: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| ApiError::Internal(format!("parse anthropic response: {e}")))?;

    // Concatenate all text blocks of the assistant message.
    let text = v
        .get("content")
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    if text.trim().is_empty() {
        return Err(ApiError::Internal(
            "chat model returned an empty reply".into(),
        ));
    }

    Ok(text)
}

// ---------------------------------------------------------------------------
// POST /api/chat/issues — turn the agreed breakdown into ONE tracker issue.
//
// Closes the loop on the Chat interviewer: once the conversation has converged
// on a feature, this endpoint asks the model to distil it into a single feature
// plan (a parent + ordered, prioritised sub-tasks), then files ONE issue — the
// sub-tasks rendered as a priority-sorted checklist in the body — into the
// chosen tracker via the shared `TaskSink` arm (GitHub `gh` or Linear GraphQL,
// so YOLO/argv/parse stay in one place). One feature = one issue, not N flat
// issues. `provider` selects the destination ("github" default, or "linear"); an
// unknown provider is a 400 and an unconnected one a typed 422 — the Chat rule is
// GitHub/Linear only, never the internal board. Created/failed are still returned
// as arrays (now length 0 or 1) so the UI rendering is unchanged.
// ---------------------------------------------------------------------------

/// The system prompt that distils a conversation into ONE structured feature plan
/// (a parent feature + ordered, prioritised sub-tasks) — not a flat list of
/// separate issues. Kept byte-exact; the lenient parser ([`extract_feature_plan`])
/// tolerates a model that still wraps it in prose or fences.
const EXTRACT_INSTRUCTIONS: &str = "From this conversation, extract the agreed feature as a SINGLE JSON object: {\"title\": string, \"summary\": string, \"problem\": string, \"goal\": string, \"tasks\": [{\"title\": string, \"detail\": string, \"priority\": \"high\" | \"medium\" | \"low\"}]}. title = a concise feature title; summary = 1–2 sentences describing the feature; tasks = the sub-tasks needed to build it, each with a short title, a 1–2 sentence detail, and a priority. The task COUNT must match the scope actually discussed: a trivial ask is a SINGLE task, a small feature two or three — never pad to a fixed number, and never invent tasks the conversation didn't call for. Order the tasks by priority and logical sequence (most important / earliest first). problem = 1–3 sentences naming the user-felt problem this feature solves (no solution language); goal = ONE sentence naming the concrete user outcome. Write every string in a plain engineer's voice: name concrete behaviors, files, and surfaces; no marketing adjectives, no 'This feature will…' openers, no filler like 'improve the user experience', and don't restate the title inside summary/problem/goal. Output ONLY the raw JSON object, no prose, no markdown code fences.";

/// The final user turn appended to the transcript for the extraction call. Ends
/// the history on a `user` turn (Anthropic rejects a trailing-assistant array —
/// the v0.33.0 issue-filing 400) and gives the model a direct last-word
/// instruction to emit the JSON now.
const EXTRACT_USER_PROMPT: &str = "Output the agreed feature plan now as the single JSON object described above — only the raw JSON object, nothing else.";

#[derive(Deserialize)]
struct ChatIssuesRequest {
    /// The conversation to distil into issues (user/assistant turns only).
    messages: Vec<ChatMessage>,
    /// Explicit `owner/repo` target. When well-formed it short-circuits the
    /// host-aware origin read.
    #[serde(default)]
    repo_slug: Option<String>,
    /// Project dir — used to read the GitHub `origin` when no `repo_slug` is
    /// given. The created-issue path itself runs `gh` from `$HOME`.
    #[serde(default)]
    workdir: Option<String>,
    /// Which tracker to file into: `"github"` (default, back-compat) or
    /// `"linear"`. The Chat-only rule (GitHub/Linear, never the internal board)
    /// is enforced here: an unknown provider is a hard 400, and a chosen-but-
    /// unconnected provider is a loud typed 422 — never a silent board fallback.
    #[serde(default)]
    provider: Option<String>,
    /// Spec 003: a client-supplied, user-edited feature plan. When present,
    /// extraction is **skipped** and this plan is filed verbatim — the
    /// what-you-see-is-what-you-file guarantee (Confirm files exactly the draft
    /// the user reviewed). Absent → extract from `messages` (back-compat).
    #[serde(default)]
    plan: Option<FeaturePlan>,
    /// Spec 003: how to file — `"single"` (one issue, sub-tasks as a priority
    /// checklist — default, today's behaviour) or `"per_task"` (one issue per
    /// task). Anything unrecognised falls back to single.
    #[serde(default)]
    split: Option<String>,
    /// Spec 003: labels to apply to the created issue(s). GitHub `--label`;
    /// Linear is a documented v1 no-op. Empty = none.
    #[serde(default)]
    labels: Vec<String>,
}

/// One sub-task of the feature. `detail`/`priority` are optional so a terse model
/// reply still parses; a missing or garbled priority defaults to Medium.
#[derive(Deserialize)]
struct SubTask {
    title: String,
    #[serde(default)]
    detail: String,
    #[serde(default)]
    priority: Option<String>,
}

/// The whole agreed feature: a parent + its ordered, prioritised sub-tasks. This
/// becomes ONE issue (sub-tasks as a checklist in the body), not N flat issues.
#[derive(Deserialize)]
struct FeaturePlan {
    title: String,
    #[serde(default)]
    summary: String,
    /// Spec 006 F2: SDD framing. Optional — absent keeps [`compose_issue_body`]
    /// byte-identical to the pre-006 body (pinned), so a terse model reply, an
    /// old client's plan, and every existing fixture still parse and render
    /// exactly as before.
    #[serde(default)]
    problem: Option<String>,
    #[serde(default)]
    goal: Option<String>,
    tasks: Vec<SubTask>,
}

/// Sub-task priority. Ordered High→Low; drives the checklist sort.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Priority {
    High,
    Medium,
    Low,
}

impl Priority {
    /// Sort key — High sorts first.
    fn rank(self) -> u8 {
        match self {
            Priority::High => 0,
            Priority::Medium => 1,
            Priority::Low => 2,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Priority::High => "High",
            Priority::Medium => "Medium",
            Priority::Low => "Low",
        }
    }
}

/// Map the model's free-form priority string to a [`Priority`], leniently — it
/// may say "high"/"P1"/"critical"/etc., or omit it. Anything unrecognised (or
/// absent) is Medium, so a sloppy reply never drops a task.
fn parse_priority(raw: Option<&str>) -> Priority {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("high") | Some("highest") | Some("critical") | Some("urgent") | Some("p0")
        | Some("p1") | Some("1") => Priority::High,
        Some("low") | Some("lowest") | Some("minor") | Some("trivial") | Some("p3")
        | Some("p4") | Some("3") | Some("4") => Priority::Low,
        _ => Priority::Medium,
    }
}

/// Render a feature plan into ONE issue body: the summary, then the sub-tasks as
/// a checklist sorted by priority (High→Low, stable within a priority so the
/// model's sequence is preserved). This is what turns "5 flat tickets" into one
/// ticket with ordered, prioritised sub-tasks.
///
/// Spec 006 F2: when the plan carries a `problem` and/or `goal`, the body is
/// SDD-shaped — `## Problem` / `## Goal` sections and the checklist under
/// `## Acceptance criteria`. The `- [ ]` line rendering is shared between both
/// shapes, which is what makes the spec_md → backlog round-trip (AC 5) hold by
/// construction. With both fields absent the output is byte-identical to the
/// pre-006 body (pinned).
fn compose_issue_body(plan: &FeaturePlan) -> String {
    let mut tasks: Vec<(usize, &SubTask, Priority)> = plan
        .tasks
        .iter()
        .enumerate()
        .map(|(i, t)| (i, t, parse_priority(t.priority.as_deref())))
        .collect();
    // Stable sort by priority; equal priorities keep the model's original order.
    tasks.sort_by_key(|(i, _, p)| (p.rank(), *i));

    // Present-but-blank is absent: a model that emits "" must not flip the shape.
    let problem = plan
        .problem
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let goal = plan
        .goal
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let sdd = problem.is_some() || goal.is_some();

    let mut body = String::new();
    let summary = plan.summary.trim();
    if !summary.is_empty() {
        body.push_str(summary);
        body.push_str("\n\n");
    }
    if let Some(p) = problem {
        body.push_str("## Problem\n\n");
        body.push_str(p);
        body.push_str("\n\n");
    }
    if let Some(g) = goal {
        body.push_str("## Goal\n\n");
        body.push_str(g);
        body.push_str("\n\n");
    }
    body.push_str(if sdd {
        "## Acceptance criteria\n\n"
    } else {
        "## Sub-tasks (priority order)\n\n"
    });
    for (_, t, p) in &tasks {
        body.push_str(&format!("- [ ] **[{}]** {}", p.label(), t.title.trim()));
        let detail = t.detail.trim();
        if !detail.is_empty() {
            body.push_str(" — ");
            body.push_str(detail);
        }
        body.push('\n');
    }
    body
}

/// How Chat files the plan (spec 003). `Single` = ONE issue with the sub-tasks as
/// a priority-ordered checklist (default — the pre-003 behaviour). `PerTask` = one
/// issue per task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SplitMode {
    Single,
    PerTask,
}

impl SplitMode {
    /// Map the request's optional `split` string; anything unrecognised (or
    /// absent) is `Single`, so a sloppy value never fans out into N issues.
    fn parse(raw: Option<&str>) -> SplitMode {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("per_task") | Some("per-task") | Some("multiple") | Some("split") => {
                SplitMode::PerTask
            }
            _ => SplitMode::Single,
        }
    }
}

/// Turn a plan into the concrete issue(s) to file — the ONE place a plan becomes
/// `NewFeature`s, so "what the user confirmed is what gets filed" holds (no
/// re-extraction). `Single` → one issue whose body is the priority checklist
/// ([`compose_issue_body`]). `PerTask` → one issue per task, priority-ordered
/// (High→Low, stable), each body carrying the task detail + priority + parent
/// feature. `labels` ride on every issue.
fn plan_to_features(plan: &FeaturePlan, split: SplitMode, labels: &[String]) -> Vec<NewFeature> {
    match split {
        SplitMode::Single => vec![NewFeature {
            title: plan.title.clone(),
            body: Some(compose_issue_body(plan)),
            labels: labels.to_vec(),
        }],
        SplitMode::PerTask => {
            let mut tasks: Vec<(usize, &SubTask, Priority)> = plan
                .tasks
                .iter()
                .enumerate()
                .map(|(i, t)| (i, t, parse_priority(t.priority.as_deref())))
                .collect();
            // Same stable priority sort the checklist uses.
            tasks.sort_by_key(|(i, _, p)| (p.rank(), *i));
            tasks
                .into_iter()
                .map(|(_, t, p)| NewFeature {
                    title: t.title.trim().to_string(),
                    body: Some(compose_task_body(plan, t, p)),
                    labels: labels.to_vec(),
                })
                .collect()
        }
    }
}

/// One per-task issue body: the task detail, its priority, and a back-reference to
/// the parent feature so a split-out issue still reads standalone.
fn compose_task_body(plan: &FeaturePlan, task: &SubTask, priority: Priority) -> String {
    let mut body = String::new();
    let detail = task.detail.trim();
    if !detail.is_empty() {
        body.push_str(detail);
        body.push_str("\n\n");
    }
    body.push_str(&format!("**Priority:** {}\n", priority.label()));
    let feature = plan.title.trim();
    if !feature.is_empty() {
        body.push_str(&format!("**Feature:** {feature}\n"));
    }
    body
}

/// Run the extraction LLM call over a transcript and parse a [`FeaturePlan`].
/// Shared by the preview endpoint and the create endpoint (when the client did
/// NOT supply an already-edited plan). Byte-identical to the call it was extracted
/// from, so the OAuth identity block + trailing-user-turn invariants hold.
async fn extract_plan(
    auth: &Auth,
    transcript: &[ChatMessage],
    workdir: Option<&str>,
) -> Result<FeaturePlan, ApiError> {
    // Append an explicit final USER turn asking for the JSON (Anthropic rejects a
    // trailing-assistant array; a direct last-word instruction also extracts more
    // reliably than the system prompt alone).
    let mut messages: Vec<serde_json::Value> = transcript
        .iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();
    messages.push(json!({ "role": "user", "content": EXTRACT_USER_PROMPT }));

    // Lead the system with the strict-JSON instruction, then the SAME repo +
    // harness snapshot the interview used — so each task names the real files.
    let extract_system = match gather_repo_context(workdir) {
        Some(c) => format!(
            "{EXTRACT_INSTRUCTIONS}\n\nGround every task in this real project snapshot — \
name the actual files/modules each task touches:\n\
=== REPO & HARNESS CONTEXT ===\n{c}\n=== END CONTEXT ==="
        ),
        None => EXTRACT_INSTRUCTIONS.to_string(),
    };
    let system = build_system(auth, &extract_system);
    let text = call_anthropic(auth, DEFAULT_MODEL, system, &messages, 2048).await?;

    // Parse leniently (fences/prose tolerated). No object / no tasks → 422.
    extract_feature_plan(&text).ok_or_else(|| {
        ApiError::Custom(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({ "error": { "code": "no_tasks", "message": "could not extract a feature plan from the conversation" } }),
        )
    })
}

/// Distil the agreed task breakdown from a chat transcript, then file each task
/// into the chosen tracker (`provider`: GitHub default, or Linear). Returns
/// `{ provider, repo?, created[], failed[] }` (200 even on a partial or total
/// per-task failure). The hard errors are LLM/auth (502/400), no extractable
/// tasks (`no_tasks` 422), an unknown provider (400), and a chosen-but-
/// unconnected tracker (`no_github_repo`/`no_linear` 422) — all typed envelopes.
async fn chat_issues(
    State(state): State<AppState>,
    Json(mut body): Json<ChatIssuesRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // A create needs EITHER a client-supplied edited plan (Confirm) or a transcript
    // to extract one from (the legacy one-shot path).
    if body.messages.is_empty() && body.plan.is_none() {
        return Err(ApiError::BadRequest(
            "chat issues: provide a `plan` or a non-empty `messages` array".into(),
        ));
    }

    // Same credential resolution + actionable error as the chat handler.
    let auth = resolve_auth().ok_or_else(|| ApiError::BadRequest(NO_CREDS_MSG.into()))?;
    // The bearer/API token, kept so it can be scrubbed from any per-task error.
    let secret = match &auth {
        Auth::ApiKey(k) => k.clone(),
        Auth::Oauth(t) => t.clone(),
    };

    // Spec 003: Confirm files the CLIENT's edited plan VERBATIM (what-you-see-is-
    // what-you-file); only when none is supplied do we extract from the transcript
    // (back-compat one-shot). Re-extracting here would reintroduce the drift this
    // whole feature removes, so it must NOT happen when a plan is present.
    let plan = match body.plan.take() {
        Some(p) => {
            if p.title.trim().is_empty() || p.tasks.is_empty() {
                return Err(ApiError::Custom(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    json!({ "error": { "code": "no_tasks", "message": "the supplied plan has no title or no tasks" } }),
                ));
            }
            p
        }
        None => extract_plan(&auth, &body.messages, body.workdir.as_deref()).await?,
    };

    let split = SplitMode::parse(body.split.as_deref());

    // Default GitHub; Linear the alt. Anything else is a hard 400 (the Chat rule is
    // GitHub/Linear only, never the internal board). One issue on `single`, N on
    // `per_task` — the `created[]`/`failed[]` arrays carry either.
    match resolve_provider(body.provider.as_deref()) {
        Ok(IssueProvider::Github) => {
            create_github_issues(&state, &body, &plan, split, &secret).await
        }
        Ok(IssueProvider::Linear) => {
            create_linear_issues(&state, &plan, split, &body.labels, &secret).await
        }
        Err(other) => Err(ApiError::BadRequest(format!(
            "chat issues: unknown provider {other:?} (expected \"github\" or \"linear\")"
        ))),
    }
}

/// Spec 003: extract the feature plan and return it as an editable DRAFT — files
/// NOTHING. The UI renders `{title, summary, tasks[], body}` (body = the
/// single-issue composed markdown, the default split's preview), lets the user
/// edit / regenerate, then POSTs the result to `/api/chat/issues` to file it.
async fn chat_issues_preview(
    Json(body): Json<ChatIssuesRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Preview / Regenerate ALWAYS re-extracts from the transcript; a client `plan`
    // is irrelevant here (it's what Confirm sends, not Preview).
    if body.messages.is_empty() {
        return Err(ApiError::BadRequest(
            "chat issues preview: messages cannot be empty".into(),
        ));
    }
    let auth = resolve_auth().ok_or_else(|| ApiError::BadRequest(NO_CREDS_MSG.into()))?;
    let plan = extract_plan(&auth, &body.messages, body.workdir.as_deref()).await?;

    // Normalise each task's priority to a canonical `high|medium|low` so the UI has
    // a stable value to bind its per-task selector to.
    let tasks: Vec<serde_json::Value> = plan
        .tasks
        .iter()
        .map(|t| {
            json!({
                "title": t.title,
                "detail": t.detail,
                "priority": parse_priority(t.priority.as_deref()).label().to_ascii_lowercase(),
            })
        })
        .collect();

    Ok(Json(json!({
        "title": plan.title,
        "summary": plan.summary,
        // Spec 006 F2 (C4): the SDD fields ride the draft round-trip — this
        // response IS what the UI stores and posts back on Confirm, so omitting
        // them here would silently strip the shape from every previewed plan.
        "problem": plan.problem,
        "goal": plan.goal,
        "tasks": tasks,
        "body": compose_issue_body(&plan),
    })))
}

/// The tracker a `chat_issues` request targets. Closed set — the Chat rule is
/// GitHub/Linear only (never the internal board).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IssueProvider {
    Github,
    Linear,
}

/// Resolve the request's optional `provider` to an [`IssueProvider`]. Absent or
/// blank defaults to GitHub (back-compat with v0.29.0); an unrecognized value is
/// returned as `Err(value)` so the handler can 400 with the offending string.
/// Pure so the defaulting + validation is unit-tested without a live request.
fn resolve_provider(raw: Option<&str>) -> Result<IssueProvider, String> {
    match raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("github")
    {
        "github" => Ok(IssueProvider::Github),
        "linear" => Ok(IssueProvider::Linear),
        other => Err(other.to_string()),
    }
}

/// File the feature plan as GitHub issue(s) — one (sub-tasks as a checklist) or
/// one-per-task, per `split`. Resolves the repo slug once (a well-formed client
/// hint wins with no IO; else the LOCAL project's `origin` — Chat never files over
/// SSH), then creates each issue via the shared `TaskSink::Github` arm. Per-issue
/// failures land in `failed` (still a 200) so the UI surfaces every reason.
async fn create_github_issues(
    state: &AppState,
    body: &ChatIssuesRequest,
    plan: &FeaturePlan,
    split: SplitMode,
    secret: &str,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Resolve the GitHub slug: a well-formed client hint wins (no IO); else read
    // the LOCAL project's `origin` (Chat never files over SSH). No slug → 422.
    let slug = match body.repo_slug.as_deref().map(str::trim) {
        Some(s) if slug_matches(s) => s.to_string(),
        _ => {
            let host = state
                .store
                .get_host(agentum_core::LOCAL_HOST_ID)
                .await?
                .ok_or_else(|| ApiError::Internal("local host record missing".into()))?;
            let workdir = body
                .workdir
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    std::env::current_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| "/".to_string())
                });
            match super::board_goals::resolve_github_slug(
                &host,
                &workdir,
                body.repo_slug.as_deref(),
            )
            .await
            {
                Ok(slug) => slug,
                Err(_) => {
                    return Err(ApiError::Custom(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        json!({ "error": { "code": "no_github_repo", "message": "no GitHub repo resolved for this project" } }),
                    ));
                }
            }
        }
    };

    // The GitHub slug arm runs `gh` from `$HOME`, so `workdir` here is unused —
    // pass the project dir (or a temp dir) for shape only.
    let workdir_path: std::path::PathBuf = body
        .workdir
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);

    let features = plan_to_features(plan, split, &body.labels);
    let (mut created, mut failed) = (Vec::new(), Vec::new());
    for feature in &features {
        let res = TaskSink::Github
            .create_feature(
                &SinkCtx {
                    store: &state.store,
                    workdir: &workdir_path,
                    parent_goal_id: None,
                    slug: Some(&slug),
                },
                feature,
            )
            .await;
        match res {
            Ok(fref) => {
                created.push(json!({ "title": feature.title, "url": fref.url.unwrap_or_default() }))
            }
            Err(e) => {
                let detail = redact(&e.to_string(), secret)
                    .chars()
                    .take(300)
                    .collect::<String>();
                failed.push(json!({ "title": feature.title, "error": detail }));
            }
        }
    }
    Ok(Json(json!({
        "provider": "github",
        "repo": slug,
        "created": created,
        "failed": failed,
    })))
}

/// File the feature plan as Linear issue(s) — one (checklist) or one-per-task, per
/// `split` — via the shared `TaskSink::Linear` arm (single-team resolution lives
/// in `crate::linear`). A missing Linear connection is a loud typed 422 — never a
/// silent board fallback. `labels` ride on each `NewFeature` but Linear's create
/// arm ignores them for now (documented v1 no-op); per-issue failures land in
/// `failed` (still a 200).
async fn create_linear_issues(
    state: &AppState,
    plan: &FeaturePlan,
    split: SplitMode,
    labels: &[String],
    secret: &str,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Loud, actionable failure when Linear isn't connected — the Chat rule
    // forbids a silent internal-board fallback.
    if !crate::linear::available() {
        return Err(ApiError::Custom(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({ "error": { "code": "no_linear", "message": "no Linear workspace connected (connect Linear in Settings)" } }),
        ));
    }

    // Linear's create arm ignores `workdir`/`slug`/`parent_goal_id`; pass a
    // neutral ctx for shape only.
    let tmp = std::env::temp_dir();
    let features = plan_to_features(plan, split, labels);
    let (mut created, mut failed) = (Vec::new(), Vec::new());
    for feature in &features {
        let res = TaskSink::Linear
            .create_feature(
                &SinkCtx {
                    store: &state.store,
                    workdir: &tmp,
                    parent_goal_id: None,
                    slug: None,
                },
                feature,
            )
            .await;
        match res {
            Ok(fref) => created.push(json!({
                "title": feature.title,
                "id": fref.id,
                "url": fref.url.unwrap_or_default(),
            })),
            Err(e) => {
                let detail = redact(&e.to_string(), secret)
                    .chars()
                    .take(300)
                    .collect::<String>();
                failed.push(json!({ "title": feature.title, "error": detail }));
            }
        }
    }
    Ok(Json(json!({
        "provider": "linear",
        "created": created,
        "failed": failed,
    })))
}

/// Pull a `FeaturePlan` out of a possibly-noisy model reply: strip markdown
/// fences, slice from the first `{` to the last `}`, then parse. Returns `None`
/// on no object / parse failure / a missing title / an empty task list — all of
/// which the caller maps to the `no_tasks` 422.
fn extract_feature_plan(raw: &str) -> Option<FeaturePlan> {
    let cleaned = raw.replace("```json", "").replace("```", "");
    let start = cleaned.find('{')?;
    let end = cleaned.rfind('}')?;
    if end < start {
        return None;
    }
    // `{` and `}` are ASCII, so these byte offsets are valid char boundaries.
    let slice = &cleaned[start..=end];
    let plan: FeaturePlan = serde_json::from_str(slice).ok()?;
    if plan.title.trim().is_empty() || plan.tasks.is_empty() {
        return None;
    }
    Some(plan)
}

/// A client `repo_slug` must look like `owner/repo` — exactly one `/`, both
/// halves non-empty, no whitespace (the `^[^/\s]+/[^/\s]+$` shape). Mirrors
/// `board_goals::is_valid_slug` so a malformed hint falls through to the read.
fn slug_matches(s: &str) -> bool {
    let mut parts = s.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(repo), None) => {
            !owner.is_empty()
                && !repo.is_empty()
                && !owner.chars().any(char::is_whitespace)
                && !repo.chars().any(char::is_whitespace)
        }
        _ => false,
    }
}

/// Replace a bearer token anywhere in a string before it can be logged/returned.
fn redact(msg: &str, token: &str) -> String {
    if token.is_empty() {
        msg.to_string()
    } else {
        msg.replace(token, "<redacted>")
    }
}

// ---------------------------------------------------------------------------
// Issue-body drafting (spec 007) — one-shot "title → SDD-shaped description"
// for `POST /api/github/issues/draft-body` (routes::github). Lives here so it
// reuses the chat module's whole LLM surface (auth resolution, the byte-exact
// OAuth identity block, repo-context gathering, the shared Anthropic call)
// instead of growing a second one.

/// Hard cap on a generated body — the model is asked for a compact spec, so
/// anything beyond this is runaway output, not signal.
const DRAFT_BODY_MAX_CHARS: usize = 12_000;

/// Instructions for the drafting call. Pinned as a `const` so the prompt test
/// can assert the section contract without duplicating strings.
const DRAFT_BODY_INSTRUCTIONS: &str = "You draft GitHub issue descriptions for the repository described below.\n\
Given an issue TITLE, write the issue BODY as GitHub-flavored markdown with exactly these sections:\n\
## Problem — what is wrong or missing today, grounded in the repo context when available.\n\
## Goal — the observable outcome when this issue is done.\n\
## Acceptance criteria — 3 to 6 checklist items, each on its own line as `- [ ] …`, concrete and testable.\n\
Rules: output ONLY the markdown body (no title heading, no code fences around the whole reply, no preamble or sign-off). \
Reference real files/areas from the repo context when it is provided; never invent paths.";

/// The user turn for the drafting call — kept tiny; the repo grounding rides in
/// the system block like every other call in this module.
fn draft_body_user_message(title: &str) -> String {
    format!("TITLE: {title}\n\nWrite the issue body.")
}

/// Assemble the drafting system instructions: the pinned section contract plus
/// the repo snapshot (when a local workdir is available) and the slug hint.
fn draft_body_instructions(repo_slug: Option<&str>, repo_context: Option<&str>) -> String {
    let mut out = String::from(DRAFT_BODY_INSTRUCTIONS);
    if let Some(slug) = repo_slug {
        out.push_str(&format!("\n\nThe GitHub repository is `{slug}`."));
    }
    match repo_context {
        Some(ctx) => out.push_str(&format!(
            "\n\n=== REPO CONTEXT (a real snapshot of the project — ground the body in it) ===\n{ctx}\n=== END CONTEXT ===",
        )),
        None => out.push_str(
            "\n\nNo repo snapshot is available; keep the body honest and generic — do not invent project details.",
        ),
    }
    out
}

/// Clean a model reply into a postable body: strip a whole-reply code fence if
/// the model wrapped one anyway, trim, and cap at [`DRAFT_BODY_MAX_CHARS`].
fn sanitize_draft_body(raw: &str) -> String {
    let trimmed = raw.trim();
    // A reply that IS one fenced block (```markdown\n…\n```) gets unwrapped;
    // fences inside a longer body are legitimate markdown and stay.
    let unfenced = trimmed
        .strip_prefix("```")
        .and_then(|rest| {
            let rest = rest.split_once('\n').map(|(_, tail)| tail).unwrap_or(rest);
            rest.strip_suffix("```")
        })
        .map(str::trim)
        .unwrap_or(trimmed);
    truncate_chars(unfenced, DRAFT_BODY_MAX_CHARS)
}

/// Draft an SDD-shaped issue body from a title + local repo context. Shared
/// plumbing with `/api/chat`: same credential resolution (loud, actionable
/// error naming both recovery paths when absent), same repo snapshot, same
/// Anthropic call. `pub(crate)` — the HTTP surface lives in `routes::github`.
pub(crate) async fn draft_issue_body(
    workdir: Option<&str>,
    repo_slug: Option<&str>,
    title: &str,
) -> Result<String, ApiError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ApiError::BadRequest(
            "issue `title` must not be blank".into(),
        ));
    }
    let auth = resolve_auth().ok_or_else(|| ApiError::BadRequest(NO_CREDS_MSG.into()))?;

    let repo_context = gather_repo_context(workdir);
    let instructions = draft_body_instructions(repo_slug, repo_context.as_deref());
    let system = build_system(&auth, &instructions);
    let messages = vec![json!({ "role": "user", "content": draft_body_user_message(title) })];

    // 2048 output tokens: a full Problem/Goal/ACs body comfortably fits; the
    // interview's 1024 reply cap is tuned for short turns, not a document.
    let text = call_anthropic(&auth, DEFAULT_MODEL, system, &messages, 2048).await?;
    let body = sanitize_draft_body(&text);
    if body.is_empty() {
        return Err(ApiError::Internal(
            "the model returned an empty issue body".into(),
        ));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pull the `(role, content)` pairs back out of a sanitized array.
    fn pairs(v: &[serde_json::Value]) -> Vec<(String, String)> {
        v.iter()
            .map(|m| {
                (
                    m["role"].as_str().unwrap_or_default().to_string(),
                    m["content"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn sanitize_drops_trailing_assistant_prefill() {
        // The reported v0.33.0 bug: a converged transcript ends on the assistant's
        // breakdown proposal. The default model rejects a trailing-assistant array
        // ("does not support assistant message prefill"), so it must be dropped.
        let msgs = vec![
            json!({ "role": "user", "content": "add csv export" }),
            json!({ "role": "assistant", "content": "here is the breakdown" }),
        ];
        let out = sanitize_messages(&msgs);
        assert_eq!(pairs(&out), vec![("user".into(), "add csv export".into())]);
    }

    #[test]
    fn sanitize_keeps_an_alternating_history_ending_on_user() {
        let msgs = vec![
            json!({ "role": "user", "content": "a" }),
            json!({ "role": "assistant", "content": "b" }),
            json!({ "role": "user", "content": "c" }),
        ];
        let out = sanitize_messages(&msgs);
        assert_eq!(
            pairs(&out),
            vec![
                ("user".into(), "a".into()),
                ("assistant".into(), "b".into()),
                ("user".into(), "c".into()),
            ]
        );
    }

    #[test]
    fn sanitize_drops_leading_assistant() {
        // The array must open on a user turn.
        let msgs = vec![
            json!({ "role": "assistant", "content": "hi, describe a feature" }),
            json!({ "role": "user", "content": "ok" }),
        ];
        let out = sanitize_messages(&msgs);
        assert_eq!(pairs(&out), vec![("user".into(), "ok".into())]);
    }

    #[test]
    fn sanitize_merges_consecutive_same_role() {
        // Anthropic requires alternating roles; consecutive same-role turns fold
        // into one (text joined) rather than 400.
        let msgs = vec![
            json!({ "role": "user", "content": "one" }),
            json!({ "role": "user", "content": "two" }),
            json!({ "role": "assistant", "content": "ok" }),
            json!({ "role": "user", "content": "three" }),
        ];
        let out = sanitize_messages(&msgs);
        assert_eq!(
            pairs(&out),
            vec![
                ("user".into(), "one\n\ntwo".into()),
                ("assistant".into(), "ok".into()),
                ("user".into(), "three".into()),
            ]
        );
    }

    #[test]
    fn sanitize_collapses_all_assistant_to_empty() {
        // No user turn at all → empty (the caller turns this into a 400).
        let msgs = vec![
            json!({ "role": "assistant", "content": "a" }),
            json!({ "role": "assistant", "content": "b" }),
        ];
        assert!(sanitize_messages(&msgs).is_empty());
    }

    #[test]
    fn sanitize_appended_extract_prompt_ends_on_user() {
        // chat_issues appends EXTRACT_USER_PROMPT to a transcript ending on the
        // assistant proposal; after sanitize the array is valid and ends on user.
        let msgs = vec![
            json!({ "role": "user", "content": "feature" }),
            json!({ "role": "assistant", "content": "breakdown proposal" }),
            json!({ "role": "user", "content": EXTRACT_USER_PROMPT }),
        ];
        let out = sanitize_messages(&msgs);
        let p = pairs(&out);
        assert_eq!(p.len(), 3);
        assert_eq!(p.last().unwrap().0, "user");
        assert_eq!(p.last().unwrap().1, EXTRACT_USER_PROMPT);
    }

    #[test]
    fn resolve_provider_defaults_to_github() {
        // Absent, empty, and whitespace-only all mean "github" (back-compat with
        // the v0.29.0 GitHub-only contract).
        assert_eq!(resolve_provider(None), Ok(IssueProvider::Github));
        assert_eq!(resolve_provider(Some("")), Ok(IssueProvider::Github));
        assert_eq!(resolve_provider(Some("   ")), Ok(IssueProvider::Github));
    }

    #[test]
    fn resolve_provider_accepts_github_and_linear_trimmed() {
        assert_eq!(resolve_provider(Some("github")), Ok(IssueProvider::Github));
        assert_eq!(resolve_provider(Some("linear")), Ok(IssueProvider::Linear));
        assert_eq!(
            resolve_provider(Some("  linear ")),
            Ok(IssueProvider::Linear)
        );
    }

    #[test]
    fn resolve_provider_rejects_unknown_and_board() {
        // "board" must NOT resolve — the Chat rule forbids the internal board.
        assert_eq!(resolve_provider(Some("board")), Err("board".to_string()));
        assert_eq!(resolve_provider(Some("gitlab")), Err("gitlab".to_string()));
    }

    #[test]
    fn extract_feature_plan_parses_object() {
        let raw = r#"{"title":"CSV export","summary":"Export the board.","tasks":[{"title":"Add button","detail":"Toolbar button.","priority":"high"},{"title":"Serialize","detail":"Rows to CSV.","priority":"medium"}]}"#;
        let plan = extract_feature_plan(raw).expect("must parse");
        assert_eq!(plan.title, "CSV export");
        assert_eq!(plan.tasks.len(), 2);
        assert_eq!(plan.tasks[0].title, "Add button");
        assert_eq!(plan.tasks[1].priority.as_deref(), Some("medium"));
    }

    #[test]
    fn extract_feature_plan_tolerates_fences_and_prose() {
        // The model sometimes wraps the object despite the instruction; the lenient
        // slice-between-braces parse must still recover it. detail/priority are
        // optional, so a terse task still parses.
        let raw = "Here you go:\n```json\n{\"title\":\"T\",\"summary\":\"s\",\"tasks\":[{\"title\":\"X\"}]}\n```\n";
        let plan = extract_feature_plan(raw).expect("must recover from fences/prose");
        assert_eq!(plan.title, "T");
        assert_eq!(plan.tasks.len(), 1);
        assert_eq!(plan.tasks[0].detail, "");
        assert!(plan.tasks[0].priority.is_none());
    }

    #[test]
    fn extract_feature_plan_rejects_empty_or_missing() {
        assert!(extract_feature_plan("no object here").is_none());
        assert!(
            extract_feature_plan(r#"{"title":"","summary":"s","tasks":[{"title":"a"}]}"#).is_none(),
            "blank title = no plan"
        );
        assert!(
            extract_feature_plan(r#"{"title":"T","summary":"s","tasks":[]}"#).is_none(),
            "empty tasks = no plan"
        );
        assert!(extract_feature_plan("} {").is_none(), "reversed braces");
    }

    #[test]
    fn parse_priority_is_lenient() {
        assert_eq!(parse_priority(Some("high")), Priority::High);
        assert_eq!(parse_priority(Some(" P1 ")), Priority::High);
        assert_eq!(parse_priority(Some("CRITICAL")), Priority::High);
        assert_eq!(parse_priority(Some("low")), Priority::Low);
        assert_eq!(parse_priority(Some("p3")), Priority::Low);
        assert_eq!(parse_priority(Some("medium")), Priority::Medium);
        assert_eq!(parse_priority(Some("weird")), Priority::Medium);
        assert_eq!(parse_priority(None), Priority::Medium);
    }

    #[test]
    fn compose_issue_body_sorts_by_priority_keeps_order_and_renders_checklist() {
        // Given low, high, medium → the checklist must come out High→Med→Low, each
        // a checkbox with its priority tag, under one body led by the summary.
        let plan = FeaturePlan {
            title: "Feature".into(),
            summary: "Does a thing.".into(),
            problem: None,
            goal: None,
            tasks: vec![
                SubTask {
                    title: "C low".into(),
                    detail: "cc".into(),
                    priority: Some("low".into()),
                },
                SubTask {
                    title: "A high".into(),
                    detail: "aa".into(),
                    priority: Some("high".into()),
                },
                SubTask {
                    title: "B med".into(),
                    detail: String::new(),
                    priority: None,
                },
            ],
        };
        let body = compose_issue_body(&plan);
        assert!(
            body.starts_with("Does a thing.\n\n## Sub-tasks (priority order)"),
            "summary then heading: {body}"
        );
        let a = body.find("A high").expect("high task present");
        let b = body.find("B med").expect("med task present");
        let c = body.find("C low").expect("low task present");
        assert!(a < b && b < c, "ordered High→Med→Low: {body}");
        assert!(body.contains("- [ ] **[High]** A high — aa"));
        // No detail → no trailing " — detail".
        assert!(
            body.contains("- [ ] **[Medium]** B med\n"),
            "med line: {body}"
        );
    }

    /// Spec 006 F2 pin, written BEFORE the SDD-shape change: a plan with no
    /// `problem`/`goal` must compose the pre-006 body **byte for byte** — the
    /// full-literal net under the riskiest string edit in this feature.
    #[test]
    fn compose_issue_body_without_problem_goal_is_byte_identical() {
        let plan = FeaturePlan {
            title: "Feature".into(),
            summary: "Does a thing.".into(),
            problem: None,
            goal: None,
            tasks: vec![
                SubTask {
                    title: "C low".into(),
                    detail: "cc".into(),
                    priority: Some("low".into()),
                },
                SubTask {
                    title: "A high".into(),
                    detail: "aa".into(),
                    priority: Some("high".into()),
                },
                SubTask {
                    title: "B med".into(),
                    detail: String::new(),
                    priority: None,
                },
            ],
        };
        assert_eq!(
            compose_issue_body(&plan),
            "Does a thing.\n\n\
             ## Sub-tasks (priority order)\n\n\
             - [ ] **[High]** A high — aa\n\
             - [ ] **[Medium]** B med\n\
             - [ ] **[Low]** C low — cc\n"
        );
    }

    /// Spec 006 F2: present-but-blank `problem`/`goal` counts as absent — a
    /// model that emits `""` must not flip the body shape (decision §8).
    #[test]
    fn compose_issue_body_blank_problem_goal_falls_back_to_today() {
        let plan = FeaturePlan {
            title: "Feature".into(),
            summary: "Does a thing.".into(),
            problem: Some("  ".into()),
            goal: Some(String::new()),
            tasks: vec![
                SubTask {
                    title: "C low".into(),
                    detail: "cc".into(),
                    priority: Some("low".into()),
                },
                SubTask {
                    title: "A high".into(),
                    detail: "aa".into(),
                    priority: Some("high".into()),
                },
                SubTask {
                    title: "B med".into(),
                    detail: String::new(),
                    priority: None,
                },
            ],
        };
        assert_eq!(
            compose_issue_body(&plan),
            "Does a thing.\n\n\
             ## Sub-tasks (priority order)\n\n\
             - [ ] **[High]** A high — aa\n\
             - [ ] **[Medium]** B med\n\
             - [ ] **[Low]** C low — cc\n",
            "blank problem/goal must render the pre-006 body byte-identically"
        );
    }

    /// Spec 006 F2 (AC 4): both fields present → SDD shape, in order — summary
    /// lead, `## Problem`, `## Goal`, `## Acceptance criteria` with the SAME
    /// `- [ ]` task lines, and no legacy heading.
    #[test]
    fn compose_issue_body_renders_problem_goal_and_acceptance_criteria() {
        let plan = FeaturePlan {
            title: "Feature".into(),
            summary: "Does a thing.".into(),
            problem: Some("Users lose their drafts.".into()),
            goal: Some("Drafts survive a reload.".into()),
            tasks: vec![
                SubTask {
                    title: "A high".into(),
                    detail: "aa".into(),
                    priority: Some("high".into()),
                },
                SubTask {
                    title: "B med".into(),
                    detail: String::new(),
                    priority: None,
                },
            ],
        };
        let body = compose_issue_body(&plan);
        assert!(
            body.starts_with("Does a thing.\n\n"),
            "summary leads: {body}"
        );
        let p = body.find("## Problem").expect("problem heading");
        let g = body.find("## Goal").expect("goal heading");
        let ac = body.find("## Acceptance criteria").expect("AC heading");
        assert!(p < g && g < ac, "sections in order: {body}");
        assert!(body.contains("Users lose their drafts."));
        assert!(body.contains("Drafts survive a reload."));
        // The checklist rendering is the SHARED code path — identical lines.
        assert!(body.contains("- [ ] **[High]** A high — aa"));
        assert!(body.contains("- [ ] **[Medium]** B med\n"));
        assert!(
            !body.contains("## Sub-tasks (priority order)"),
            "SDD shape replaces the legacy heading: {body}"
        );
        // #256: no boilerplate footer — the body ends with real content.
        assert!(
            !body.contains("_Created from an agentum Chat"),
            "no templated footer: {body}"
        );
    }

    /// Spec 006 F2: the new fields are serde-default — an old client's plan and
    /// a terse model reply (no `problem`/`goal`) still parse.
    #[test]
    fn feature_plan_json_defaults_problem_and_goal() {
        let old: FeaturePlan =
            serde_json::from_str(r#"{"title":"T","summary":"s","tasks":[{"title":"a"}]}"#).unwrap();
        assert!(old.problem.is_none());
        assert!(old.goal.is_none());
        let new: FeaturePlan = serde_json::from_str(
            r#"{"title":"T","summary":"s","problem":"P","goal":"G","tasks":[{"title":"a"}]}"#,
        )
        .unwrap();
        assert_eq!(new.problem.as_deref(), Some("P"));
        assert_eq!(new.goal.as_deref(), Some("G"));
    }

    /// Spec 006 AC 5: an SDD-shaped issue body round-trips through the harness
    /// bridge — `spec_md_from_issue` keeps the `- [ ]` lines parseable and
    /// `derive_backlog_from_spec` yields one feature per task (no fallback).
    #[test]
    fn sdd_issue_body_round_trips_through_spec_md_to_backlog() {
        let plan = FeaturePlan {
            title: "T".into(),
            summary: "Sum.".into(),
            problem: Some("Pain.".into()),
            goal: Some("Outcome.".into()),
            tasks: vec![
                SubTask {
                    title: "First task".into(),
                    detail: "d1".into(),
                    priority: Some("high".into()),
                },
                SubTask {
                    title: "Second task".into(),
                    detail: String::new(),
                    priority: None,
                },
                SubTask {
                    title: "Third task".into(),
                    detail: "d3".into(),
                    priority: Some("low".into()),
                },
            ],
        };
        let body = compose_issue_body(&plan);
        let spec = crate::harness::spec_md_from_issue(
            "42",
            "T",
            &body,
            "https://github.com/o/r/issues/42",
        );
        // The checkboxes were found, so the safety-net `- [ ] T` line (appended
        // only when the body has none) must be absent.
        assert!(
            !spec.contains("- [ ] T\n"),
            "no fallback checkbox — the composed lines parsed: {spec}"
        );
        let backlog = crate::harness::derive_backlog_from_spec(&spec);
        assert_eq!(backlog.features.len(), 3, "one feature per task: {spec}");
        for (f, title) in backlog
            .features
            .iter()
            .zip(["First task", "Second task", "Third task"])
        {
            assert!(
                f.name.contains(title),
                "feature {} carries the task title {title}",
                f.name
            );
        }
    }

    /// Guard a prompt regression without pinning prose: the extraction prompt
    /// must name both SDD fields (AC 4).
    #[test]
    fn extract_instructions_names_problem_and_goal() {
        assert!(EXTRACT_INSTRUCTIONS.contains("\"problem\""));
        assert!(EXTRACT_INSTRUCTIONS.contains("\"goal\""));
        // The raw-JSON tail is load-bearing for `extract_feature_plan`.
        assert!(EXTRACT_INSTRUCTIONS.contains("Output ONLY the raw JSON object"));
    }

    /// Handoff 02 mandatory item (Mateo's empty-description report): pin the
    /// WHOLE chain plan → `plan_to_features` → `compose_issue_body` →
    /// `TaskSink::Github` → `gh` argv with a fake `gh`, asserting the `--body`
    /// value that reaches the process is non-empty and carries the summary plus
    /// a checklist line — not just the composer fn in isolation.
    #[cfg(unix)]
    #[tokio::test]
    // The awaited create must observe AGENTUM_GH_BIN, so the guard spans the
    // await — the same accepted pattern as routes::harness's env-locked test.
    #[allow(clippy::await_holding_lock)]
    async fn chat_plan_body_reaches_gh_create_argv_non_empty() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let store = agentum_store::Store::open(&dir.path().join("t.db"))
            .await
            .unwrap();
        // NUL-separated argv log: the body embeds newlines, so a line-based
        // log could not be split back into arguments unambiguously.
        let log = dir.path().join("argv.log");
        let script = dir.path().join("gh-fake");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\0' \"$a\" >> \"{}\"; done\necho \"https://github.com/o/r/issues/123\"\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let plan = FeaturePlan {
            title: "CSV export".into(),
            summary: "Export the board as CSV.".into(),
            problem: None,
            goal: None,
            tasks: vec![
                SubTask {
                    title: "Add button".into(),
                    detail: "Toolbar button.".into(),
                    priority: Some("high".into()),
                },
                SubTask {
                    title: "Serialize rows".into(),
                    detail: String::new(),
                    priority: Some("medium".into()),
                },
            ],
        };
        let features = plan_to_features(&plan, SplitMode::Single, &[]);
        assert_eq!(features.len(), 1);

        let guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // SAFETY: `set_var` is unsound under concurrent access; the crate-wide
        // TEST_ENV_LOCK serialises every env-mutating test. Only this create
        // resolves `gh_bin()` while the var is set.
        unsafe { std::env::set_var("AGENTUM_GH_BIN", &script) };
        let res = TaskSink::Github
            .create_feature(
                &SinkCtx {
                    store: &store,
                    workdir: dir.path(),
                    parent_goal_id: None,
                    slug: Some("o/r"),
                },
                &features[0],
            )
            .await;
        unsafe { std::env::remove_var("AGENTUM_GH_BIN") };
        drop(guard);
        let fref = res.expect("fake gh create succeeds");
        assert_eq!(fref.id, "123");

        let raw = std::fs::read(&log).expect("the create ran the fake gh");
        let argv: Vec<String> = String::from_utf8_lossy(&raw)
            .split('\0')
            .map(str::to_string)
            .collect();
        let body_idx = argv
            .iter()
            .position(|a| a == "--body")
            .expect("--body flag present in the gh argv");
        let body = argv
            .get(body_idx + 1)
            .expect("--body carries a value")
            .clone();
        assert!(!body.trim().is_empty(), "--body must not be empty");
        assert!(
            body.contains("Export the board as CSV."),
            "body carries the summary: {body}"
        );
        assert!(
            body.contains("- [ ] **[High]** Add button — Toolbar button."),
            "body carries the checklist: {body}"
        );
    }

    #[test]
    fn slug_matches_accepts_owner_repo_only() {
        assert!(slug_matches("owner/repo"));
        assert!(!slug_matches("owner"));
        assert!(!slug_matches("owner/repo/extra"));
        assert!(!slug_matches("owner /repo"));
        assert!(!slug_matches("/repo"));
        assert!(!slug_matches("owner/"));
    }

    // --- repo + harness grounding (the interviewer's context) ---

    #[test]
    fn interviewer_grounds_when_context_present() {
        let with = interviewer_instructions(
            Some("/tmp/proj"),
            Some("o/r"),
            Some("## Repo guide (CLAUDE.md)\nThis project uses axum."),
            Some("### Watchdog\nThe watchdog tails panes and emits AgentCrashed."),
        );
        assert!(
            with.contains("REPO & HARNESS CONTEXT"),
            "context block present"
        );
        assert!(
            with.contains("This project uses axum."),
            "context body inlined"
        );
        assert!(
            with.contains("You HAVE the repo + harness snapshot"),
            "grounded access rule"
        );
        assert!(
            with.contains("RELEVANT WIKI"),
            "retrieved wiki block present"
        );
        assert!(
            with.contains("The watchdog tails panes and emits AgentCrashed."),
            "wiki excerpt inlined"
        );
    }

    #[test]
    fn interviewer_is_honest_blind_when_no_context() {
        let without = interviewer_instructions(Some("/tmp/proj"), None, None, None);
        assert!(
            !without.contains("REPO & HARNESS CONTEXT"),
            "no context block when absent"
        );
        assert!(
            without.contains("no repo snapshot for this chat"),
            "blind access rule"
        );
    }

    // --- spec 008 F2: Fast / Complex intake modes ---

    /// AC 6 — Fast MUST be byte-identical to today's single-prompt interviewer:
    /// the router only delegates, and `stage` never leaks into Fast. Guards a
    /// future refactor from folding Fast into the Socratic path (the pre-006
    /// body-pin technique).
    #[test]
    fn build_intake_instructions_fast_equals_interviewer_verbatim() {
        let wd = Some("/tmp/proj");
        let slug = Some("o/r");
        let repo = Some("## Repo guide (CLAUDE.md)\nThis project uses axum.");
        let wiki = Some("### Watchdog\nTails panes and emits AgentCrashed.");
        // Fast ignores stage — assert across the whole clamp range plus junk.
        for stage in [0u8, 1, 3, 5, 9, 250] {
            assert_eq!(
                build_intake_instructions(IntakeMode::Fast, stage, wd, slug, repo, wiki),
                interviewer_instructions(wd, slug, repo, wiki),
                "Fast must ignore stage and equal interviewer_instructions verbatim"
            );
        }
        // …and with no grounding at all (the honest-blind Fast prompt).
        assert_eq!(
            build_intake_instructions(IntakeMode::Fast, 1, None, None, None, None),
            interviewer_instructions(None, None, None, None)
        );
    }

    /// AC 7 — each Socratic pass covers exactly its one topic and (from pass 2 on)
    /// instructs reflecting the previous answer back; only the FINAL pass points
    /// the user at "Preview issues" (the convergence Fast shares).
    #[test]
    fn socratic_stage_prompts_cover_one_pass_each_and_converge_at_five() {
        let p = |stage| socratic_stage_instructions(stage, Some("/p"), Some("o/r"), None, None);

        let p1 = p(1);
        assert!(p1.contains("PASS 1 — WHO"), "stage 1 is WHO: {p1}");
        assert!(
            p1.contains("nothing to reflect back yet"),
            "stage 1 opens with nothing to reflect: {p1}"
        );
        assert!(
            !p1.contains("Preview issues"),
            "only pass 5 names Preview issues"
        );

        let p2 = p(2);
        assert!(p2.contains("PASS 2 — WHAT"), "stage 2 is WHAT: {p2}");
        assert!(
            p2.contains("reflect the user's previous answer back") && p2.contains("WHO"),
            "stage 2 reflects the WHO back: {p2}"
        );
        assert!(!p2.contains("Preview issues"));

        let p3 = p(3);
        assert!(p3.contains("PASS 3 — WHY"), "stage 3 is WHY: {p3}");
        assert!(
            p3.contains("reflect the previous answer back"),
            "stage 3 reflects: {p3}"
        );
        assert!(!p3.contains("Preview issues"));

        let p4 = p(4);
        assert!(
            p4.contains("PASS 4 — DONE CRITERIA") && p4.contains("acceptance criteria"),
            "stage 4 is done-criteria: {p4}"
        );
        assert!(
            p4.contains("reflect the previous answer back"),
            "stage 4 reflects: {p4}"
        );
        assert!(!p4.contains("Preview issues"));

        let p5 = p(5);
        assert!(
            p5.contains("PASS 5 — RISKS & SCOPE"),
            "stage 5 is risks/scope: {p5}"
        );
        assert!(
            p5.contains("reflect the previous answer back"),
            "stage 5 reflects: {p5}"
        );
        // #257 — the adaptive protocol: every pass carries the control-marker
        // rules (validate-then-re-ask + the three markers), and only pass 5's
        // BODY may gate on `done` (the convergence gate).
        for (stage, prompt) in [(1u8, &p1), (2, &p2), (3, &p3), (4, &p4)] {
            assert!(
                prompt.contains("[[socratic:advance]]")
                    && prompt.contains("[[socratic:stay]]")
                    && prompt.contains("[[socratic:done]]"),
                "stage {stage} carries the control-marker protocol"
            );
            assert!(
                prompt.contains("re-ask this topic"),
                "stage {stage} validates the previous answer adaptively"
            );
        }
        assert!(
            p5.contains("end with [[socratic:done]]") && p5.contains("end with [[socratic:stay]]"),
            "stage 5's body gates convergence on done-vs-stay: {p5}"
        );
        assert!(
            p5.contains("STOP asking questions"),
            "stage 5 stops asking: {p5}"
        );
        assert!(
            p5.contains("Preview issues"),
            "stage 5 converges on Preview issues (AC 7): {p5}"
        );
    }

    /// The server is defensive about `stage` (the client owns advancement, but a
    /// stale/edited localStorage value could arrive out of range): clamp into
    /// 1..=5 — 0 ⇒ pass 1, anything above ⇒ pass 5.
    #[test]
    fn socratic_stage_clamps_out_of_range() {
        assert!(socratic_stage_instructions(0, None, None, None, None).contains("PASS 1 — WHO"));
        assert!(socratic_stage_instructions(9, None, None, None, None).contains("PASS 5 — RISKS"));
        assert!(
            socratic_stage_instructions(250, None, None, None, None).contains("PASS 5 — RISKS")
        );
    }

    /// Socratic reuses the SAME grounding blocks as Fast — the repo snapshot +
    /// grounded access rule when a snapshot is present, the honest-blind rule when
    /// absent — so the staged interview is just as repo-grounded as today's.
    #[test]
    fn socratic_stage_reuses_the_shared_grounding_blocks() {
        let grounded = socratic_stage_instructions(
            2,
            Some("/p"),
            Some("o/r"),
            Some("## Repo guide (CLAUDE.md)\nUses axum."),
            Some("### Watchdog\nTails panes."),
        );
        assert!(
            grounded.contains("REPO & HARNESS CONTEXT"),
            "repo block present"
        );
        assert!(
            grounded.contains("You HAVE the repo + harness snapshot"),
            "grounded access rule"
        );
        assert!(grounded.contains("Uses axum."), "context body inlined");
        assert!(grounded.contains("RELEVANT WIKI"), "wiki block present");
        assert!(grounded.contains("Tails panes."), "wiki excerpt inlined");

        let blind = socratic_stage_instructions(2, None, None, None, None);
        assert!(
            !blind.contains("REPO & HARNESS CONTEXT"),
            "no context block when absent"
        );
        assert!(
            blind.contains("no repo snapshot for this chat"),
            "blind access rule"
        );
    }

    /// The two new fields are serde-default and snake_case: an old client (no
    /// `mode`/`stage`) parses (⇒ Fast), and `"fast"`/`"socratic"` decode.
    #[test]
    fn chat_request_defaults_and_decodes_intake_mode() {
        let old: ChatRequest = serde_json::from_str(r#"{"messages":[]}"#).unwrap();
        assert!(
            old.mode.is_none(),
            "absent mode ⇒ None (⇒ Fast at the handler)"
        );
        assert!(old.stage.is_none());

        let complex: ChatRequest =
            serde_json::from_str(r#"{"messages":[],"mode":"socratic","stage":3}"#).unwrap();
        assert_eq!(complex.mode, Some(IntakeMode::Socratic));
        assert_eq!(complex.stage, Some(3));

        let fast: ChatRequest = serde_json::from_str(r#"{"messages":[],"mode":"fast"}"#).unwrap();
        assert_eq!(fast.mode, Some(IntakeMode::Fast));
    }

    /// AC risk — the no-creds gate BOTH intake handlers run FIRST, independent of
    /// `{mode, stage}`. A hermetic substitute for a live `chat_stream` no-creds
    /// call: on macOS `resolve_auth()` reads the Claude Keychain (a dev machine
    /// with `claude` installed can't be forced to "no creds" via env), so the
    /// invariant is pinned at the shared gate the handler actually calls —
    /// unauthed ⇒ the loud both-paths `NO_CREDS_MSG` 400; authed ⇒ pass-through.
    /// Complex rides this SAME gate (no separate endpoint), so its first turn is
    /// never a silent dead button.
    #[test]
    fn chat_auth_gate_surfaces_no_creds_when_unauthed() {
        // Match only the error (`Auth` deliberately has no Debug — it wraps a
        // secret; success is asserted via `is_ok`, which needs no formatting).
        let err = chat_auth_gate(None)
            .err()
            .expect("unauthed gate must be an error");
        match err {
            ApiError::BadRequest(m) => assert_eq!(m, NO_CREDS_MSG),
            other => panic!("expected BadRequest(NO_CREDS_MSG), got {other:?}"),
        }
        assert!(chat_auth_gate(Some(Auth::ApiKey("sk-ant-test".into()))).is_ok());
        assert!(chat_auth_gate(Some(Auth::Oauth("sk-ant-oat-test".into()))).is_ok());
    }

    #[test]
    fn gather_repo_context_reads_guide_and_manifests() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("CLAUDE.md"),
            "# Guide\nThis project uses axum + sqlx.",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        let ctx = gather_repo_context(Some(dir.path().to_str().unwrap())).expect("context");
        assert!(ctx.contains("Repo guide (CLAUDE.md)"));
        assert!(ctx.contains("This project uses axum + sqlx."));
        // The stack manifest is included so the spec can imitate the real deps.
        assert!(ctx.contains("Root manifests") && ctx.contains("Cargo.toml"));
        assert!(ctx.contains("name = \"demo\""));
    }

    #[test]
    fn gather_repo_context_none_for_missing_or_empty_workdir() {
        assert!(gather_repo_context(None).is_none());
        assert!(gather_repo_context(Some("")).is_none());
        assert!(gather_repo_context(Some("/nonexistent/xyzzy-agentum-chat-test")).is_none());
    }

    #[test]
    fn truncate_chars_is_char_safe_and_marks() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        let t = truncate_chars("hello world", 5);
        assert!(t.starts_with("hello") && t.contains("[truncated]"));
        // Multi-byte: truncating mid-emoji must not panic.
        let _ = truncate_chars("👍👍👍👍", 2);
    }

    // --- streaming + extended thinking (the /api/chat/stream path) ---

    #[test]
    fn parse_sse_delta_extracts_answer_text() {
        let ev = parse_sse_delta(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        )
        .expect("text_delta → text event");
        assert_eq!(ev["type"], "text");
        assert_eq!(ev["text"], "Hello");
    }

    #[test]
    fn parse_sse_delta_extracts_thinking() {
        let ev = parse_sse_delta(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"let me think"}}"#,
        )
        .expect("thinking_delta → thinking event");
        assert_eq!(ev["type"], "thinking");
        assert_eq!(ev["text"], "let me think");
    }

    #[test]
    fn parse_sse_delta_maps_message_stop_to_done() {
        let ev = parse_sse_delta(r#"{"type":"message_stop"}"#).expect("message_stop → done");
        assert_eq!(ev["type"], "done");
    }

    #[test]
    fn parse_sse_delta_maps_error_frame() {
        let ev = parse_sse_delta(
            r#"{"type":"error","error":{"type":"overloaded_error","message":"overloaded"}}"#,
        )
        .expect("error frame → error event");
        assert_eq!(ev["type"], "error");
        assert_eq!(ev["message"], "overloaded");
    }

    #[test]
    fn parse_sse_delta_ignores_unforwarded_frames() {
        // Control/metadata frames and signature deltas carry no user-visible text.
        assert!(parse_sse_delta(r#"{"type":"ping"}"#).is_none());
        assert!(parse_sse_delta(r#"{"type":"message_start","message":{}}"#).is_none());
        assert!(
            parse_sse_delta(
                r#"{"type":"content_block_delta","delta":{"type":"signature_delta","signature":"x"}}"#
            )
            .is_none()
        );
        assert!(parse_sse_delta("not json").is_none());
    }

    #[test]
    fn find_double_newline_locates_frame_boundary() {
        // `event: x\ndata: {}\n\nrest` — the blank line (frame separator) is at 17.
        assert_eq!(find_double_newline(b"event: x\ndata: {}\n\nrest"), Some(17));
        assert_eq!(find_double_newline(b"partial frame, no blank line"), None);
    }

    #[test]
    fn build_stream_payload_off_omits_thinking() {
        let p = build_stream_payload(
            "claude-sonnet-4-6",
            json!("sys"),
            &[json!({ "role": "user", "content": "hi" })],
            false,
        );
        assert_eq!(p["stream"], true);
        assert_eq!(p["max_tokens"], MAX_TOKENS_REPLY);
        assert!(p.get("thinking").is_none());
    }

    #[test]
    fn build_stream_payload_on_enables_thinking_with_headroom() {
        let p = build_stream_payload(
            "claude-opus-4-8",
            json!("sys"),
            &[json!({ "role": "user", "content": "hi" })],
            true,
        );
        assert_eq!(p["thinking"]["type"], "enabled");
        assert_eq!(p["thinking"]["budget_tokens"], THINKING_BUDGET_TOKENS);
        // Anthropic rejects the call unless max_tokens > budget (it caps reasoning + answer).
        assert!(p["max_tokens"].as_u64().unwrap() > u64::from(THINKING_BUDGET_TOKENS));
    }

    // --- spec 003: preview / edit / split (the what-you-see-is-what-you-file gate) ---

    #[test]
    fn split_mode_parse_defaults_to_single() {
        assert_eq!(SplitMode::parse(None), SplitMode::Single);
        assert_eq!(SplitMode::parse(Some("single")), SplitMode::Single);
        assert_eq!(SplitMode::parse(Some("whatever")), SplitMode::Single);
        assert_eq!(SplitMode::parse(Some(" PER_TASK ")), SplitMode::PerTask);
        assert_eq!(SplitMode::parse(Some("per-task")), SplitMode::PerTask);
        assert_eq!(SplitMode::parse(Some("multiple")), SplitMode::PerTask);
    }

    #[test]
    fn plan_to_features_single_is_one_issue_with_checklist() {
        // The confirm/verbatim guarantee: Single produces ONE issue whose title is
        // the plan's title and whose body is the priority checklist — no re-extract.
        let plan = FeaturePlan {
            title: "Feature X".into(),
            summary: "Sum.".into(),
            problem: None,
            goal: None,
            tasks: vec![
                SubTask {
                    title: "A".into(),
                    detail: "aa".into(),
                    priority: Some("high".into()),
                },
                SubTask {
                    title: "B".into(),
                    detail: String::new(),
                    priority: Some("low".into()),
                },
            ],
        };
        let f = plan_to_features(&plan, SplitMode::Single, &["lbl".into()]);
        assert_eq!(f.len(), 1, "single = one issue");
        assert_eq!(f[0].title, "Feature X", "title is the feature verbatim");
        let body = f[0].body.as_deref().unwrap();
        assert!(body.contains("## Sub-tasks (priority order)"));
        assert!(body.contains("- [ ] **[High]** A — aa"));
        assert_eq!(f[0].labels, vec!["lbl".to_string()]);
    }

    #[test]
    fn plan_to_features_per_task_is_one_issue_per_task_priority_ordered() {
        let plan = FeaturePlan {
            title: "Feature X".into(),
            summary: "Sum.".into(),
            problem: None,
            goal: None,
            tasks: vec![
                SubTask {
                    title: "low one".into(),
                    detail: String::new(),
                    priority: Some("low".into()),
                },
                SubTask {
                    title: "high one".into(),
                    detail: "d".into(),
                    priority: Some("high".into()),
                },
            ],
        };
        let f = plan_to_features(&plan, SplitMode::PerTask, &[]);
        assert_eq!(f.len(), 2, "per_task = one issue per task");
        // High sorts first (same stable priority sort as the checklist).
        assert_eq!(f[0].title, "high one");
        assert_eq!(f[1].title, "low one");
        let b0 = f[0].body.as_deref().unwrap();
        assert!(b0.starts_with('d'), "detail leads the body: {b0}");
        assert!(b0.contains("**Priority:** High"));
        assert!(b0.contains("**Feature:** Feature X"));
    }

    #[test]
    fn plan_to_features_threads_labels_to_every_issue() {
        let plan = FeaturePlan {
            title: "F".into(),
            summary: String::new(),
            problem: None,
            goal: None,
            tasks: vec![
                SubTask {
                    title: "t1".into(),
                    detail: String::new(),
                    priority: None,
                },
                SubTask {
                    title: "t2".into(),
                    detail: String::new(),
                    priority: None,
                },
            ],
        };
        let labels = vec!["a".to_string(), "b".to_string()];
        let features = plan_to_features(&plan, SplitMode::PerTask, &labels);
        assert_eq!(features.len(), 2);
        for f in &features {
            assert_eq!(f.labels, labels, "labels ride on every split issue");
        }
    }

    // ── Spec 007: issue-body drafting ────────────────────────────────────

    #[test]
    fn draft_body_prompt_carries_title_and_section_contract() {
        // The user turn names the title; the system instructions pin the
        // SDD sections and the checkbox shape (AC7's prompt-content test).
        let user = draft_body_user_message("Fix the sidebar flicker");
        assert!(user.contains("Fix the sidebar flicker"));

        let instructions = draft_body_instructions(Some("o/r"), Some("## Repo guide\nstuff"));
        assert!(instructions.contains("## Problem"));
        assert!(instructions.contains("## Goal"));
        assert!(instructions.contains("## Acceptance criteria"));
        assert!(instructions.contains("- [ ]"));
        assert!(instructions.contains("`o/r`"));
        assert!(instructions.contains("## Repo guide\nstuff"));

        // Without a snapshot the prompt says so instead of inviting invention.
        let blind = draft_body_instructions(None, None);
        assert!(blind.contains("No repo snapshot is available"));
    }

    #[test]
    fn sanitize_draft_body_unwraps_whole_reply_fences_and_keeps_inner_ones() {
        // A reply that IS one fenced block gets unwrapped…
        assert_eq!(
            sanitize_draft_body("```markdown\n## Problem\nx\n```"),
            "## Problem\nx"
        );
        assert_eq!(sanitize_draft_body("```\nbody\n```"), "body");
        // …but fences inside a longer body are legitimate markdown.
        let mixed = "## Problem\n```sh\nls\n```\ndone";
        assert_eq!(sanitize_draft_body(mixed), mixed);
        assert_eq!(sanitize_draft_body("  plain  "), "plain");
    }
}
