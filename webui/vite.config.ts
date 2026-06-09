/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Vite + Vitest config. The dev-server proxy mirrors the production reverse-proxy
// sidecar (DW1): /api/* is forwarded to the daemon with the prefix stripped, and
// /ws is upgraded — so the SPA always uses same-origin relative URLs in both dev
// and prod. Point it at a daemon with VITE_DAEMON_TARGET (default localhost:8080).
const DAEMON_TARGET = process.env.VITE_DAEMON_TARGET ?? 'http://localhost:8080';

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': {
        target: DAEMON_TARGET,
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/api/, ''),
      },
      '/ws': {
        target: DAEMON_TARGET,
        changeOrigin: true,
        ws: true,
      },
    },
  },
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./vitest.setup.ts'],
    include: ['tests/**/*.{test,spec}.{ts,tsx}'],
    exclude: ['e2e/**', 'node_modules/**'],
    css: false,
  },
});
