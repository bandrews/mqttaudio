// Client-side rate/delta computation (DW6 label hygiene): the daemon's clip and
// xrun counters are cumulative, so the UI must diff successive polls rather than
// present a counter as an instantaneous rate. `useDelta` tracks the previous
// value and returns the change since the last poll (clamped >= 0 so a counter
// reset reads as 0, not a negative spike).

import { useRef } from 'react';

export interface Delta {
  value: number;
  delta: number;
}

/** The previous value of `delta` computation, clamped non-negative on reset. */
export function diffCounter(prev: number | undefined, curr: number): number {
  if (prev === undefined) return 0;
  return Math.max(0, curr - prev);
}

export function useDelta(value: number): Delta {
  const prevRef = useRef<number | undefined>(undefined);
  const delta = diffCounter(prevRef.current, value);
  prevRef.current = value;
  return { value, delta };
}
