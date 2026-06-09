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

### 2. Animated glyph background (`web/ds/bg-glyphs.js` or inline)
- A single vanilla-JS module, no dependencies.
- Renders a `<canvas>` at `position:fixed; inset:0; z-index:-10` on the `#141414`
  field, behind all content.
- Draws a scattered field of faint monospace glyphs sampled from a terminal-ish set
  (`$ › ● ✓ ⚠ { } [ ] / 0 1 _ — +`), placed on a jittered grid with per-cell random
  opacity in the `rgba(255,255,255, 0.02–0.10)` range.
- Radial alpha mask: denser toward the edges, dissolving behind the centered hero
  (mirrors ara's mask).
- Subtle slow animation: occasional per-glyph flicker / drift; cheap (throttled rAF,
  off-DOM).
- `prefers-reduced-motion: reduce` → render one static frame, no loop.
- Re-renders on resize (debounced). DPR-aware for crisp glyphs.
- Light mode: glyphs flip to `rgba(0,0,0, …)` over the white field.

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
