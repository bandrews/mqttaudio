// ABOUTME: Client-side validation helpers for console inputs: volume, speed, internal_id, blanks.
// ABOUTME: Each check returns an optional error and warning message for the form to display.

// Client-side validation mirroring the daemon (API-CONTRACT §2). The daemon clamps
// some out-of-range values and rejects others with a 400, so the console VALIDATES
// and WARNS (errors block obvious mistakes, warnings inform) — it never silently
// fixes a value.

export interface FieldIssue {
  error?: string;
  warning?: string;
}

export function validateVolume(v: number): FieldIssue {
  if (!Number.isFinite(v)) return { error: 'volume must be a number' };
  if (v < 0 || v > 4) return { warning: `volume ${v} is outside [0,4]; the daemon will clamp it` };
  return {};
}

export function validateSpeed(v: number, pitch: boolean): FieldIssue {
  if (!Number.isFinite(v)) return { error: 'speed must be a number' };
  const [lo, hi] = pitch ? [0.05, 8] : [-100, 100];
  if (v === 0) return { error: 'speed 0 is refused by the daemon; use stop to end playback' };
  if (pitch && v < 0) return { error: 'reverse (negative speed) is not supported with pitch correction' };
  if (v < lo || v > hi) return { warning: `speed ${v} is outside [${lo},${hi}]; the daemon will clamp it` };
  return {};
}

export function validateInternalId(s: string): FieldIssue {
  if (s && !/^\d+$/.test(s)) {
    return { error: 'internal_id must be digits; the daemon refuses a non-numeric value (400)' };
  }
  return {};
}

export function isBlank(s: string | undefined): boolean {
  return !s || s.trim() === '';
}
