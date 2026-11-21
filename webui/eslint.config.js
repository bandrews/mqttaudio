import js from '@eslint/js';
import globals from 'globals';
import reactHooks from 'eslint-plugin-react-hooks';
import reactRefresh from 'eslint-plugin-react-refresh';
import tseslint from 'typescript-eslint';

// Flat ESLint config. Warnings are errors in CI (`eslint . --max-warnings=0`).
// The DW2 Electron seam is enforced here: no module outside the connection layer
// may touch `fetch`/`WebSocket` directly. The connection implementations are
// exempted below.
export default tseslint.config(
  { ignores: ['dist', 'coverage', 'playwright-report', 'test-results'] },
  {
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    files: ['**/*.{ts,tsx}'],
    languageOptions: {
      ecmaVersion: 2021,
      globals: { ...globals.browser, ...globals.node },
    },
    plugins: {
      'react-hooks': reactHooks,
      'react-refresh': reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      'react-refresh/only-export-components': ['warn', { allowConstantExport: true }],
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_' }],
    },
  },
  // DW2: confine fetch/WebSocket to the connection layer so an Electron build is a
  // connection swap, not a UI rewrite. Everything under src/ except the connection
  // implementations is forbidden from touching them directly.
  {
    files: ['src/**/*.{ts,tsx}'],
    ignores: ['src/api/connection.browser.ts', 'src/api/connection.electron.ts'],
    rules: {
      'no-restricted-globals': [
        'error',
        { name: 'fetch', message: 'Use DaemonConnection (src/api/connection.ts), not fetch directly (DW2 Electron seam).' },
        { name: 'WebSocket', message: 'Use DaemonConnection.subscribe (src/api/connection.ts), not WebSocket directly (DW2 Electron seam).' },
      ],
    },
  },
  // Test + config files may use Node globals freely.
  {
    files: ['tests/**/*.{ts,tsx}', '*.config.{ts,js}', 'vitest.setup.ts'],
    languageOptions: { globals: { ...globals.node } },
  },
);
