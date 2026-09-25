// ABOUTME: Tests the Connect screen's auth-required paths with scripted transports.
// ABOUTME: A 401 on /version or the /ws probe reveals the token field; a valid token then connects.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Connect } from '../src/components/Connect';
import { HttpError, type Connection, type DaemonConnection, type Subscription, type SubscriptionHandlers } from '../src/api/connection';

// A transport whose gated probe (/version) 401s until the connection carries a
// token, then succeeds — driving the auth-required path.
function makeTransport(connection: Connection): DaemonConnection {
  return {
    connection,
    async get<T>(path: string): Promise<T> {
      if (path === '/health') return { status: 'ok', service: 'mqttaudio', version: '2.0.0' } as T;
      if (path === '/version') {
        if (!connection.token) throw new HttpError(401, '/version');
        return { name: 'mqttaudio', version: '2.0.0' } as T;
      }
      throw new HttpError(404, path);
    },
    async post<T>(): Promise<T> {
      return {} as T;
    },
    subscribe(_p: string, _h: SubscriptionHandlers): Subscription {
      return { close: () => {} };
    },
  };
}

// A daemon with a token but without require_auth: /version is open, while the
// /ws probe answers 401 until the connection carries the right token, then
// refuses only the missing WebSocket upgrade (400).
function makeTokenOnlyTransport(connection: Connection): DaemonConnection {
  return {
    connection,
    async get<T>(path: string): Promise<T> {
      if (path === '/health') return { status: 'ok', service: 'mqttaudio', version: '2.0.0' } as T;
      if (path === '/version') return { name: 'mqttaudio', version: '2.0.0' } as T;
      if (path === '/ws') {
        throw new HttpError(connection.token === 'sekret' ? 400 : 401, '/ws');
      }
      throw new HttpError(404, path);
    },
    async post<T>(): Promise<T> {
      return {} as T;
    },
    subscribe(_p: string, _h: SubscriptionHandlers): Subscription {
      return { close: () => {} };
    },
  };
}

describe('Connect — auth-required path (DW9)', () => {
  it('reveals the token field on 401, then connects once a token is supplied', async () => {
    const user = userEvent.setup();
    const onConnected = vi.fn();
    render(<Connect onConnected={onConnected} makeTransport={makeTransport} />);

    // No token field initially.
    expect(screen.queryByLabelText('Bearer token')).toBeNull();

    await user.click(screen.getByRole('button', { name: /connect/i }));

    // 401 -> token field appears with an informational prompt.
    const tokenField = await screen.findByLabelText('Bearer token');
    expect(tokenField).toBeInTheDocument();
    expect(onConnected).not.toHaveBeenCalled();

    await user.type(tokenField, 'sekret');
    await user.click(screen.getByRole('button', { name: /connect/i }));

    await waitFor(() => expect(onConnected).toHaveBeenCalledTimes(1));
    // With a valid token the gated probe now succeeds, so the connection carries
    // the token and the daemon's version is read.
    const [, result, connection] = onConnected.mock.calls[0]!;
    expect(result.version.version).toBe('2.0.0');
    expect(connection.token).toBe('sekret');
  });

  it('asks for a token when only the command routes need one, and refuses a wrong token', async () => {
    const user = userEvent.setup();
    const onConnected = vi.fn();
    render(<Connect onConnected={onConnected} makeTransport={makeTokenOnlyTransport} />);

    await user.click(screen.getByRole('button', { name: /connect/i }));

    // /version is open, but the /ws probe 401s -> the token field appears.
    const tokenField = await screen.findByLabelText('Bearer token');
    expect(onConnected).not.toHaveBeenCalled();

    await user.type(tokenField, 'wrong');
    await user.click(screen.getByRole('button', { name: /connect/i }));
    expect(await screen.findByText('Authentication failed — check the token.')).toBeInTheDocument();
    expect(onConnected).not.toHaveBeenCalled();

    await user.clear(tokenField);
    await user.type(tokenField, 'sekret');
    await user.click(screen.getByRole('button', { name: /connect/i }));

    await waitFor(() => expect(onConnected).toHaveBeenCalledTimes(1));
    const [, result, connection] = onConnected.mock.calls[0]!;
    expect(result.version.version).toBe('2.0.0');
    expect(connection.token).toBe('sekret');
  });
});
