import { describe, it, expect } from 'vitest';
import { nextBackoff } from '../src/api/backoff';

const noJitter = { random: () => 0 };

describe('nextBackoff (F4)', () => {
  it('grows exponentially from the base', () => {
    const o = { baseMs: 500, capMs: 60_000, ...noJitter };
    expect(nextBackoff(0, o)).toBe(500);
    expect(nextBackoff(1, o)).toBe(1000);
    expect(nextBackoff(2, o)).toBe(2000);
    expect(nextBackoff(3, o)).toBe(4000);
  });

  it('caps at capMs', () => {
    expect(nextBackoff(20, { baseMs: 500, capMs: 15_000, ...noJitter })).toBe(15_000);
  });

  it('adds bounded jitter on top of the delay and never exceeds the cap', () => {
    expect(nextBackoff(1, { baseMs: 500, capMs: 60_000, jitter: 0.3, random: () => 1 })).toBe(1300);
    expect(nextBackoff(30, { baseMs: 500, capMs: 15_000, jitter: 0.3, random: () => 1 })).toBe(15_000);
  });
});
