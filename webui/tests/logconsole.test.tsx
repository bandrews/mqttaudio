import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import { LogConsole } from '../src/features/logs/LogConsole';
import { ClientProvider } from '../src/state/QueryProvider';
import type { DaemonClient, ConnectionState } from '../src/api/client';
import type { Subscription, SubscriptionHandlers } from '../src/api/connection';

// A controllable mock client: captures the /ws handlers so the test can drive
// frames and close events, and returns a scripted probe classification.
class MockClient {
  handlers: SubscriptionHandlers | null = null;
  subscribeCount = 0;
  probeResult: Exclude<ConnectionState, 'connecting'> = 'live';
  subscribe(_path: string, handlers: SubscriptionHandlers): Subscription {
    this.subscribeCount += 1;
    this.handlers = handlers;
    return { close: () => {} };
  }
  async probeConnection(): Promise<Exclude<ConnectionState, 'connecting'>> {
    return this.probeResult;
  }
}

function renderConsole(client: MockClient) {
  return render(
    <ClientProvider client={client as unknown as DaemonClient}>
      <LogConsole />
    </ClientProvider>,
  );
}

describe('LogConsole over a mock /ws socket (F3/F4)', () => {
  it('welcome frame flips to live and records the daemon version', () => {
    const client = new MockClient();
    renderConsole(client);
    expect(screen.getByLabelText(/stream connecting/i)).toBeInTheDocument();
    act(() => client.handlers!.onMessage({ type: 'connected', version: '2.0.0' }));
    expect(screen.getByLabelText('stream live')).toBeInTheDocument();
    expect(screen.getByText(/daemon v2\.0\.0/)).toBeInTheDocument();
  });

  it('log frames append in order', () => {
    const client = new MockClient();
    renderConsole(client);
    act(() => {
      client.handlers!.onMessage({ type: 'connected', version: '2.0.0' });
      client.handlers!.onMessage({ type: 'log', message: 'first line' });
      client.handlers!.onMessage({ type: 'log', message: 'second line' });
    });
    const log = screen.getByRole('log');
    expect(log).toHaveTextContent('first line');
    expect(log).toHaveTextContent('second line');
    expect(log.textContent!.indexOf('first line')).toBeLessThan(log.textContent!.indexOf('second line'));
  });

  describe('reconnection (fake timers)', () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    it('close -> offline -> backoff reconnect -> gap marker on re-welcome', async () => {
      const client = new MockClient();
      client.probeResult = 'offline';
      renderConsole(client);
      act(() => client.handlers!.onMessage({ type: 'connected' }));
      expect(screen.getByLabelText('stream live')).toBeInTheDocument();

      // Socket closes; the hook probes (async) then schedules a backoff reconnect.
      await act(async () => {
        client.handlers!.onClose?.({});
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(screen.getByLabelText(/stream offline/i)).toBeInTheDocument();
      expect(client.subscribeCount).toBe(1);

      // Advancing past the backoff delay reconnects.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(60_000);
      });
      expect(client.subscribeCount).toBe(2);

      // The re-welcome marks the gap (messages during the disconnect were dropped).
      act(() => client.handlers!.onMessage({ type: 'connected' }));
      expect(screen.getByLabelText('stream live')).toBeInTheDocument();
      expect(screen.getByRole('log')).toHaveTextContent(/messages during the gap were dropped/i);
    });

    it('a 401 lands in unauthorized and does not blind-retry', async () => {
      const client = new MockClient();
      client.probeResult = 'unauthorized';
      renderConsole(client);
      act(() => client.handlers!.onMessage({ type: 'connected' }));

      await act(async () => {
        client.handlers!.onClose?.({});
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(screen.getByLabelText('stream unauthorized')).toBeInTheDocument();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(120_000);
      });
      expect(client.subscribeCount).toBe(1); // no reconnect attempted
    });
  });
});
