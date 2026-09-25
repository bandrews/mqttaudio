// ABOUTME: React hook that runs one command through the connected client and tracks its state.
// ABOUTME: Exposes idle/sending/ok/error with the daemon's message or the thrown error text.

// Runs a command through the active client and tracks the result. Note: command
// endpoints answer once the daemon has carried the command out (API-CONTRACT §1):
// a 200 means it completed, and an error status rejects with an HttpError.

import { useState } from 'react';
import type { CommandResponse } from '../../api/contract';
import type { DaemonClient } from '../../api/client';
import { useClient } from '../../state/clientContext';

export interface RunState {
  status: 'idle' | 'sending' | 'ok' | 'error';
  message?: string;
}

export function useCommandRunner() {
  const client = useClient();
  const [state, setState] = useState<RunState>({ status: 'idle' });

  async function run(fn: (client: DaemonClient) => Promise<CommandResponse>) {
    if (!client) {
      setState({ status: 'error', message: 'Not connected.' });
      return;
    }
    setState({ status: 'sending' });
    try {
      const res = await fn(client);
      setState({
        status: res.success ? 'ok' : 'error',
        message: res.error ?? res.message ?? (res.success ? 'Command completed.' : 'Failed.'),
      });
    } catch (err) {
      setState({ status: 'error', message: err instanceof Error ? err.message : String(err) });
    }
  }

  return { state, run };
}
