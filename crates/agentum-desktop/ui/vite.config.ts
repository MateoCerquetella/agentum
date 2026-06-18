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
  },
  envPrefix: ['VITE_', 'TAURI_'],
  test: {
    // Forwards the '@/tauri' `api` module to the legacy `window.api` stub so the
    // existing window.api-based tests keep working after the call-site migration.
    setupFiles: ['./src/test-setup.ts'],
  },
})
