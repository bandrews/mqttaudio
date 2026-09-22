// Selector helpers (kept separate from the component so fast-refresh stays happy).

import type { SampleSelector } from '../../api/contract';

export function selectorToParams(s: SampleSelector): SampleSelector {
  const out: SampleSelector = {};
  if (s.internal_id?.trim()) out.internal_id = s.internal_id.trim();
  if (s.id?.trim()) out.id = s.id.trim();
  if (s.file?.trim()) out.file = s.file.trim();
  if (s.voice?.trim()) out.voice = s.voice.trim();
  return out;
}

export function isSelectorEmpty(s: SampleSelector): boolean {
  return Object.keys(selectorToParams(s)).length === 0;
}
