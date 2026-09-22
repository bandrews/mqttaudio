import { describe, it, expect, vi } from 'vitest';
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { renderWithClient, mockReadClient } from './_helpers';
import { MatrixMixer } from '../src/features/matrix/MatrixMixer';
import {
  buildChannelMap,
  setGain,
  summedDests,
  toggleRoute,
  type Route,
} from '../src/features/matrix/routeModel';
import type { DaemonClient } from '../src/api/client';

describe('routeModel', () => {
  it('toggles routes on and off', () => {
    let r: Route[] = [];
    r = toggleRoute(r, 0, 4);
    expect(r).toEqual([{ src: 0, dest: 4 }]);
    r = toggleRoute(r, 0, 4);
    expect(r).toEqual([]);
  });

  it('detects destinations summed from multiple sources', () => {
    const r: Route[] = [
      { src: 0, dest: 0 },
      { src: 1, dest: 0 },
      { src: 2, dest: 1 },
    ];
    expect([...summedDests(r)]).toEqual([0]);
  });

  it('builds channel_map, omitting unity gain and sorting', () => {
    let r: Route[] = [
      { src: 1, dest: 5 },
      { src: 0, dest: 4 },
    ];
    r = setGain(r, 0, 4, 0.5);
    r = setGain(r, 1, 5, 1);
    expect(buildChannelMap(r)).toEqual([
      { src: 0, dest: 4, gain: 0.5 },
      { src: 1, dest: 5 },
    ]);
  });
});

function statusClient() {
  return mockReadClient({
    status: async () =>
      ({
        status: 'running',
        version: '2.0.0',
        active_samples: 0,
        active_inputs: 0,
        active_voices: 0,
        output_channels: 8,
        clip_count: 0,
        xruns: 0,
        cache: { memory: { entries: 0, size_bytes: 0 }, disk: { entries: 0, size_bytes: 0 } },
      }) as Awaited<ReturnType<DaemonClient['status']>>,
    rawCommand: vi.fn().mockResolvedValue({ success: true, message: 'ok' }),
  } as Partial<DaemonClient>);
}

describe('MatrixMixer (F1-F5)', () => {
  it('routes via /command with a clip-risk badge for summed destinations', async () => {
    const user = userEvent.setup();
    const client = statusClient();
    renderWithClient(<MatrixMixer />, client);

    // 8 dest columns from output_channels.
    await screen.findByLabelText('route 0 to 7');

    await user.type(screen.getByLabelText('matrix file'), '/quad.wav');
    // Route src0+src1 both into dest 0 (a summed destination -> clip risk).
    await user.click(screen.getByLabelText('route 0 to 0'));
    await user.click(screen.getByLabelText('route 1 to 0'));
    expect(screen.getByLabelText('dest 0 clip risk')).toBeInTheDocument();

    const preview = screen.getByLabelText('matrix JSON preview');
    expect(within(preview).getByText(/"channel_map"/)).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Play routed' }));
    const raw = client.rawCommand as unknown as ReturnType<typeof vi.fn>;
    expect(raw).toHaveBeenCalledTimes(1);
    const sent = raw.mock.calls[0]![0] as { command: string; message: { channel_map: unknown[] } };
    expect(sent.command).toBe('play');
    expect(sent.message.channel_map).toEqual([
      { src: 0, dest: 0 },
      { src: 1, dest: 0 },
    ]);
  });

  it('disables Play with no routes selected', async () => {
    const client = statusClient();
    renderWithClient(<MatrixMixer />, client);
    await screen.findByLabelText('route 0 to 0');
    expect(screen.getByRole('button', { name: 'Play routed' })).toBeDisabled();
  });
});
