// ABOUTME: Tests for the mixer's windowed check, SampleTransport, VoiceStrip and InputStrip.
// ABOUTME: Uses a spy client to check the commands each control sends and the errors it shows.

import { describe, it, expect, vi } from 'vitest';
import { fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { renderWithClient } from './_helpers';
import { SampleTransport } from '../src/features/mixer/SampleTransport';
import { VoiceStrip, InputStrip } from '../src/features/mixer/strips';
import { isWindowed } from '../src/features/mixer/windowed';
import type { DaemonClient } from '../src/api/client';
import { HttpError } from '../src/api/connection';
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

/** A spy whose `method` fails the way the daemon refuses a command. */
function refusing(method: string, status: number, path: string, error: string) {
  const client = spy();
  (client[method] as ReturnType<typeof vi.fn>).mockRejectedValue(
    new HttpError(status, path, JSON.stringify({ success: false, error })),
  );
  return client;
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

  it('shows the daemon error when a seek is refused', async () => {
    const error = 'Seek does not apply to windowed (streamed) sounds';
    const client = refusing('seek', 409, '/seek', error);
    renderWithClient(<SampleTransport sample={sample()} />, client);
    fireEvent.change(screen.getByLabelText('seek 1'), { target: { value: 5000 } });
    expect(client.seek).toHaveBeenCalledWith({ internal_id: '1', position_ms: 5000 });
    expect(await screen.findByRole('alert')).toHaveTextContent(error);
  });

  it('shows the daemon error when a speed change is refused', async () => {
    const error = 'Speed does not apply to windowed (streamed) sounds';
    const client = refusing('speed', 409, '/speed', error);
    renderWithClient(<SampleTransport sample={sample()} />, client);
    fireEvent.change(screen.getByLabelText('speed 1'), { target: { value: 2 } });
    expect(client.speed).toHaveBeenCalledWith({
      internal_id: '1',
      speed: 2,
      pitch_correction: false,
    });
    expect(await screen.findByRole('alert')).toHaveTextContent(error);
  });

  it('shows the daemon error when a stop is refused', async () => {
    const user = userEvent.setup();
    const error = 'No playing sound matched the selector';
    const client = refusing('stop', 404, '/stop', error);
    renderWithClient(<SampleTransport sample={sample()} />, client);
    await user.click(screen.getByRole('button', { name: 'Stop' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(error);
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

  it('shows the daemon error when a voice command is refused', async () => {
    const user = userEvent.setup();
    const error = "Voice 'music' has no sounds";
    const client = refusing('voiceFadeOut', 404, '/voice/fade_out', error);
    renderWithClient(
      <VoiceStrip voice={{ id: 'music', sample_count: 1, volume: 1, ducking_multiplier: 1 }} />,
      client,
    );
    await user.click(screen.getByRole('button', { name: 'Fade out' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(error);
  });

  it('shows the daemon error when a voice volume change is refused', async () => {
    const error = "Voice 'music' not found";
    const client = refusing('voiceVolume', 404, '/voice/volume', error);
    renderWithClient(
      <VoiceStrip voice={{ id: 'music', sample_count: 1, volume: 1, ducking_multiplier: 1 }} />,
      client,
    );
    fireEvent.change(screen.getByLabelText('voice music volume'), { target: { value: 0.5 } });
    expect(client.voiceVolume).toHaveBeenCalledWith({ voice: 'music', volume: 0.5 });
    expect(await screen.findByRole('alert')).toHaveTextContent(error);
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

  it('shows the daemon error when unmuting the talkback microphone is refused', async () => {
    const user = userEvent.setup();
    const error = 'The talkback microphone opens only through a talkback lease';
    const client = refusing('inputMute', 403, '/input/mute', error);
    renderWithClient(
      <InputStrip
        input={{ index: 0, voice_id: 'talkback', volume: 0, channels: 1, muted: true }}
      />,
      client,
    );
    const mute = screen.getByRole('checkbox', { name: 'mute input 0' });
    await user.click(mute);
    expect(client.inputMute).toHaveBeenCalledWith({ input: '0', mute: false });
    expect(await screen.findByRole('alert')).toHaveTextContent(error);
    expect(mute).toBeChecked();
  });

  it('shows the daemon error when an input volume change is refused', async () => {
    const error = 'The talkback microphone opens only through a talkback lease';
    const client = refusing('inputVolume', 403, '/input/volume', error);
    renderWithClient(
      <InputStrip
        input={{ index: 0, voice_id: 'talkback', volume: 0, channels: 1, muted: true }}
      />,
      client,
    );
    fireEvent.change(screen.getByLabelText('input 0 volume'), { target: { value: 0.5 } });
    expect(client.inputVolume).toHaveBeenCalledWith({ input: '0', volume: 0.5 });
    expect(await screen.findByRole('alert')).toHaveTextContent(error);
  });
});
