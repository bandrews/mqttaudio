import { describe, it, expect } from 'vitest';
import { screen } from '@testing-library/react';
import { renderWithClient, mockReadClient } from './_helpers';
import { HealthHeader } from '../src/features/dashboard/HealthHeader';
import { NowPlayingBoard } from '../src/features/dashboard/NowPlayingBoard';
import { VoicesRack } from '../src/features/dashboard/VoicesRack';
import { InputsRack } from '../src/features/dashboard/InputsRack';
import { CacheTable } from '../src/features/dashboard/CacheTable';
import type { DaemonClient } from '../src/api/client';

const status = {
  status: 'running',
  version: '2.0.0',
  active_samples: 2,
  active_inputs: 1,
  active_voices: 3,
  output_channels: 8,
  clip_count: 5,
  xruns: 1,
  cache: { memory: { entries: 3, size_bytes: 1_572_864 }, disk: { entries: 10, size_bytes: 5_242_880 } },
};
const metrics = {
  uptime_seconds: 3661,
  clips: 5,
  xruns: 1,
  active_voices: 3,
  active_samples: 2,
  active_inputs: 1,
  output_channels: 8,
  cache: {
    memory_bytes: 1_572_864,
    memory_entries: 3,
    memory_headroom_bytes: 858_993_459,
    memory_cap_bytes: 1_073_741_824,
    disk_bytes: 0,
  },
  ducking: { music: 0.1 },
};

describe('HealthHeader (F1/F2)', () => {
  it('shows counts, the clip + stream-error chips, uptime, and the cache budget', async () => {
    const client = mockReadClient({
      status: async () => status,
      metrics: async () => metrics,
    } as Partial<DaemonClient>);
    renderWithClient(<HealthHeader />, client);

    expect(await screen.findByLabelText('clips 5')).toBeInTheDocument();
    // xruns are labeled as stream errors, never "buffer xruns".
    expect(await screen.findByLabelText('stream errors 1')).toBeInTheDocument();
    expect(screen.queryByText(/xrun/i)).toBeNull();
    expect(await screen.findByLabelText('uptime')).toHaveTextContent('1h 1m');
    // cache budget gauge: 1.5 MiB used / 1.0 GiB cap
    expect(await screen.findByLabelText('cache memory budget')).toHaveTextContent('1.5 MiB');
    expect(screen.getByLabelText('cache memory budget')).toHaveTextContent('GiB');
  });
});

describe('NowPlayingBoard (F3)', () => {
  it('lists samples and shows live-position-unavailable, not a fake bar', async () => {
    const client = mockReadClient({
      statusSamples: async () => ({
        samples: [
          {
            internal_id: '1',
            id: null,
            voice: 'music',
            file: '/sounds/song.mp3',
            position: 0,
            position_ms: 0,
            total_frames: 480_000,
            total_ms: 10_000,
            sample_rate: 48_000,
            volume: 0.8,
            voice_volume: 1,
            speed: 1,
            loop_mode: true,
            progress_percent: 0,
          },
        ],
      }),
    } as Partial<DaemonClient>);
    renderWithClient(<NowPlayingBoard />, client);

    expect(await screen.findByText('song.mp3')).toBeInTheDocument();
    expect(screen.getByText('voice: music')).toBeInTheDocument();
    expect(screen.getByText('loop')).toBeInTheDocument();
    expect(screen.getByText(/live position unavailable/i)).toBeInTheDocument();
    expect(screen.queryByRole('progressbar')).toBeNull();
  });

  it('shows an empty state with no samples', async () => {
    const client = mockReadClient({ statusSamples: async () => ({ samples: [] }) } as Partial<DaemonClient>);
    renderWithClient(<NowPlayingBoard />, client);
    expect(await screen.findByText(/no samples playing/i)).toBeInTheDocument();
  });
});

describe('VoicesRack (F4)', () => {
  it('flags a ducked voice with its multiplier', async () => {
    const client = mockReadClient({
      statusVoices: async () => ({
        voices: [
          { id: 'music', sample_count: 1, volume: 1, ducking_multiplier: 0.1 },
          { id: 'fx', sample_count: 0, volume: 1, ducking_multiplier: 1 },
        ],
      }),
    } as Partial<DaemonClient>);
    renderWithClient(<VoicesRack />, client);
    expect(await screen.findByLabelText('music ducked')).toBeInTheDocument();
    expect(screen.getByText('ducked ×0.10')).toBeInTheDocument();
  });
});

describe('InputsRack (F5)', () => {
  it('distinguishes muted from live inputs', async () => {
    const client = mockReadClient({
      statusInputs: async () => ({
        inputs: [
          { index: 0, voice_id: 'mic', volume: 0.7, channels: 1, muted: false },
          { index: 1, voice_id: 'aux', volume: 0, channels: 2, muted: true },
        ],
      }),
    } as Partial<DaemonClient>);
    renderWithClient(<InputsRack />, client);
    expect(await screen.findByLabelText('aux muted')).toBeInTheDocument();
    expect(screen.getByText('mic')).toBeInTheDocument();
  });
});

describe('CacheTable (F6)', () => {
  it('shows memory and disk entries + sizes', async () => {
    const client = mockReadClient({
      statusCache: async () => ({
        memory: { entries: 3, size_bytes: 1_572_864, size_mb: 1.5 },
        disk: { entries: 10, size_bytes: 5_242_880, size_mb: 5 },
      }),
    } as Partial<DaemonClient>);
    renderWithClient(<CacheTable />, client);
    expect(await screen.findByText('1.5 MiB')).toBeInTheDocument();
    expect(screen.getByText('5.0 MiB')).toBeInTheDocument();
  });
});
