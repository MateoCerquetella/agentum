<script lang="ts">
  /**
   * DiffView — CodeMirror 6 side-by-side diff (ORCA §2 "richer diff/editor").
   *
   * Replaces SessionGitPanel's old plain-text unified-diff render. Takes two
   * file revisions (`original` / `modified`) and the filename, and shows a
   * read-only `MergeView` with per-language syntax highlighting and collapsed
   * unchanged regions. Language is picked by file extension; unknown types
   * fall back to no highlighting (still diffed).
   *
   * Tree-shaken CodeMirror is a few hundred KB — far lighter than Monaco —
   * which keeps the embedded SPA small (the rebuild-rhythm bundle).
   */
  import { onDestroy } from 'svelte';
  import { MergeView } from '@codemirror/merge';
  import { EditorView } from '@codemirror/view';
  import { EditorState, type Extension } from '@codemirror/state';
  import { oneDark } from '@codemirror/theme-one-dark';
  import { javascript } from '@codemirror/lang-javascript';
  import { rust } from '@codemirror/lang-rust';
  import { python } from '@codemirror/lang-python';
  import { json } from '@codemirror/lang-json';
  import { html } from '@codemirror/lang-html';
  import { css } from '@codemirror/lang-css';
  import { markdown } from '@codemirror/lang-markdown';

  interface Props {
    original: string;
    modified: string;
    filename: string;
  }
  let { original, modified, filename }: Props = $props();

  let host = $state<HTMLDivElement | null>(null);
  let view: MergeView | null = null;

  /** Pick a language extension from the file extension. Kept to the modes
   *  this repo (and most agent worktrees) actually hit — Rust, JS/TS/Svelte,
   *  Python, JSON, HTML, CSS, Markdown — to bound bundle size. */
  function langFor(name: string): Extension[] {
    const ext = name.split('.').pop()?.toLowerCase() ?? '';
    switch (ext) {
      case 'ts':
      case 'tsx':
      case 'mts':
      case 'cts':
        return [javascript({ typescript: true, jsx: ext === 'tsx' })];
      case 'js':
      case 'jsx':
      case 'mjs':
      case 'cjs':
      case 'svelte': // close enough: highlights the script/markup reasonably
        return [javascript({ jsx: ext === 'jsx' })];
      case 'rs':
        return [rust()];
      case 'py':
        return [python()];
      case 'json':
        return [json()];
      case 'html':
      case 'htm':
        return [html()];
      case 'css':
      case 'scss':
      case 'less':
        return [css()];
      case 'md':
      case 'markdown':
        return [markdown()];
      default:
        return [];
    }
  }

  function destroy() {
    view?.destroy();
    view = null;
  }

  // Build (and rebuild on any input change). $effect re-runs when the tracked
  // props change; the returned closure tears the old view down first.
  $effect(() => {
    // Track inputs so the effect re-runs when they change.
    const a = original;
    const b = modified;
    const fname = filename;
    if (!host) return;
    destroy();
    const common: Extension[] = [
      EditorView.editable.of(false),
      EditorState.readOnly.of(true),
      EditorView.lineWrapping,
      oneDark,
      ...langFor(fname)
    ];
    view = new MergeView({
      a: { doc: a, extensions: common },
      b: { doc: b, extensions: common },
      parent: host,
      gutter: true,
      highlightChanges: true,
      collapseUnchanged: { margin: 3, minSize: 4 }
    });
    return () => destroy();
  });

  onDestroy(destroy);
</script>

<div class="diffview" bind:this={host}></div>

<style>
  .diffview {
    max-height: 360px;
    overflow: auto;
    border-radius: var(--radius);
    font-size: 11.5px;
  }
  /* CodeMirror renders its own DOM inside .diffview; reach it with :global. */
  :global(.diffview .cm-mergeView) {
    background: #050505;
  }
  :global(.diffview .cm-editor) {
    background: transparent;
  }
  :global(.diffview .cm-mergeView .cm-editor.cm-focused) {
    outline: none;
  }
</style>
