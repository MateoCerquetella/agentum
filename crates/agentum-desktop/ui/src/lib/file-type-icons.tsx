import React from 'react'
import type { LucideIcon } from 'lucide-react'
import { cn } from '@/lib/utils'
import {
  getFileExtension,
  getLucideFileIcon,
  getLucideFileIconCategory,
  type LucideFileIconCategory
} from './lucide-file-icons'

// File-type icons. For recognised source/config extensions we render a small
// colored monospace label (`rs`, `TS`, `S`, `#`, `N`, …) — the "Seti" badge
// aesthetic from the file explorer — and for everything else fall back to the
// Lucide glyph table. The same resolver feeds the explorer, tabs, quick-open,
// source-control and the diff trees so a file looks identical everywhere.
//
// Why extension precedence (badge before named-file Lucide icon): the explorer
// has always badged by extension, so `README.md`→`md` and `Cargo.toml`→`T`
// even though both have dedicated named icons. Keeping that rule is what makes
// the badges match the look users already see in the tree.

export type FileIconComponent = React.ComponentType<{ className?: string }>

// Color group → label. One label per color group (aliases share a group).
const BADGE_LABEL_BY_GROUP: Record<string, string> = {
  rs: 'rs',
  ts: 'TS',
  js: 'JS',
  svelte: 'S',
  vue: 'V',
  py: 'py',
  rb: 'rb',
  go: 'go',
  md: 'md',
  json: '{}',
  toml: 'T',
  yaml: 'Y',
  nix: 'N',
  css: '#',
  scss: '#',
  html: '<>',
  sh: '$',
  java: 'jv',
  c: 'C',
  cpp: 'C+',
  swift: 'sw',
  kt: 'kt',
  php: 'ph'
}

// Extension → color group. The group keys the per-theme color in main.css
// (`[data-ft="…"]`). Aliases (tsx→ts, bash→sh, …) collapse onto one group.
const BADGE_GROUP_BY_EXT: Record<string, string> = {
  rs: 'rs',
  ts: 'ts',
  tsx: 'ts',
  mts: 'ts',
  cts: 'ts',
  js: 'js',
  jsx: 'js',
  mjs: 'js',
  cjs: 'js',
  svelte: 'svelte',
  vue: 'vue',
  py: 'py',
  rb: 'rb',
  go: 'go',
  md: 'md',
  mdx: 'md',
  json: 'json',
  toml: 'toml',
  yaml: 'yaml',
  yml: 'yaml',
  nix: 'nix',
  css: 'css',
  scss: 'scss',
  sass: 'scss',
  html: 'html',
  htm: 'html',
  sh: 'sh',
  bash: 'sh',
  zsh: 'sh',
  fish: 'sh',
  java: 'java',
  c: 'c',
  h: 'c',
  cpp: 'cpp',
  cc: 'cpp',
  cxx: 'cpp',
  hpp: 'cpp',
  hh: 'cpp',
  hxx: 'cpp',
  swift: 'swift',
  kt: 'kt',
  kts: 'kt',
  php: 'php',
  phtml: 'php'
}

/** Badge color group for a path, or null when no badge applies (Lucide instead). */
export function fileTypeBadgeGroup(filePath: string): string | null {
  return BADGE_GROUP_BY_EXT[getFileExtension(filePath)] ?? null
}

// Why cache: each consumer does `const Icon = getFileTypeIcon(path)` then
// `<Icon …/>`. Returning a fresh component every call would give React a new
// element type on every render and remount the icon. One stable component per
// group keeps identity steady across renders.
const badgeComponentCache: Record<string, FileIconComponent> = {}

function badgeComponentForGroup(group: string): FileIconComponent {
  const cached = badgeComponentCache[group]
  if (cached) {
    return cached
  }
  const label = BADGE_LABEL_BY_GROUP[group]
  const Component: FileIconComponent = ({ className }) => (
    <span aria-hidden="true" data-ft={group} className={cn('file-type-badge', className)}>
      {label}
    </span>
  )
  Component.displayName = `FileTypeBadge(${group})`
  badgeComponentCache[group] = Component
  return Component
}

// Why cache (same reason as badges): one stable wrapper per (icon, category)
// keeps React element identity steady so tinted fallback icons don't remount.
const categoryIconCache = new Map<string, FileIconComponent>()

function categoryTintedIcon(Icon: LucideIcon, category: LucideFileIconCategory): FileIconComponent {
  const key = `${category}:${Icon.displayName ?? Icon.name ?? 'icon'}`
  const cached = categoryIconCache.get(key)
  if (cached) {
    return cached
  }
  // `data-fcategory` keys the per-theme colour in main.css. Like the badge
  // rules, that CSS is unlayered so it wins over any `text-*` in `className`.
  const Component: FileIconComponent = ({ className }) => (
    <Icon data-fcategory={category} className={cn('file-type-icon', className)} />
  )
  Component.displayName = `FileTypeIcon(${category})`
  categoryIconCache.set(key, Component)
  return Component
}

/**
 * Icon component for a file path: a colored extension badge for recognised
 * source/config types, otherwise the Lucide glyph — tinted by broad category
 * (media/data/archive/secrets) for a little more colour, calm for prose/code.
 * The returned component accepts `className` for sizing/spacing (badge and
 * category colors come from CSS, so any `text-*` in `className` is overridden
 * for those types).
 */
export function getFileTypeIcon(filePath: string): FileIconComponent {
  const group = fileTypeBadgeGroup(filePath)
  if (group) {
    return badgeComponentForGroup(group)
  }
  const Icon = getLucideFileIcon(filePath)
  const category = getLucideFileIconCategory(Icon)
  return category ? categoryTintedIcon(Icon, category) : Icon
}
