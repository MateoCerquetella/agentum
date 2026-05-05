import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

const config = {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter({
      pages: 'build',
      assets: 'build',
      fallback: 'index.html',
      precompress: false,
      strict: false
    }),
    alias: {
      $components: 'src/lib/components',
      $stores: 'src/lib/stores',
      $themes: 'src/lib/themes'
    }
  }
};

export default config;
