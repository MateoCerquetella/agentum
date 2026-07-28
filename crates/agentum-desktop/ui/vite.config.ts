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
      // The beta package declares a CommonJS entry that it does not ship.
      // Resolve its published ESM entry explicitly for Vitest as well as Vite.
      '@xterm/addon-ligatures': resolve(
        __dirname,
        'node_modules/@xterm/addon-ligatures/lib/addon-ligatures.mjs'
      ),
      '@renderer': resolve(__dirname, 'src'),
      '@': resolve(__dirname, 'src'),
      '@resources': resolve(__dirname, 'resources'),
      // Map Electron-era relative imports to their new locations
      '../../shared': resolve(__dirname, 'src/shared'),
      '../../../shared': resolve(__dirname, 'src/shared'),
      '../../../../shared': resolve(__dirname, 'src/shared'),
      // Resources
      '../../resources': resolve(__dirname, 'resources'),
      '../../../resources': resolve(__dirname, 'resources'),
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
    // No hand-authored `manualChunks`: it shipped the v0.17.0 black-screen.
    // The heavy libs here (react-markdown, @tiptap/prosemirror, mermaid) all
    // depend on React, and pinning them into named vendor chunks while React
    // stayed in the eager entry created a cycle (entry → *-vendor → entry-for-
    // React). The browser evaluated a vendor chunk before the entry had
    // initialized its React binding, so React read back `undefined` and the
    // vendor's top-level `React.Activity = …` (React 19) threw — aborting the
    // whole entry graph and rendering a black app. Splitting React into its own
    // chunk only moved the cycle elsewhere (tiptap ↔ react ↔ mermaid), because
    // these libraries are genuinely interdependent and any arbitrary cut line
    // reintroduces a cross-chunk cycle.
    //
    // Rollup's automatic code-splitting, driven by the `lazy()` dynamic imports
    // in App.tsx (Terminal, CommentMarkdownImpl, MarkdownPreview, editors, …),
    // already keeps markdown/xterm/monaco off the eager entry chunk AND orders
    // module evaluation correctly. scripts/check-entry-size.mjs guards the entry
    // size as a regression backstop. Do not reintroduce manualChunks for these
    // React-dependent libraries without verifying the built app actually mounts.
  },
  envPrefix: ['VITE_', 'TAURI_'],
  test: {
    // Forwards the '@/tauri' `api` module to the legacy `window.api` stub so the
    // existing window.api-based tests keep working after the call-site migration.
    setupFiles: ['./src/test-setup.ts'],
  },
})
