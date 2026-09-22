import { describe, it, expect, vi } from 'vitest';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { renderWithClient, mockReadClient } from './_helpers';
import { DaemonClient } from '../src/api/client';
import type { DaemonConnection, Subscription, SubscriptionHandlers } from '../src/api/connection';
import { NowPlayingBoard } from '../src/features/dashboard/NowPlayingBoard';
import { TelemetrySwitch } from '../src/features/telemetry/TelemetrySwitch';
import { isWindowed } from '../src/features/mixer/windowed';
import type { SampleInfo } from '../src/api/contract';

function sample(overrides: Partial<SampleInfo> = {}): SampleInfo {
  return {
    internal_id: '1',
    id: null,
    voice: 'music',
    file: '/a.mp3',
    position: 0,
    position_ms: 0,
    total_frames: 480_000,
    total_ms: 10_000,
    sample_rate: 48_000,
    volume: 1,
    voice_volume: 1,
    speed: 1,
    loop_mode: false,
    progress_percent: 0,
    ...overrides,
  };
}

describe('isWindowed prefers the real flag (W6 F4)', () => {
  it('uses sample.windowed when present, else infers from total_frames', () => {
    expect(isWindowed({ total_frames: 480_000, windowed: true })).toBe(true);
    expect(isWindowed({ total_frames: 0, windowed: false })).toBe(false);
    expect(isWindowed({ total_frames: 0 })).toBe(true);
  });
});

describe('DaemonClient telemetry', () => {
  it('setTelemetry posts the flag to /telemetry', async () => {
    const posts: { path: string; body: unknown }[] = [];
    const conn: DaemonConnection = {
      connection: { id: 't', label: 't', baseUrl: '/api' },
      async get<T>() {
        return {} as T;
      },
      async post<T>(path: string, body?: unknown) {
        posts.push({ path, body });
        return { enabled: true } as T;
      },
      subscribe(_p: string, _h: SubscriptionHandlers): Subscription {
        return { close: () => {} };
      },
    };
    await new DaemonClient(conn).setTelemetry(true);
    expect(posts).toEqual([{ path: '/telemetry', body: { enabled: true } }]);
  });
});

describe('NowPlayingBoard progress (W6)', () => {
  it('shows a determinate progress bar when telemetry is on and the sample is not windowed', async () => {
    const client = mockReadClient({
      telemetry: async () => ({ enabled: true }),
      statusSamples: async () => ({ samples: [sample({ progress_percent: 25, position_ms: 2500 })] }),
    });
    renderWithClient(<NowPlayingBoard />, client);
    const bar = await screen.findByLabelText('progress 1');
    expect(bar).toHaveAttribute('aria-valuenow', '25');
    expect(screen.queryByText(/live position unavailable/i)).toBeNull();
  });

  it('shows streamed (forward-only) for a windowed sample, no progress bar', async () => {
    const client = mockReadClient({
      telemetry: async () => ({ enabled: true }),
      statusSamples: async () => ({ samples: [sample({ windowed: true, total_frames: 0, total_ms: 0 })] }),
    });
    renderWithClient(<NowPlayingBoard />, client);
    expect(await screen.findByText(/streamed \(forward-only\)/i)).toBeInTheDocument();
    expect(screen.queryByLabelText('progress 1')).toBeNull();
  });

  it('shows the unavailable note when telemetry is off', async () => {
    const client = mockReadClient({
      telemetry: async () => ({ enabled: false }),
      statusSamples: async () => ({ samples: [sample()] }),
    });
    renderWithClient(<NowPlayingBoard />, client);
    expect(await screen.findByText(/live position unavailable/i)).toBeInTheDocument();
  });
});

describe('TelemetrySwitch (DW3)', () => {
  it('toggles telemetry via the client', async () => {
    const user = userEvent.setup();
    const setTelemetry = vi.fn().mockResolvedValue({ enabled: true });
    const client = mockReadClient({
      telemetry: async () => ({ enabled: false }),
      setTelemetry,
    });
    renderWithClient(<TelemetrySwitch />, client);
    const toggle = screen.getByRole('checkbox', { name: 'telemetry' });
    await waitFor(() => expect(toggle).not.toBeChecked());
    await user.click(toggle);
    expect(setTelemetry).toHaveBeenCalledWith(true);
  });
});
