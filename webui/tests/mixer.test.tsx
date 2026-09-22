import { describe, it, expect, vi } from 'vitest';
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { renderWithClient } from './_helpers';
import { SampleTransport } from '../src/features/mixer/SampleTransport';
import { VoiceStrip, InputStrip } from '../src/features/mixer/strips';
import { isWindowed } from '../src/features/mixer/windowed';
import type { DaemonClient } from '../src/api/client';
import type { SampleInfo } from '../src/api/contract';

function sample(overrides: Partial<SampleInfo> = {}): SampleInfo {
  return {
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
    loop_mode: false,
    progress_percent: 0,
    ...overrides,
  };
}

function spy() {
  const client: Record<string, ReturnType<typeof vi.fn>> = {};
  for (const m of ['stop', 'seek', 'speed', 'voiceVolume', 'voiceFadeOut', 'voiceStop', 'inputVolume', 'inputMute']) {
    client[m] = vi.fn().mockResolvedValue({ success: true });
  }
  return client as unknown as DaemonClient & Record<string, ReturnType<typeof vi.fn>>;
}

describe('windowed inference (W5 F3)', () => {
  it('treats total_frames === 0 as windowed/streamed', () => {
    expect(isWindowed({ total_frames: 0 })).toBe(true);
    expect(isWindowed({ total_frames: 480_000 })).toBe(false);
  });
});

describe('SampleTransport (F1-F3)', () => {
  it('gates a windowed sample: streamed badge, no seek, forward-only note', () => {
    renderWithClient(<SampleTransport sample={sample({ total_frames: 0, total_ms: 0 })} />, spy());
    expect(screen.getByLabelText('1 streamed')).toBeInTheDocument();
    expect(screen.getByText(/forward-only/i)).toBeInTheDocument();
    expect(screen.queryByLabelText('seek 1')).toBeNull();
    expect(screen.queryByLabelText('speed 1')).toBeNull();
  });

  it('shows seek + speed for a normal sample, with a pitch toggle, and stops it', async () => {
    const user = userEvent.setup();
    const client = spy();
    renderWithClient(<SampleTransport sample={sample()} />, client);
    expect(screen.getByLabelText('seek 1')).toBeInTheDocument();
    expect(screen.getByLabelText('speed 1')).toBeInTheDocument();
    // Enabling pitch correction surfaces the reverse-disabled note.
    await user.click(screen.getByRole('checkbox', { name: /pitch/i }));
    expect(screen.getByText(/reverse.*disabled/i)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Stop' }));
    expect(client.stop).toHaveBeenCalledWith({ internal_id: '1' });
  });
});

describe('VoiceStrip (F4)', () => {
  it('fades out with time_ms and stops the voice', async () => {
    const user = userEvent.setup();
    const client = spy();
    renderWithClient(
      <VoiceStrip voice={{ id: 'music', sample_count: 1, volume: 1, ducking_multiplier: 1 }} />,
      client,
    );
    await user.click(screen.getByRole('button', { name: 'Fade out' }));
    expect(client.voiceFadeOut).toHaveBeenCalledWith({ voice: 'music', time_ms: 2000 });
    await user.click(screen.getByRole('button', { name: 'Stop' }));
    expect(client.voiceStop).toHaveBeenCalledWith({ voice: 'music' });
  });
});

describe('InputStrip (F5)', () => {
  it('mutes an input (string index)', async () => {
    const user = userEvent.setup();
    const client = spy();
    renderWithClient(
      <InputStrip input={{ index: 0, voice_id: 'mic', volume: 0.7, channels: 1, muted: false }} />,
      client,
    );
    await user.click(screen.getByRole('checkbox', { name: 'mute input 0' }));
    expect(client.inputMute).toHaveBeenCalledWith({ input: '0', mute: true });
  });
});
