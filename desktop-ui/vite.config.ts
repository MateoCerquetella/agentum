import { resolve } from 'path'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
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
})
