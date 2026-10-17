import { describe, it, expect } from 'vitest';
import { formatBytes, formatDuration, formatUptime } from '../src/utils/format';
import { diffCounter } from '../src/state/rates';

describe('format', () => {
  it('formats bytes in base-1024 units', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1024)).toBe('1.0 KiB');
    expect(formatBytes(1_572_864)).toBe('1.5 MiB');
    expect(formatBytes(1_073_741_824)).toBe('1.0 GiB');
    expect(formatBytes(-1)).toBe('—');
  });

  it('formats durations as m:ss / h:mm:ss', () => {
    expect(formatDuration(10_000)).toBe('0:10');
    expect(formatDuration(95_000)).toBe('1:35');
    expect(formatDuration(3_661_000)).toBe('1:01:01');
  });

  it('formats uptime compactly', () => {
    expect(formatUptime(45)).toBe('45s');
    expect(formatUptime(3661)).toBe('1h 1m');
    expect(formatUptime(90_061)).toBe('1d 1h 1m');
  });
});

describe('diffCounter (DW6 rate hygiene)', () => {
  it('returns 0 on the first sample and the delta thereafter', () => {
    expect(diffCounter(undefined, 5)).toBe(0);
    expect(diffCounter(5, 8)).toBe(3);
  });
  it('clamps a counter reset to 0 (never a negative spike)', () => {
    expect(diffCounter(100, 2)).toBe(0);
  });
});
