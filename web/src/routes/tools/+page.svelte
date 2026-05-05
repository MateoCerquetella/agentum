<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api, type Session } from '$lib/api';
  import Icon from '$components/Icon.svelte';

  interface AdapterInfo {
    name: string;
    icon: string;
    description: string;
    tools: Array<{ name: string; description: string; args: string }>;
    env: Array<{ key: string; value: string }>;
    features: string[];
  }

  const ADAPTERS: AdapterInfo[] = [
    {
      name: 'claude',
      icon: 'cpu',
      description: 'Anthropic Claude Code CLI — first-class adapter with compact support, model switching, and MCP tool integration.',
      tools: [
        { name: 'code:edit', description: 'Edit files in the codebase', args: 'path, content, instructions' },
        { name: 'code:analyze', description: 'Analyze code structure and patterns', args: 'query, scope' },
        { name: 'shell:exec', description: 'Run shell commands in the workspace', args: 'command, workdir?, timeout?' },
        { name: 'fs:read', description: 'Read file contents', args: 'path, offset?, limit?' },
        { name: 'fs:write', description: 'Write or create files', args: 'path, content' },
        { name: 'mcp:tools', description: 'Discover and invoke MCP server tools', args: 'server, tool, params' },
        { name: 'mcp:resources', description: 'Access MCP server resources', args: 'server, uri' },
        { name: 'mcp:prompts', description: 'Use MCP server prompt templates', args: 'server, name, args' }
      ],
      env: [
        { key: 'ANTHROPIC_API_KEY', value: 'required' },
        { key: 'CLAUDE_MODEL', value: 'claude-sonnet-4-20250514' }
      ],
      features: [
        'Atomic compact on context-low',
        'Model switching via --model flag',
        'MCP server integration',
        'Permission system with approval modes',
        'Sub-agent spawning'
      ]
    },
    {
      name: 'codex',
      icon: 'zap',
      description: 'OpenAI Codex CLI — general-purpose coding agent with function calling and parallel tool execution.',
      tools: [
        { name: 'edit', description: 'Edit files', args: 'file, old, new' },
        { name: 'search', description: 'Search codebase', args: 'pattern, files?' },
        { name: 'exec', description: 'Execute commands', args: 'command, workdir?' },
        { name: 'read', description: 'Read files', args: 'path' }
      ],
      env: [
        { key: 'OPENAI_API_KEY', value: 'required' },
        { key: 'OPENAI_MODEL', value: 'gpt-5.1-codex' }
      ],
      features: [
        'Function calling with structured output',
        'Parallel tool execution',
        'Streaming responses',
        'Token usage tracking'
      ]
    },
    {
      name: 'gemini',
      icon: 'grid',
      description: 'Google Gemini CLI — multimodal agent with vision capabilities and long-context support.',
      tools: [
        { name: 'file:edit', description: 'Edit source files', args: 'path, edit' },
        { name: 'file:read', description: 'Read file contents', args: 'path' },
        { name: 'shell:run', description: 'Execute shell commands', args: 'command, cwd?' },
        { name: 'browser:view', description: 'View visual output', args: 'url, screenshot?' }
      ],
      env: [
        { key: 'GOOGLE_API_KEY', value: 'required' },
        { key: 'GEMINI_MODEL', value: 'gemini-2.5-pro' }
      ],
      features: [
        'Multimodal (vision) support',
        '2M token context window',
        'Structured JSON output',
        'Grounding with Google Search'
      ]
    },
    {
      name: 'hermes',
      icon: 'shield',
      description: 'Nous Hermes — open-source local agent powered by Hermes-function-calling models, runs fully offline.',
      tools: [
        { name: 'edit', description: 'Edit code files', args: 'file, patch' },
        { name: 'read', description: 'Read files', args: 'path' },
        { name: 'run', description: 'Run commands', args: 'cmd' },
        { name: 'grep', description: 'Search in files', args: 'pattern, path' }
      ],
      env: [
        { key: 'HERMES_ENDPOINT', value: 'http://127.0.0.1:8080/v1' },
        { key: 'HERMES_MODEL', value: 'hermes-3-8b' }
      ],
      features: [
        'Fully offline operation',
        'No API keys needed',
        'Open-source model',
        'Function-calling grammar'
      ]
    },
    {
      name: 'passthrough',
      icon: 'terminal',
      description: 'Generic passthrough — run any binary on PATH as an AI agent. agentum handles the tmux lifecycle.',
      tools: [],
      env: [{ key: 'TOOL_BINARY', value: 'any binary on $PATH' }],
      features: [
        'Any binary on PATH',
        'Stdio over tmux pane',
        'Full lifecycle management',
        'Watchdog monitoring'
      ]
    }
  ];

  let sessions = $state<Session[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let expanded = $state<Set<string>>(new Set());
  let toolFilter = $state('');
  let selectedAdapter = $state<string | null>(null);

  onMount(async () => {
    try {
      sessions = await api.listSessions();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  });

  function toggle(name: string) {
    expanded = new Set(expanded);
    if (expanded.has(name)) expanded.delete(name);
    else expanded.add(name);
  }

  function sessionCount(tool: string): number {
    return sessions.filter(s => s.tool === tool).length;
  }

  function filteredAdapters(): AdapterInfo[] {
    if (!toolFilter.trim()) return ADAPTERS;
    const q = toolFilter.toLowerCase();
    return ADAPTERS.filter(a =>
      a.name.toLowerCase().includes(q) ||
      a.description.toLowerCase().includes(q) ||
      a.tools.some(t => t.name.toLowerCase().includes(q) || t.description.toLowerCase().includes(q))
    );
  }

  function highlightTool(name: string): boolean {
    if (!toolFilter.trim()) return false;
    return name.toLowerCase().includes(toolFilter.toLowerCase());
  }
</script>

<section class="head">
  <div>
    <h2>Tools & Adapters</h2>
    <p class="muted">MCP-style capability registry. Each adapter exposes tools, required environment variables, and features.</p>
  </div>
  <div class="search-wrap">
    <input
      type="text"
      bind:value={toolFilter}
      placeholder="filter tools…"
      class="search"
    />
  </div>
</section>

{#if error}
  <div class="error">{error}</div>
{/if}

<div class="adapters">
  {#each filteredAdapters() as adapter (adapter.name)}
    <div class="card">
      <button
        class="card-header"
        onclick={() => toggle(adapter.name)}
        class:expanded={expanded.has(adapter.name)}
      >
        <div class="header-left">
          <span class="adapter-icon">
            <Icon name={adapter.icon} size={20} />
          </span>
          <div>
            <div class="adapter-name">{adapter.name}</div>
            <div class="adapter-sessions mono">
              {sessionCount(adapter.name)} session{sessionCount(adapter.name) !== 1 ? 's' : ''}
            </div>
          </div>
        </div>
        <span class="chevron">{expanded.has(adapter.name) ? '▾' : '▸'}</span>
      </button>

      <div class="card-body" class:open={expanded.has(adapter.name)}>
        <p class="desc">{adapter.description}</p>

        {#if adapter.features.length > 0}
          <div class="section">
            <div class="section-title">Features</div>
            <div class="tags">
              {#each adapter.features as f}
                <span class="tag">{f}</span>
              {/each}
            </div>
          </div>
        {/if}

        {#if adapter.tools.length > 0}
          <div class="section">
            <div class="section-title">Tools ({adapter.tools.length})</div>
            <div class="tools-list">
              {#each adapter.tools as tool (tool.name)}
                <div
                  class="tool-row"
                  class:highlighted={highlightTool(tool.name)}
                >
                  <code class="tool-name">{tool.name}</code>
                  <span class="tool-desc">{tool.description}</span>
                  {#if tool.args}
                    <code class="tool-args">({tool.args})</code>
                  {/if}
                </div>
              {/each}
            </div>
          </div>
        {/if}

        {#if adapter.env.length > 0}
          <div class="section">
            <div class="section-title">Environment</div>
            <div class="env-list">
              {#each adapter.env as e}
                <div class="env-row">
                  <code class="env-key">{e.key}</code>
                  <code class="env-val">{e.value}</code>
                </div>
              {/each}
            </div>
          </div>
        {/if}
      </div>
    </div>
  {/each}
</div>

<style>
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
    margin-bottom: 1rem;
    flex-wrap: wrap;
  }
  h2 {
    font-family: var(--font-display);
    font-weight: 600;
    margin: 0 0 0.2rem;
    font-size: 1.4rem;
  }
  .muted { color: var(--muted); margin: 0; font-size: 0.85rem; }
  .search-wrap { flex-shrink: 0; }
  .search {
    padding: 0.45rem 0.8rem;
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 0.85rem;
    width: 200px;
  }
  .search:focus { outline: none; border-color: color-mix(in srgb, var(--accent) 50%, var(--border)); }
  .mono { font-family: var(--font-mono); }
  .error {
    padding: 0.7rem 1rem;
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    font-family: var(--font-mono);
    margin-bottom: 0.6rem;
  }

  .adapters {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  .card {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    overflow: hidden;
    transition: border-color var(--transition, 150ms ease);
  }
  .card:hover {
    border-color: color-mix(in srgb, var(--accent) 25%, var(--border));
  }
  .card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: 0.8rem 1rem;
    cursor: pointer;
    transition: background var(--transition, 150ms ease);
  }
  .card-header:hover { background: var(--surface-2); }
  .card-header.expanded { border-bottom: 1px solid var(--border); }
  .header-left {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }
  .adapter-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--accent) 12%, var(--surface-2));
    color: var(--accent);
  }
  .adapter-name {
    font-family: var(--font-display);
    font-weight: 600;
    font-size: 1rem;
    color: var(--text);
    text-align: left;
  }
  .adapter-sessions {
    font-size: 0.72rem;
    color: var(--muted);
    text-align: left;
  }
  .chevron {
    font-family: var(--font-mono);
    color: var(--muted);
    font-size: 0.85rem;
  }

  .card-body {
    display: none;
    padding: 1rem;
    flex-direction: column;
    gap: 0.9rem;
  }
  .card-body.open { display: flex; }
  .desc {
    margin: 0;
    color: var(--text-2);
    font-size: 0.9rem;
    line-height: 1.55;
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .section-title {
    font-family: var(--font-display);
    font-size: 0.78rem;
    font-weight: 600;
    color: var(--text-2);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
  }
  .tag {
    font-family: var(--font-mono);
    font-size: 0.72rem;
    padding: 0.2em 0.6em;
    border-radius: 999px;
    background: color-mix(in srgb, var(--accent) 10%, var(--surface-2));
    color: var(--accent);
    border: 1px solid color-mix(in srgb, var(--accent) 25%, var(--border));
  }

  .tools-list {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .tool-row {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    padding: 0.35rem 0.6rem;
    border-radius: 5px;
    flex-wrap: wrap;
  }
  .tool-row.highlighted {
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }
  .tool-name {
    font-family: var(--font-mono);
    font-size: 0.8rem;
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, var(--surface-2));
    padding: 0.05em 0.45em;
    border-radius: 3px;
  }
  .tool-desc {
    font-size: 0.82rem;
    color: var(--text-2);
  }
  .tool-args {
    font-family: var(--font-mono);
    font-size: 0.7rem;
    color: var(--muted);
  }

  .env-list {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .env-row {
    display: flex;
    gap: 1rem;
    align-items: center;
  }
  .env-key {
    font-family: var(--font-mono);
    font-size: 0.78rem;
    color: var(--text);
    min-width: 14rem;
  }
  .env-val {
    font-family: var(--font-mono);
    font-size: 0.72rem;
    color: var(--muted);
  }
</style>
