// The active DaemonClient, shared via context so feature hooks (queries.ts) and
// components can reach the connected daemon without prop-drilling. Null until a
// connection is established (Sprint W0 Connect surface).

import { createContext, useContext } from 'react';
import type { DaemonClient } from '../api/client';

export const ClientContext = createContext<DaemonClient | null>(null);

export function useClient(): DaemonClient | null {
  return useContext(ClientContext);
}
