import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
import pkg from './package.json' with { type: 'json' };

const API_TARGET = process.env.AGENTUM_BACKEND ?? 'http://127.0.0.1:8822';

export default defineConfig({
  plugins: [sveltekit()],
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version)
  },
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
