import { describe, it, expect } from 'vitest';
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { renderWithClient, mockReadClient } from './_helpers';
import { ConfigView } from '../src/features/config/ConfigView';
import { DaemonClient } from '../src/api/client';
import type { DaemonConnection, SubscriptionHandlers, Subscription } from '../src/api/connection';
import type { DaemonClient as DC } from '../src/api/client';

describe('DaemonClient.config (W8)', () => {
  it('reads GET /config', async () => {
    let path = '';
    const conn: DaemonConnection = {
      connection: { id: 't', label: 't', baseUrl: '/api' },
      async get<T>(p: string) {
        path = p;
        return { audio: {} } as T;
      },
      async post<T>() {
        return {} as T;
      },
      subscribe(_p: string, _h: SubscriptionHandlers): Subscription {
        return { close: () => {} };
      },
    };
    await new DaemonClient(conn).config();
    expect(path).toBe('/config');
  });
});

const config = {
  audio: { master_gain: 1, output_ceiling_db: -1, channel_aliases: { front_left: 0 } },
  ducking_rules: [{ primary_voice: 'narration', ducked_voices: ['music'], target_volume: 0.1 }],
  inputs: [],
  http: { auth_token: null, port: 8080 },
};

describe('ConfigView (W8)', () => {
  it('shows the running config, the restart banner, an output-stage snippet, and live ducking', async () => {
    const client = mockReadClient({
      config: async () => config,
      metrics: async () =>
        ({ ducking: { music: 0.1 } }) as unknown as Awaited<ReturnType<DC['metrics']>>,
    } as Partial<DC>);
    renderWithClient(<ConfigView />, client);

    // Restart-required banner (config is read-once, DW8).
    expect(await screen.findByText(/read once at startup/i)).toBeInTheDocument();
    // The output-stage snippet reflects the current master_gain and is flagged restart-required.
    const snippet = await screen.findByLabelText('audio output stage snippet');
    expect(snippet).toHaveTextContent('master_gain');
    expect(snippet).toHaveTextContent('output_ceiling_db');
    // Live ducking shows the ducked voice from /metrics.
    expect(screen.getByText(/music: ×0\.10/)).toBeInTheDocument();
    // The running config is shown read-only.
    expect(screen.getByLabelText('running config')).toHaveTextContent('ducking_rules');
  });

  it('the section editor validates JSON and builds a restart-required snippet', async () => {
    const user = userEvent.setup();
    const client = mockReadClient({
      config: async () => config,
      metrics: async () => ({ ducking: {} }) as unknown as Awaited<ReturnType<DC['metrics']>>,
    } as Partial<DC>);
    renderWithClient(<ConfigView />, client);

    // Default section is ducking_rules; load current then build the snippet.
    await screen.findByLabelText('section json');
    await user.click(screen.getByRole('button', { name: 'Load current' }));
    await user.click(screen.getByRole('button', { name: 'Build snippet' }));
    const snippet = await screen.findByLabelText('ducking_rules snippet');
    expect(snippet).toHaveTextContent('ducking_rules');
    expect(snippet).toHaveTextContent('narration');
  });
});
