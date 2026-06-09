// Subscribe to the /ws/state typed state channel (Sprint W7) when telemetry is on,
// exposing the latest tick (output meters + per-sample positions). When telemetry
// is off (or there is no client) it subscribes to nothing and the UI falls back to
// polling. A generation guard ignores stale (StrictMode re-run) callbacks.

import { useEffect, useRef, useState } from 'react';
import type { DaemonClient } from '../../api/client';
import type { Subscription } from '../../api/connection';
import type { TickFrame } from '../../api/contract';

export interface StateChannelState {
  connected: boolean;
  /** Latest per-output-channel peak (linear amplitude). */
  output: number[];
  /** internal_id -> live position_ms from the latest tick. */
  positions: Record<string, number>;
}

const EMPTY: StateChannelState = { connected: false, output: [], positions: {} };

export function useStateChannel(client: DaemonClient | null, enabled: boolean): StateChannelState {
  const [state, setState] = useState<StateChannelState>(EMPTY);
  const subRef = useRef<Subscription | null>(null);
  const genRef = useRef(0);

  useEffect(() => {
    if (!client || !enabled) {
      setState(EMPTY);
      return;
    }
    const myGen = ++genRef.current;
    const stale = () => genRef.current !== myGen;

    subRef.current = client.subscribeState({
      onOpen: () => {
        if (!stale()) setState((s) => ({ ...s, connected: true }));
      },
      onMessage: (data) => {
        if (stale()) return;
        const frame = data as TickFrame;
        if (frame?.type === 'tick') {
          const positions: Record<string, number> = {};
          for (const s of frame.samples ?? []) positions[s.internal_id] = s.position_ms;
          setState({ connected: true, output: frame.meters?.output ?? [], positions });
        }
      },
      onClose: () => {
        if (!stale()) setState((s) => ({ ...s, connected: false }));
      },
    });

    return () => {
      // Intentionally bump the generation ref so stale subscription callbacks no-op.
      // eslint-disable-next-line react-hooks/exhaustive-deps
      genRef.current++;
      subRef.current?.close();
      subRef.current = null;
      setState(EMPTY);
    };
  }, [client, enabled]);

  return state;
}
