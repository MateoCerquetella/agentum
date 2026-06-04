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
  FileVideo
} from 'lucide-react'
import { describe, expect, it } from 'vitest'
import { fileTypeBadgeGroup, getFileTypeIcon } from './file-type-icons'

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

  it('falls back to dedicated Lucide icons for unbadged named files and extensions', () => {
    expect(getFileTypeIcon('/repo/.editorconfig')).toBe(FileSliders)
    expect(getFileTypeIcon('C:\\repo\\.env.local')).toBe(FileLock)
    expect(getFileTypeIcon('README')).toBe(FileText)
    expect(getFileTypeIcon('Dockerfile.dev')).toBe(FileCog)
    expect(getFileTypeIcon('config/settings.jsonc')).toBe(FileJson)
    expect(getFileTypeIcon('assets/logo.png')).toBe(FileImage)
    expect(getFileTypeIcon('notes.patch')).toBe(FileDiff)
  })

  it('uses more specific icons for data, security, and presentation files', () => {
    expect(getFileTypeIcon('db/schema.sql')).toBe(Database)
    expect(getFileTypeIcon('reports/summary.xlsx')).toBe(FileSpreadsheet)
    expect(getFileTypeIcon('certs/server.pem')).toBe(FileKey)
    expect(getFileTypeIcon('slides/status.pptx')).toBe(FileChartColumn)
  })

  it('handles compound archive extensions before their trailing extension', () => {
    expect(getFileTypeIcon('release.tar.gz')).toBe(FileArchive)
  })

  it('matches audio and video extensions', () => {
    expect(getFileTypeIcon('sound/theme.mp3')).toBe(FileMusic)
    expect(getFileTypeIcon('demo.mov')).toBe(FileVideo)
  })

  it('falls back to the generic file icon for unknown files', () => {
    expect(getFileTypeIcon('unknown.customtype')).toBe(File)
  })

  it('keeps a less / css-family extension on the Lucide type glyph', () => {
    expect(getFileTypeIcon('styles/app.less')).toBe(FileType)
  })
})
