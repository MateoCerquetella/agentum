import {
  Database,
  File,
  FileArchive,
  FileBox,
  FileChartColumn,
  FileCog,
  FileDiff,
  FileImage,
  FileJson,
  FileKey,
  FileLock,
  FileMusic,
  FileSliders,
  FileSpreadsheet,
  FileText,
  FileType,
  FileVideo,
  type LucideIcon
} from 'lucide-react'
import { describe, expect, it } from 'vitest'
import { fileTypeBadgeGroup, getFileTypeIcon } from './file-type-icons'
import { getLucideFileIcon, getLucideFileIconCategory } from './lucide-file-icons'

describe('fileTypeBadgeGroup', () => {
  it('badges recognised source/config extensions by their color group', () => {
    expect(fileTypeBadgeGroup('src/worktree.rs')).toBe('rs')
    expect(fileTypeBadgeGroup('src/App.tsx')).toBe('ts')
    expect(fileTypeBadgeGroup('src/main.mts')).toBe('ts')
    expect(fileTypeBadgeGroup('src/index.js')).toBe('js')
    expect(fileTypeBadgeGroup('ui/Pane.svelte')).toBe('svelte')
    expect(fileTypeBadgeGroup('styles/app.css')).toBe('css')
    expect(fileTypeBadgeGroup('styles/app.scss')).toBe('scss')
    expect(fileTypeBadgeGroup('flake.nix')).toBe('nix')
    expect(fileTypeBadgeGroup('scripts/run.bash')).toBe('sh')
  })

  it('badges JVM/native/scripting languages, with headers folded onto C/C++', () => {
    expect(fileTypeBadgeGroup('Main.java')).toBe('java')
    expect(fileTypeBadgeGroup('src/main.c')).toBe('c')
    expect(fileTypeBadgeGroup('src/util.h')).toBe('c')
    expect(fileTypeBadgeGroup('src/engine.cpp')).toBe('cpp')
    expect(fileTypeBadgeGroup('src/engine.hpp')).toBe('cpp')
    expect(fileTypeBadgeGroup('App.swift')).toBe('swift')
    expect(fileTypeBadgeGroup('Main.kt')).toBe('kt')
    expect(fileTypeBadgeGroup('build.gradle.kts')).toBe('kt')
    expect(fileTypeBadgeGroup('index.php')).toBe('php')
  })

  it('badges by extension even for files with a dedicated named icon (matches the explorer look)', () => {
    // README.md and Cargo.toml have named Lucide icons, but the tree has always
    // badged them by extension — keep that.
    expect(fileTypeBadgeGroup('README.md')).toBe('md')
    expect(fileTypeBadgeGroup('Cargo.toml')).toBe('toml')
    expect(fileTypeBadgeGroup('package.json')).toBe('json')
  })

  it('returns null for extensions without a badge', () => {
    expect(fileTypeBadgeGroup('db/schema.sql')).toBeNull()
    expect(fileTypeBadgeGroup('config/settings.jsonc')).toBeNull()
    expect(fileTypeBadgeGroup('README')).toBeNull()
    expect(fileTypeBadgeGroup('assets/logo.png')).toBeNull()
    expect(fileTypeBadgeGroup('unknown.customtype')).toBeNull()
  })
})

describe('getFileTypeIcon', () => {
  it('returns a stable badge component for badged extensions', () => {
    const a = getFileTypeIcon('a.rs')
    const b = getFileTypeIcon('b.rs')
    expect(a).toBe(b)
    expect((a as { displayName?: string }).displayName).toBe('FileTypeBadge(rs)')
    expect(a).not.toBe(FileText)
  })

  it('falls back to calm (untinted) Lucide icons for prose/code/config files', () => {
    // Categories that stay monochrome are returned as the raw Lucide glyph.
    expect(getFileTypeIcon('/repo/.editorconfig')).toBe(FileSliders)
    expect(getFileTypeIcon('README')).toBe(FileText)
    expect(getFileTypeIcon('Dockerfile.dev')).toBe(FileCog)
    expect(getFileTypeIcon('notes.patch')).toBe(FileDiff)
  })

  it('tints media, data, archive, and secret fallback icons by category', () => {
    // Resolution (which glyph) is unchanged, but getFileTypeIcon wraps the
    // tinted categories so main.css can colour them — identity therefore differs
    // from the raw Lucide icon. Verify both the resolved glyph and the tint.
    const cases: { path: string; icon: LucideIcon; category: string }[] = [
      { path: 'assets/logo.png', icon: FileImage, category: 'media' },
      { path: 'sound/theme.mp3', icon: FileMusic, category: 'media' },
      { path: 'demo.mov', icon: FileVideo, category: 'media' },
      { path: 'release.tar.gz', icon: FileArchive, category: 'archive' },
      { path: 'db/schema.sql', icon: Database, category: 'data' },
      { path: 'reports/summary.xlsx', icon: FileSpreadsheet, category: 'data' },
      { path: 'slides/status.pptx', icon: FileChartColumn, category: 'data' },
      { path: 'config/settings.jsonc', icon: FileJson, category: 'data' },
      { path: 'certs/server.pem', icon: FileKey, category: 'secure' },
      { path: 'C:\\repo\\.env.local', icon: FileLock, category: 'secure' }
    ]
    for (const { path, icon, category } of cases) {
      expect(getLucideFileIcon(path)).toBe(icon)
      expect(getLucideFileIconCategory(icon)).toBe(category)
      const tinted = getFileTypeIcon(path)
      expect(tinted).not.toBe(icon)
      expect((tinted as { displayName?: string }).displayName).toBe(`FileTypeIcon(${category})`)
    }
  })

  it('falls back to the generic file icon for unknown files', () => {
    expect(getFileTypeIcon('unknown.customtype')).toBe(File)
  })

  it('keeps a less / css-family extension on the Lucide type glyph', () => {
    expect(getFileTypeIcon('styles/app.less')).toBe(FileType)
  })
})
