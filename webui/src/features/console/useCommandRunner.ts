// Runs a command through the active client and tracks the result. Note: command
// endpoints return success on ENQUEUE, not on validity/effect (API-CONTRACT §1),
// so the UI says "accepted" and points the operator at the dashboard to confirm.

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
        message: res.error ?? res.message ?? (res.success ? 'Command accepted (enqueued).' : 'Failed.'),
      });
    } catch (err) {
      setState({ status: 'error', message: err instanceof Error ? err.message : String(err) });
    }
  }

  return { state, run };
}
