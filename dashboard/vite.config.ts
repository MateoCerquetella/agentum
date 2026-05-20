import { sveltekit } from '@sveltejs/kit/vite';
// `defineConfig` from `vitest/config` accepts both the Vite server config
// and the Vitest `test` block so svelte-check's tsconfig validates the
// `test` key without a triple-slash reference dance.
import { defineConfig } from 'vitest/config';

const API_TARGET = process.env.AGENTUM_BACKEND ?? 'http://127.0.0.1:8822';

export default defineConfig({
  plugins: [sveltekit()],
  server: {
    port: 5173,
    strictPort: false,
    proxy: {
      '/api': {
        target: API_TARGET,
        changeOrigin: true,
        ws: true
      }
    }
  },
  test: {
    // Pure data tests only — no DOM (intentionally no jsdom/happy-dom).
    // Add a setup later if Svelte component tests ever appear.
    include: ['src/**/*.{test,spec}.ts']
  }
});
