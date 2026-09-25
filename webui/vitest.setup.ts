// ABOUTME: Vitest setup: registers the jest-dom matchers.
// ABOUTME: Unmounts rendered React trees after each test.

import '@testing-library/jest-dom/vitest';
import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/react';

afterEach(() => {
  cleanup();
});
