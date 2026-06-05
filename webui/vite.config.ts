/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Vite + Vitest config. The dev-server proxy that fronts the daemon (same-origin
// /api + /ws) is added in Sprint W1; this sprint serves the app against whatever
// base URL the connection is handed.
export default defineConfig({
  plugins: [react()],
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./vitest.setup.ts'],
    include: ['tests/**/*.{test,spec}.{ts,tsx}'],
    exclude: ['e2e/**', 'node_modules/**'],
    css: false,
  },
});
