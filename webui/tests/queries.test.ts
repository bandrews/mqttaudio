import { describe, it, expect, vi } from 'vitest';
import {
  POLL,
  versionQueryOptions,
  healthQueryOptions,
  statusQueryOptions,
  metricsQueryOptions,
  samplesQueryOptions,
  voicesQueryOptions,
  inputsQueryOptions,
  cacheQueryOptions,
} from '../src/state/queries';
import type { DaemonClient } from '../src/api/client';

function mockClient() {
  return {
    version: vi.fn().mockResolvedValue({ name: 'mqttaudio', version: '2.0.0' }),
    health: vi.fn().mockResolvedValue({ status: 'ok' }),
    status: vi.fn().mockResolvedValue({}),
    metrics: vi.fn().mockResolvedValue({}),
    statusSamples: vi.fn().mockResolvedValue({ samples: [] }),
    statusVoices: vi.fn().mockResolvedValue({ voices: [] }),
    statusInputs: vi.fn().mockResolvedValue({ inputs: [] }),
    statusCache: vi.fn().mockResolvedValue({}),
  } as unknown as DaemonClient & Record<string, ReturnType<typeof vi.fn>>;
}

describe('query cadences (DW6)', () => {
  it('/version is fetched once (staleTime Infinity, no interval)', () => {
    const o = versionQueryOptions(mockClient());
    expect(o.staleTime).toBe(Infinity);
    expect(o.refetchInterval).toBe(false);
  });

  it('/metrics and /status* poll in the 1-2s band', () => {
    expect(POLL.status).toBeGreaterThanOrEqual(1000);
    expect(POLL.status).toBeLessThanOrEqual(2000);
    const client = mockClient();
    for (const f of [statusQueryOptions, metricsQueryOptions, samplesQueryOptions, voicesQueryOptions, inputsQueryOptions]) {
      expect(f(client).refetchInterval).toBe(POLL.status);
    }
  });

  it('/status/cache polls in the 2-5s band', () => {
    expect(POLL.cache).toBeGreaterThanOrEqual(2000);
    expect(POLL.cache).toBeLessThanOrEqual(5000);
    expect(cacheQueryOptions(mockClient()).refetchInterval).toBe(POLL.cache);
  });

  it('/health uses a slow connection-loss cadence', () => {
    expect(healthQueryOptions(mockClient()).refetchInterval).toBe(POLL.health);
    expect(POLL.health).toBeGreaterThanOrEqual(5000);
  });

  it('each query calls its matching client read method', async () => {
    const client = mockClient();
    await (statusQueryOptions(client).queryFn as () => unknown)();
    await (cacheQueryOptions(client).queryFn as () => unknown)();
    expect((client as unknown as Record<string, ReturnType<typeof vi.fn>>).status).toHaveBeenCalledOnce();
    expect((client as unknown as Record<string, ReturnType<typeof vi.fn>>).statusCache).toHaveBeenCalledOnce();
  });
});
