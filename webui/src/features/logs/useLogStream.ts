// Drives the /ws log-stream subscription with a real connection lifecycle
// (Sprint W1, F3/F4): connect -> on the {type:"connected"} welcome go live; on
// each {type:"log"} append; on close, classify via a /version re-probe (401 ->
// unauthorized and stop; otherwise reconnect with exponential backoff) and mark
// the gap so dropped messages during a disconnect are visible. /ws is logs-only —
// these lines are never treated as state/playback events.

import { useEffect, useRef, useState } from 'react';
import type { ConnectionState, DaemonClient } from '../../api/client';
import type { Subscription } from '../../api/connection';
import { nextBackoff, type BackoffOptions } from '../../api/backoff';

export type LogLineKind = 'log' | 'system' | 'gap';

export interface LogLine {
  id: number;
  text: string;
  kind: LogLineKind;
}

export interface LogStreamState {
  state: ConnectionState;
  version?: string;
  lines: LogLine[];
}

export interface UseLogStreamOptions {
  maxLines?: number;
  backoff?: BackoffOptions;
  path?: string;
}

interface WelcomeFrame {
  type: 'connected';
  message?: string;
  version?: string;
}
interface LogFrame {
  type: 'log';
  message: string;
}
type Frame = WelcomeFrame | LogFrame | { type: string; [k: string]: unknown };

export function useLogStream(
  client: DaemonClient | null,
  options: UseLogStreamOptions = {},
): LogStreamState {
  const maxLines = options.maxLines ?? 500;
  const path = options.path ?? '/ws';
  const backoffOpts = options.backoff;

  const [state, setState] = useState<ConnectionState>(client ? 'connecting' : 'offline');
  const [version, setVersion] = useState<string | undefined>(undefined);
  const [lines, setLines] = useState<LogLine[]>([]);

  // Mutable bits that must not re-trigger the effect.
  const subRef = useRef<Subscription | null>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const attemptRef = useRef(0);
  // Generation token: bumped on every (re)connect and on teardown, so callbacks
  // from a superseded subscription (e.g. a StrictMode effect re-run) are ignored.
  const genRef = useRef(0);
  // True only between a close-triggered reconnect and the next welcome, so the
  // gap marker fires for a real disconnect — not for a fresh subscribe (e.g. a
  // StrictMode effect re-run).
  const pendingGapRef = useRef(false);
  const idRef = useRef(0);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    if (!client) {
      setState('offline');
      return;
    }

    const append = (text: string, kind: LogLineKind) => {
      if (!mountedRef.current) return;
      const id = idRef.current++;
      setLines((prev) => {
        const next = [...prev, { id, text, kind }];
        return next.length > maxLines ? next.slice(next.length - maxLines) : next;
      });
    };

    const clearTimer = () => {
      if (timerRef.current) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };

    const connect = () => {
      if (!mountedRef.current) return;
      const myGen = ++genRef.current;
      const isStale = () => genRef.current !== myGen;
      setState('connecting');
      subRef.current = client.subscribe(path, {
        onMessage: (data) => {
          if (isStale()) return;
          const frame = data as Frame;
          if (frame?.type === 'connected') {
            if (pendingGapRef.current) {
              append('— stream reconnected; messages during the gap were dropped —', 'gap');
              pendingGapRef.current = false;
            }
            attemptRef.current = 0;
            setVersion((frame as WelcomeFrame).version);
            setState('live');
          } else if (frame?.type === 'log') {
            append((frame as LogFrame).message, 'log');
          }
        },
        onClose: () => {
          if (isStale()) return;
          scheduleReconnect();
        },
        onError: () => {
          if (isStale()) return;
          scheduleReconnect();
        },
      });
    };

    const scheduleReconnect = async () => {
      if (!mountedRef.current || timerRef.current) return;
      subRef.current = null;
      // Classify: a 401 is not a transient blip — stop and surface it.
      const classification = await client.probeConnection();
      if (!mountedRef.current) return;
      if (classification === 'unauthorized') {
        setState('unauthorized');
        return;
      }
      setState('offline');
      // Mark that the next successful welcome follows a real disconnect.
      pendingGapRef.current = true;
      const delay = nextBackoff(attemptRef.current, backoffOpts);
      attemptRef.current += 1;
      timerRef.current = setTimeout(() => {
        timerRef.current = null;
        connect();
      }, delay);
    };

    connect();

    return () => {
      mountedRef.current = false;
      // Intentionally bump the generation ref so stale subscription callbacks no-op.
      // eslint-disable-next-line react-hooks/exhaustive-deps
      genRef.current++;
      clearTimer();
      subRef.current?.close();
      subRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, maxLines, path]);

  return { state, version, lines };
}
