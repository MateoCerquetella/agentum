# Landing redesign — apply the Agentum Design System (ara.so direction)

**Date:** 2026-06-09
**Status:** Approved (design)
**Scope:** Foundations + marketing landing page (`web/`). Desktop app reskin is a
deliberate follow-up, NOT part of this work.

---

## Goal

Replace the marketing site's old visual language (Space Grotesk + IBM Plex Mono,
coral CTA `#f36458`, `#0b0b0b` field) with the finished **Agentum Design System**
(`~/Downloads/Agentum Design System`), which matches **ara.so**: warm near-black
`#141414` field, **system-ui everywhere** (no webfont), white-pill CTAs, Lucide
icons, and a scattered-glyph hero background. The page should read as a sibling of
ara.so while remaining unmistakably Agentum's own (real content, real links, our
own background implementation).

## Non-goals

- No reskin of the Tauri desktop app (`crates/agentum-desktop/ui/`) in this pass.
- No copying of ara.so's proprietary canvas/shader code. Our background is written
  from scratch.
- No new marketing copy beyond recasting existing real content into the new layout.

---

## Inputs

- **Design system:** `~/Downloads/Agentum Design System/` — `styles.css`,
  `tokens/*.css` (colors, themes, typography, spacing, elevation, base, fonts),
  `assets/*` (mark, wordmark, app-icon, favicon), and
  `ui_kits/marketing/index.html` (the ara-matched reference layout).
- **Aesthetic reference:** ara.so (inspected live) — fixed full-bleed `<canvas>`
  scattered-glyph field over `#141414`, system-ui font, white-pill CTA, YC
  announcement bar, macOS product window below the hero.
- **Current site:** `web/index.html` (2287 lines, self-contained). Must preserve
  its SEO head, both JSON-LD blocks, GitHub link
  (`github.com/MateoCerquetella/agentum`), live `#gh-stars` counter, and the
  copy-install-command behaviour (`#copy-btn` / `#install-cmd` / `#cli-list`).

---

## Deliverables

### 1. Foundations — design system as linked files
- Copy `styles.css`, `tokens/`, and `assets/` from the design system into
  `web/ds/` (tokens) and `web/assets/` (brand). The page links `ds/styles.css`.
- Tokens become a reusable repo artifact the desktop app can adopt later.
- **Fonts:** remove the Google Fonts `<link>` + `preconnect`s. All type resolves to
  `system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif` via the
  design-system typography tokens.
- **Icons:** Lucide-style inline SVGs (24×24 viewBox, 2px stroke, round caps/joins,
  no fill, `currentColor`). Replace any off-system glyphs.
- **Brand assets:** new `favicon.svg`, `mark.svg`, `wordmark.svg`, `app-icon`.

### 2. Animated Braille background — faithful port of ara.so (inline `<canvas>`)
> **Decision change (per user):** rather than an original recreation, copy ara's
> actual background. Extracted from the provided `ara.webarchive`
> (`index-VAV34NYo.js`) and ported verbatim to vanilla JS.

- A single vanilla-JS module, no dependencies; `<canvas id="bg-glyphs">` at
  `position:fixed; inset:0` behind all content, DPR-aware.
- ara's exact algorithm: an **80×80 grid** where each cell's field value is the sum
  of **4 random sine-wave emitters** (placed in the central 50%, freq 0.2–0.5, amp
  0.5–1.0, random phase/speed) **+ a wave that follows the cursor** + **click
  ripples** (expanding rings, 4 s life). The field intensity selects a **Unicode
  Braille glyph** (U+2800–U+28FF, ara's exact 361-char weighted set) and its opacity
  (0.4–0.9).
- Type: `350 <min(cellW,cellH)*0.8>px system-ui`; colour `rgba(190,190,190, …)` on
  `#141414` (dark) / `rgba(120,120,120, …)` on `#f7f7f7` (light) — ara's values.
- `animationSpeed 0.75`, frame-rate-independent phase advance.
- **Animates regardless of `prefers-reduced-motion`** (matches ara; it's an
  opacity-only ambient field, no transform/flashing). The user explicitly requested
  motion; the earlier reduced-motion gate was the cause of the "not moving" report.
- Paints one frame **synchronously** at load (never blank, even while the tab is
  hidden); the rAF loop is **capped to ~30fps** to keep CPU/fans calm and pauses
  automatically when the tab is backgrounded.
- Re-renders on resize (debounced).

### 3. Rebuild `web/index.html` to the ara layout
Structure (from the marketing kit, populated with real content):
1. Announcement bar (open-source / MIT, or current message).
2. Sticky nav with `backdrop-filter` blur — wordmark left, center links, GitHub
   star pill + white "Get started" pill + dark/light toggle right.
3. Hero — app icon, display headline (`system-ui`, weight 500, `-0.035em`), grey
   lede, white-pill primary CTA + secondary install hint, over the glyph canvas.
4. Product window (macOS chrome) framing a live-terminal mock — the Agentum app.
5. Trust row — agent CLIs Agentum drives (Claude, Codex, Gemini, Hermes, Cursor,
   OpenCode) with muted agent-hue dots.
6. "The loop" — alternating feature blocks (Sessions, Watchdog & skills, Board).
7. Mission control — wide multi-pane window.
8. Final CTA — headline + install command (real `cargo install` / curl, copyable).
9. Footer — brand, product/resources/project link columns, legal line.

Preserve from the current page: full SEO `<head>`, both JSON-LD blocks, canonical,
OG/Twitter tags, GitHub URL, the `#gh-stars` fetch, and the copy-to-clipboard
install behaviour. Keep the dark/light toggle wired to the design-system
`.theme-light` class.

---

## Acceptance criteria

- No Google Fonts request; computed `font-family` on `body` is `system-ui`-led.
- CTA is a white pill with `#141414` ink in dark mode (inverts in light mode); no
  coral remains anywhere.
- Page background is `#141414` with the animated scattered-glyph canvas visible
  behind the hero and fading behind centered content.
- Dark/light toggle flips the whole page (including glyph color) via `.theme-light`.
- SEO head, both JSON-LD blocks, GitHub link, live star count, and copy-install all
  still function.
- Side-by-side in Chrome (MCP), the hero reads as the same family as ara.so:
  near-black field, scattered glyph texture, system font, white pill CTA.
- `prefers-reduced-motion` disables the background animation.

## Verification

Open the rebuilt `web/index.html` in Chrome via MCP, screenshot at 1440-wide, and
compare against ara.so. Toggle light/dark. Confirm no font network request and no
console errors.
