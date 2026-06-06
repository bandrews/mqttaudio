// Client-side validation mirroring the daemon (API-CONTRACT §2). The parser does
// NOT clamp, so the console VALIDATES and WARNS (errors block obvious mistakes,
// warnings inform) — it never silently fixes a value.

export interface FieldIssue {
  error?: string;
  warning?: string;
}

export function validateVolume(v: number): FieldIssue {
  if (!Number.isFinite(v)) return { error: 'volume must be a number' };
  if (v < 0 || v > 1) return { warning: `volume ${v} is outside [0,1]; the daemon will clamp it` };
  return {};
}

export function validateSpeed(v: number, pitch: boolean): FieldIssue {
  if (!Number.isFinite(v)) return { error: 'speed must be a number' };
  const [lo, hi] = pitch ? [0.05, 8] : [-100, 100];
  if (pitch && v < 0) return { error: 'reverse (negative speed) is not supported with pitch correction' };
  if (v < lo || v > hi) return { warning: `speed ${v} is outside [${lo},${hi}]; the daemon will clamp it` };
  return {};
}

export function validateInternalId(s: string): FieldIssue {
  if (s && !/^\d+$/.test(s)) {
    return { warning: 'internal_id should be digits (it is matched as a number); a non-numeric value never matches' };
  }
  return {};
}

export function isBlank(s: string | undefined): boolean {
  return !s || s.trim() === '';
}
