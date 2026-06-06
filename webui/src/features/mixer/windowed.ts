// Infer whether a sample is windowed/streamed (forward-only). /status/samples has
// no is-windowed flag yet, so we infer from total_frames === 0 — streamed plays
// construct their status with total_frames: 0 (see docs/bugs.md, Sprint W5).
// Sprint W6 F4 adds a real `windowed` field; this helper is the seam to switch to.

import type { SampleInfo } from '../../api/contract';

export function isWindowed(sample: Pick<SampleInfo, 'total_frames'>): boolean {
  return sample.total_frames === 0;
}
