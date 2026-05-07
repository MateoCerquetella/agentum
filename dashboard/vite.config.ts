import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

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
  }
});
