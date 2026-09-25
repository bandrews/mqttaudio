// ABOUTME: Tests the command alias map against the command names the daemon accepts.
// ABOUTME: Each canonical command lists every alternate name the daemon parses as it.

import { describe, it, expect } from 'vitest';
import { COMMAND_ALIASES } from '../src/api/contract';

describe('COMMAND_ALIASES (API-CONTRACT §1)', () => {
  it('maps each canonical command to every alternate name the daemon accepts', () => {
    expect(COMMAND_ALIASES).toEqual({
      play: ['soundPlay'],
      stopall: ['soundStopAll'],
      precache: ['soundPrecache'],
      fadeall: ['soundFadeAll', 'fadeout', 'soundFadeOut'],
    });
  });
});
