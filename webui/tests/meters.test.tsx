import { describe, it, expect, vi } from 'vitest';
import { screen, act, waitFor } from '@testing-library/react';
import { renderWithClient, mockReadClient } from './_helpers';
import { Meters } from '../src/features/telemetry/Meters';
import { DaemonClient } from '../src/api/client';
import type { DaemonConnection, SubscriptionHandlers, Subscription } from '../src/api/connection';
import type { DaemonClient as DC } from '../src/api/client';

describe('DaemonClient state channel (W7)', () => {
  it('subscribeState opens /ws/state and meters() reads the poll fallback', async () => {
    let subscribedPath = '';
    let gotPath = '';
    const conn: DaemonConnection = {
      connection: { id: 't', label: 't', baseUrl: '/api' },
      async get<T>(path: string) {
        gotPath = path;
        return { output: [0.1, 0.2] } as T;
      },
      async post<T>() {
        return {} as T;
      },
      subscribe(path: string, _h: SubscriptionHandlers): Subscription {
        subscribedPath = path;
        return { close: () => {} };
      },
    };
    const client = new DaemonClient(conn);
    client.subscribeState({ onMessage: () => {} });
    expect(subscribedPath).toBe('/ws/state');
    await client.meters();
    expect(gotPath).toBe('/status/meters');
  });
});

describe('Meters (W7)', () => {
  it('renders live output bars from a /ws/state tick frame', async () => {
    let captured: SubscriptionHandlers | null = null;
    const client = mockReadClient({
      telemetry: async () => ({ enabled: true }),
      subscribeState: (h: SubscriptionHandlers) => {
        captured = h;
        return { close: () => {} };
      },
    } as Partial<DC>);
    renderWithClient(<Meters />, client);

    await waitFor(() => expect(captured).not.toBeNull());
    act(() => captured!.onMessage({ type: 'tick', samples: [], meters: { output: [0.5, 0.9] } }));

    const bar0 = await screen.findByLabelText('output meter 0');
    expect(bar0).toHaveAttribute('aria-valuenow', '50');
    expect(screen.getByLabelText('output meter 1')).toHaveAttribute('aria-valuenow', '90');
  });

  it('shows the enable hint when telemetry is off', async () => {
    const subscribeState = vi.fn();
    const client = mockReadClient({
      telemetry: async () => ({ enabled: false }),
      subscribeState,
    } as Partial<DC>);
    renderWithClient(<Meters />, client);
    expect(await screen.findByText(/enable telemetry/i)).toBeInTheDocument();
    // With telemetry off, the state channel is not subscribed.
    expect(subscribeState).not.toHaveBeenCalled();
  });
});
