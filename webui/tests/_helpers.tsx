// ABOUTME: Test helpers: render a component with a fresh QueryClient and a given client.
// ABOUTME: mockReadClient supplies an inert socket and a 'live' probe, plus any overrides.

import type { ReactElement } from 'react';
import { render } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { ClientContext } from '../src/state/clientContext';
import type { DaemonClient } from '../src/api/client';

/** Render a component with a fresh QueryClient and a (mock) DaemonClient. */
export function renderWithClient(ui: ReactElement, client: Partial<DaemonClient>) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={queryClient}>
      <ClientContext.Provider value={client as DaemonClient}>{ui}</ClientContext.Provider>
    </QueryClientProvider>,
  );
}

/** A mock client whose read methods resolve fixtures and whose socket is inert. */
export function mockReadClient(overrides: Partial<DaemonClient> = {}): Partial<DaemonClient> {
  return {
    subscribe: () => ({ close: () => {} }),
    probeConnection: async () => 'live',
    ...overrides,
  };
}
