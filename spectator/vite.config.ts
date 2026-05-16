/// <reference types="vitest" />
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Vite emits the spectator bundle to `dist/`. Filenames are pinned (no content hash) so
// the Rust server's `include_str!` paths in `server/src/net.rs` resolve at compile time
// against a stable layout: `dist/index.html`, `dist/index.js`, `dist/index.css`.
// Cache-busting is not needed here — the server sends `Cache-Control: no-store`.
export default defineConfig({
  plugins: [svelte()],
  server: {
    port: 5173,
    // Forward WS endpoints to the Rust server during `vite dev`. The browser opens
    // `ws://localhost:5173/spectate`; Vite proxies it to localhost:7878.
    proxy: {
      '/spectate': { target: 'ws://localhost:7878', ws: true },
      '/bot': { target: 'ws://localhost:7878', ws: true },
      '/admin': { target: 'ws://localhost:7878', ws: true },
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    assetsDir: '.',
    target: 'es2020',
    rollupOptions: {
      output: {
        entryFileNames: 'index.js',
        chunkFileNames: '[name].js',
        assetFileNames: (assetInfo) => {
          if (assetInfo.name?.endsWith('.css')) return 'index.css';
          return '[name][extname]';
        },
      },
    },
  },
  test: {
    environment: 'jsdom',
    globals: true,
    include: ['src/tests/**/*.test.ts'],
  },
});
