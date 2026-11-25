# Moving around threads: old Agentum vs BB, and what Agentum takes

Compared from source on 2026-11-24 (old Agentum desktop v0.98.1 and TUI; BB 0.50.1).

| Capability | Old Agentum | BB | Better | Agentum (this) |
|---|---|---|---|---|
| List and grouping | host → project → workspace; grouped by state | pinned, sections, by project/machine/time | tie | Pinned, **Needs you**, **Ready**, **Working**, then by project; Older folded |
| Switching | ⌘J palette by recent visit; one key flips back | ⌘[ ] back/forward, ⌘⇧[ ] prev/next | Agentum | ⌘K empty lists threads by last visit (the open one last: ⌘K ⏎ flips back); ⌘⇧[ ] prev/next |
| Keyboard | big registry, ⌘1–9 hard-coded | registry with contexts and overrides | BB | ⌘1–9 follow the sidebar's visible order (computed from data, not the DOM) |
| Palette | four palettes; TUI prefixes `>` `#` `@` | one, fuzzy, recents first | BB | one palette with prefixes: `>` commands, `#` threads, `@` machines |
| Search | names, branch, repo, PR | what was said, with excerpts, jump to message | BB | titles instantly + full-text (FTS5) of what was said; excerpt with marks; opens at the message |
| Panes | terminal/browser/editor tabs, splits | up to 8 thread panes | tie | two panes (⌥-click opens on the right) + a side panel of plugin tabs per thread |
| Attention | Needs You / Active / Settled queue | 12 indicators + read marks + unread divider | tie | the queue *and* read marks: a finished turn is Ready until opened |
| Machines | host health, inline reconnect | grey out offline hosts in the picker | Agentum | machine picker marks online/offline; offline can't be picked; sidebar shows machines online |
| Organising | pin, archive, sleep; order kept per device | pins, sections, server-side order | BB | pin/archive on the hub (every device agrees); order field ready for drag reorder |
| Fork / handoff | fork a pane | fork from a message, hand off to another agent | BB | side chats (a fork with the parent's context) |
| Side chat | none | side panel, hidden from sidebar | BB | side chats are child threads, hidden from the sidebar's list |
| Mobile | PWA | WebView app, push opens the thread | BB | PWA + push (notify plugin) opening the thread |

Slow or clumsy in both, avoided here: shortcut differences per client, jump targets found by
scanning the DOM on every key press, search waiting for 2 characters plus a delay, layouts saved
only in one browser, sort order jumping on a cold start (Agentum's summaries come from the hub, so
the first paint already has the final order).
