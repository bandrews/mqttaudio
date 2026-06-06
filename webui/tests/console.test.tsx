import { describe, it, expect, vi } from 'vitest';
import { screen, fireEvent, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { renderWithClient } from './_helpers';
import { SampleControl, VoiceControl } from '../src/features/console/CommandConsole';
import { CueLauncher } from '../src/features/console/CueLauncher';
import { RawCommandEditor } from '../src/features/console/RawCommandEditor';
import { validateSpeed, validateVolume, validateInternalId } from '../src/features/console/validation';
import { mergeMacros, parseMacroList } from '../src/features/console/macroMerge';
import type { DaemonClient } from '../src/api/client';

function spyClient() {
  const methods = [
    'rawCommand', 'stop', 'volume', 'seek', 'speed', 'voiceVolume', 'voiceFadeOut', 'voiceStop',
    'inputVolume', 'inputMute', 'precache', 'cacheInvalidate', 'cacheReload', 'cacheClear', 'stopAll',
  ] as const;
  const client: Record<string, ReturnType<typeof vi.fn>> = {};
  for (const m of methods) client[m] = vi.fn().mockResolvedValue({ success: true, message: 'Command accepted' });
  return client as unknown as DaemonClient & Record<string, ReturnType<typeof vi.fn>>;
}

describe('validation (mirrors the daemon)', () => {
  it('warns on out-of-range volume but does not error', () => {
    expect(validateVolume(0.5)).toEqual({});
    expect(validateVolume(2).warning).toMatch(/clamp/);
  });
  it('errors on reverse + pitch correction, warns out of range otherwise', () => {
    expect(validateSpeed(-2, true).error).toMatch(/reverse/);
    expect(validateSpeed(9, true).warning).toMatch(/clamp/);
    expect(validateSpeed(-50, false)).toEqual({});
  });
  it('warns when internal_id is not digits', () => {
    expect(validateInternalId('7')).toEqual({});
    expect(validateInternalId('abc').warning).toMatch(/digits/);
  });
});

describe('macro precedence (API-CONTRACT §1)', () => {
  it('command params > macro[0] > macro[1]', () => {
    const defs = { quiet: { volume: 0.2 }, wholeroom: { channel_map: [1], volume: 0.9 } };
    const merged = mergeMacros({ voice: 'bg' }, ['quiet', 'wholeroom'], defs);
    expect(merged).toEqual({ channel_map: [1], volume: 0.2, voice: 'bg' });
  });
  it('parses a macro list', () => {
    expect(parseMacroList('quiet, wholeroom  extra')).toEqual(['quiet', 'wholeroom', 'extra']);
  });
});

describe('CueLauncher (F4)', () => {
  it('previews and emits the full play surface via /command', async () => {
    const user = userEvent.setup();
    const client = spyClient();
    renderWithClient(<CueLauncher />, client);
    await user.type(screen.getByLabelText('file'), '/s.wav');
    await user.type(screen.getByLabelText('volume'), '0.4');
    const preview = screen.getByLabelText('play JSON preview');
    await waitFor(() => expect(preview).toHaveTextContent('"file": "/s.wav"'));
    await user.click(screen.getByRole('button', { name: 'Play' }));
    expect(client.rawCommand).toHaveBeenCalledWith({
      command: 'play',
      message: { file: '/s.wav', volume: 0.4 },
    });
  });
});

describe('RawCommandEditor (F3)', () => {
  it('sends valid JSON and rejects invalid JSON without sending', async () => {
    const user = userEvent.setup();
    const client = spyClient();
    renderWithClient(<RawCommandEditor />, client);
    // Default template is valid play JSON with the full surface.
    await user.click(screen.getByRole('button', { name: 'Send' }));
    expect(client.rawCommand).toHaveBeenCalledTimes(1);
    const sent = (client.rawCommand as unknown as ReturnType<typeof vi.fn>).mock.calls[0]![0] as {
      command: string;
      message: { channel_map?: unknown };
    };
    expect(sent.command).toBe('play');
    expect(sent.message.channel_map).toBeDefined();

    // Replace with invalid JSON -> error, no second send.
    fireEvent.change(screen.getByLabelText('raw command JSON'), { target: { value: '{ not json' } });
    await user.click(screen.getByRole('button', { name: 'Send' }));
    expect(await screen.findByLabelText('json error')).toBeInTheDocument();
    expect(client.rawCommand).toHaveBeenCalledTimes(1);
  });
});

describe('Command forms', () => {
  it('shows the empty-selector warning by default', () => {
    renderWithClient(<SampleControl />, spyClient());
    expect(screen.getByLabelText('empty selector warning')).toBeInTheDocument();
  });

  it('voice fade-out emits the typed time_ms key', async () => {
    const user = userEvent.setup();
    const client = spyClient();
    renderWithClient(<VoiceControl />, client);
    await user.type(screen.getByLabelText('voice'), 'music');
    await user.click(screen.getByRole('button', { name: 'Fade out' }));
    expect(client.voiceFadeOut).toHaveBeenCalledWith({ voice: 'music', time_ms: 2000 });
  });

  it('stop sends the selector once a criterion is set', async () => {
    const user = userEvent.setup();
    const client = spyClient();
    renderWithClient(<SampleControl />, client);
    await user.type(screen.getByLabelText('id'), 'cue-1');
    await user.click(screen.getByRole('button', { name: 'Stop' }));
    expect(client.stop).toHaveBeenCalledWith({ id: 'cue-1' });
  });
});
