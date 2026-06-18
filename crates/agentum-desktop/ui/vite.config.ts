import { resolve } from 'path'
import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    // Force a single React/React-DOM copy. A transitive dep can pull a second
    // react-dom (e.g. 19.2.3 vs the top-level 19.2.7); two copies in one bundle
    // make React's hook dispatcher null → "null is not an object (B.H.useEffect)"
    // crash at the app root. Deduping resolves every import to the top-level copy.
    dedupe: ['react', 'react-dom', 'react/jsx-runtime'],
    alias: {
      '@renderer': resolve('src'),
      '@': resolve('src'),
      // Map Electron-era relative imports to their new locations
      '../../shared': resolve('src/shared'),
      '../../../shared': resolve('src/shared'),
      '../../../../shared': resolve('src/shared'),
      // Resources
      '../../resources': resolve('resources'),
      '../../../resources': resolve('resources'),
    },
  },
  worker: {
    format: 'es',
  },
  // Tauri expects a fixed port in dev mode
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    outDir: 'dist',
    target: 'esnext',
    minify: 'esbuild',
    // Raised above Rollup's 500 KB default: this bundle loads inside a Tauri
    // webview from local disk, so a few large lazy chunks are expected. The
    // eager entry chunk is guarded separately by scripts/check-entry-size.mjs.
    chunkSizeWarningLimit: 2500,
    rollupOptions: {
      output: {
        // Pin the heavy, lazy-only libraries into named vendor chunks so they
        // can't silently merge back into the eager entry chunk — the regression
        // class fixed in #21 (react-markdown and xterm had leaked in via
        // always-mounted surfaces). These libs are only reached through lazy()
        // boundaries today, so naming their chunks keeps them lazy and stable.
        //
        // React/react-dom are deliberately NOT chunked here: the single deduped
        // copy (see `dedupe` above) must stay in the entry. Splitting it has
        // historically tripped React's hook dispatcher to null at the app root.
        manualChunks(id) {
          if (!id.includes('node_modules')) return undefined
          if (
            id.includes('react-markdown') ||
            id.includes('/micromark') ||
            id.includes('/mdast') ||
            id.includes('/hast') ||
            id.includes('/remark') ||
            id.includes('/rehype') ||
            id.includes('/unified') ||
            id.includes('/unist') ||
            id.includes('/vfile') ||
            id.includes('/property-information')
          ) {
            return 'markdown-vendor'
          }
          if (id.includes('monaco-editor') || id.includes('@monaco-editor')) {
            return 'monaco-vendor'
          }
          if (id.includes('@tiptap') || id.includes('/prosemirror')) {
            return 'tiptap-vendor'
          }
          if (id.includes('/mermaid') || id.includes('/cytoscape')) {
            return 'mermaid-vendor'
          }
          return undefined
        },
      },
    },
  },
  envPrefix: ['VITE_', 'TAURI_'],
  test: {
    // Forwards the '@/tauri' `api` module to the legacy `window.api` stub so the
    // existing window.api-based tests keep working after the call-site migration.
    setupFiles: ['./src/test-setup.ts'],
  },
})
