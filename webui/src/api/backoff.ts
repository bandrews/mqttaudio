// Exponential backoff with jitter for reconnect scheduling (Sprint W1, F4). Pure
// and deterministic: the jitter source is injectable so tests can assert the
// schedule. Default base 500ms, doubling, capped at 15s, with up to 30% jitter.

export interface BackoffOptions {
  baseMs?: number;
  capMs?: number;
  /** Fractional jitter [0..1] applied on top of the delay. */
  jitter?: number;
  /** Injectable RNG in [0,1) for deterministic tests. */
  random?: () => number;
}

/** Delay (ms) before reconnect attempt `attempt` (0-based). */
export function nextBackoff(attempt: number, options: BackoffOptions = {}): number {
  const base = options.baseMs ?? 500;
  const cap = options.capMs ?? 15_000;
  const jitter = options.jitter ?? 0.3;
  const random = options.random ?? Math.random;
  const exp = Math.min(cap, base * 2 ** Math.max(0, attempt));
  const jitterMs = exp * jitter * random();
  return Math.min(cap, Math.round(exp + jitterMs));
}
