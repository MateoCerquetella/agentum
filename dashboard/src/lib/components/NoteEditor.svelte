<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { EditorView, basicSetup } from 'codemirror';
  import { EditorState } from '@codemirror/state';
  import { markdown } from '@codemirror/lang-markdown';
  import { oneDark } from '@codemirror/theme-one-dark';

  interface Props {
    value: string;
    /** Called with new doc content. Should NOT echo back into `value`. */
    onchange: (text: string) => void;
    /** Optional explicit save trigger (e.g. blur). */
    onflush?: () => void;
    /** Theme variant — keep CodeMirror's palette in sync with the app. */
    dark: boolean;
  }
  let { value, onchange, onflush, dark }: Props = $props();

  let host: HTMLDivElement;
  let view: EditorView | null = null;

  onMount(() => {
    const state = EditorState.create({
      doc: value,
      extensions: [
        basicSetup,
        markdown(),
        ...(dark ? [oneDark] : []),
        EditorView.lineWrapping,
        EditorView.updateListener.of((u) => {
          if (u.docChanged) onchange(u.state.doc.toString());
        }),
        EditorView.theme(
          {
            '&':            { height: '100%' },
            '.cm-scroller': { fontFamily: 'var(--font-mono)' },
            '.cm-content':  { fontFamily: 'var(--font-mono)', fontSize: '14px' },
            '.cm-gutters':  { background: 'transparent' }
          },
          { dark }
        )
      ]
    });
    view = new EditorView({ state, parent: host });
  });

  onDestroy(() => {
    view?.destroy();
    view = null;
  });

  // Reset doc when the parent passes a different `value` (e.g. switching notes).
  $effect(() => {
    const next = value;
    if (!view) return;
    const cur = view.state.doc.toString();
    if (cur === next) return;
    view.dispatch({
      changes: { from: 0, to: cur.length, insert: next }
    });
  });
</script>

<div class="editor" bind:this={host} onfocusout={() => onflush?.()} role="textbox" tabindex="-1"></div>

<style>
  .editor {
    flex: 1;
    min-height: 320px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface-2);
    overflow: hidden;
    display: flex;
  }
  .editor :global(.cm-editor) { flex: 1; min-height: 0; }
  .editor :global(.cm-editor.cm-focused) { outline: none; }
</style>
