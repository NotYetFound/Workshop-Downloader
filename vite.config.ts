import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  build: { target: ['es2022', 'chrome110', 'safari16'] },
  server: { watch: { ignored: ['**/src-tauri/**'] } },
});
