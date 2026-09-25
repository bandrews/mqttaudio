// ABOUTME: isWindowed: whether a sample is a windowed (streamed, forward-only) play.
// ABOUTME: Used to hide seek/speed controls and progress bars for streamed samples.

// Decide whether a sample is windowed/streamed (forward-only). /status/samples
// reports a `windowed` flag; when it is absent, infer from total_frames === 0 —
// streamed plays construct their status with total_frames: 0.

import type { SampleInfo } from '../../api/contract';

export function isWindowed(sample: Pick<SampleInfo, 'total_frames' | 'windowed'>): boolean {
  // Prefer the real flag (Sprint W6 F4); fall back to the total_frames heuristic
  // for older daemons that don't send it.
  if (typeof sample.windowed === 'boolean') return sample.windowed;
  return sample.total_frames === 0;
}
