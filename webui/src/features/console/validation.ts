// Client-side validation mirroring the daemon (API-CONTRACT §2). The parser does
// NOT clamp, so the console VALIDATES and WARNS (errors block obvious mistakes,
// warnings inform) — it never silently fixes a value.

import type { SpeakerMotion, Waypoint } from '../../api/contract';

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

export function waypointFields(w: Waypoint) {
  return typeof w === 'object' ? w : { speaker: w };
}

export function motionError(motion?: SpeakerMotion): string | undefined {
  if (!motion) return;
  if (motion.handoff !== undefined && !(motion.handoff >= 0 && motion.handoff <= 1))
    return 'Enter a handoff between 0% and 100%.';
  const last = motion.path.length - 1;
  if (last < 0) return 'Add a waypoint.';
  let previousTime = 0;
  for (const [index, waypoint] of motion.path.entries()) {
    const w = waypointFields(waypoint);
    if (!String('place' in w ? w.place : w.speaker).trim()) return 'Enter a destination for each waypoint.';
    if (w.at_ms === undefined) continue;
    if (!Number.isInteger(w.at_ms) || w.at_ms > 0xffffffff || (index === 0 ? w.at_ms !== 0 : w.at_ms <= previousTime))
      return 'Use increasing whole-millisecond arrival times, starting at zero.';
    previousTime = w.at_ms;
  }
  const end = waypointFields(motion.path[last]!).at_ms;
  const duration = motion.duration_ms ?? end;
  if (last > 0 && (duration === undefined || (end === undefined ? duration <= previousTime : duration !== end)))
    return 'Set a duration after the interior waypoints, or use the final arrival time.';
  if (duration !== undefined && (!Number.isInteger(duration) || duration < 0 || duration > 0xffffffff))
    return 'Enter a duration in whole milliseconds.';
  if (motion.source_channel !== undefined && (!Number.isSafeInteger(motion.source_channel) || motion.source_channel < 0))
    return 'Enter a zero-based input channel.';
}
