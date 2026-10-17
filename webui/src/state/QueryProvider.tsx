// Server-state plumbing (DW4): a TanStack Query client plus the active-client
// context provider. Feature hooks (queries.ts) read both.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import type { DaemonClient } from '../api/client';
import { ClientContext } from './clientContext';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { retry: false, refetchOnWindowFocus: false },
  },
});

export function QueryProvider({ children }: { children: ReactNode }) {
  return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
}

export function ClientProvider({
  client,
  children,
}: {
  client: DaemonClient | null;
  children: ReactNode;
}) {
  return <ClientContext.Provider value={client}>{children}</ClientContext.Provider>;
}
