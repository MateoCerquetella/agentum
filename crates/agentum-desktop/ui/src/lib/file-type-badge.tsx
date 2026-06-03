import React from 'react'
import { getFileTypeIcon } from '@/lib/file-type-icons'

// Colored letter-badge file icons for the file explorer, matching the orca/VS Code
// "Seti" aesthetic (e.g. `rs` orange, `TS` blue, `N` nix, `#` css). Kept separate
// from getFileTypeIcon (which returns Lucide glyphs used by tabs / quick-open) so
// only the explorer rows get badges and other consumers are unaffected.

type Badge = { label: string; color: string }

// Extension → { 1-2 char label, accent color }. Color is used for both the text
// and a translucent background so badges read in light and dark themes.
const BADGE_BY_EXT: Record<string, Badge> = {
  rs: { label: 'rs', color: '#dea584' },
  ts: { label: 'TS', color: '#3178c6' },
  tsx: { label: 'TS', color: '#3178c6' },
  js: { label: 'JS', color: '#f0db4f' },
  jsx: { label: 'JS', color: '#f0db4f' },
  mjs: { label: 'JS', color: '#f0db4f' },
  cjs: { label: 'JS', color: '#f0db4f' },
  svelte: { label: 'S', color: '#ff3e00' },
  vue: { label: 'V', color: '#42b883' },
  py: { label: 'py', color: '#4b8bbe' },
  rb: { label: 'rb', color: '#cc342d' },
  go: { label: 'go', color: '#00add8' },
  md: { label: 'md', color: '#8aa1b4' },
  mdx: { label: 'md', color: '#8aa1b4' },
  json: { label: '{}', color: '#cbcb41' },
  toml: { label: 'T', color: '#9c8d7b' },
  yaml: { label: 'Y', color: '#cb4b16' },
  yml: { label: 'Y', color: '#cb4b16' },
  nix: { label: 'N', color: '#7e7eff' },
  css: { label: '#', color: '#42a5f5' },
  scss: { label: '#', color: '#cd6799' },
  html: { label: '<>', color: '#e44d26' },
  sh: { label: '$', color: '#89e051' },
  bash: { label: '$', color: '#89e051' },
  fish: { label: '$', color: '#89e051' },
  sql: { label: 'sql', color: '#dad8d8' }
}

function extensionOf(name: string): string {
  const base = name.split(/[\\/]/).pop() ?? name
  const dot = base.lastIndexOf('.')
  return dot > 0 ? base.slice(dot + 1).toLowerCase() : ''
}

export function FileTypeBadge({
  name,
  className
}: {
  name: string
  className?: string
}): React.JSX.Element {
  const badge = BADGE_BY_EXT[extensionOf(name)]
  if (!badge) {
    // Fall back to the Lucide glyph for unmapped types.
    const Icon = getFileTypeIcon(name)
    return <Icon className={className ?? 'size-3 shrink-0 text-muted-foreground'} />
  }
  return (
    <span
      aria-hidden="true"
      className="inline-flex size-3.5 shrink-0 items-center justify-center rounded-[3px] font-mono text-[7px] font-semibold leading-none"
      style={{ color: badge.color, backgroundColor: `${badge.color}24` }}
    >
      {badge.label}
    </span>
  )
}
