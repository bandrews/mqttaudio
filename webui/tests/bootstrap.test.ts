import { describe, it, expect } from 'vitest';
import { bootstrap } from '../src/api/bootstrap';
import { HttpError, type DaemonConnection, type SubscriptionHandlers, type Subscription } from '../src/api/connection';

class ScriptedConnection implements DaemonConnection {
  connection = { id: 't', label: 't', baseUrl: 'http://h:8080' };
  constructor(private readonly responses: Record<string, () => unknown>) {}
  async get<T>(path: string): Promise<T> {
    const handler = this.responses[path];
    if (!handler) throw new HttpError(404, path);
    const value = handler();
    if (value instanceof Error) throw value;
    return value as T;
  }
  async post<T>(): Promise<T> {
    return {} as T;
  }
  subscribe(_path: string, _handlers: SubscriptionHandlers): Subscription {
    return { close: () => {} };
  }
}

describe('bootstrap (DW9 auth detection)', () => {
  it('open daemon: /version 200 -> authRequired false with version', async () => {
    const conn = new ScriptedConnection({
      '/health': () => ({ status: 'ok', service: 'mqttaudio', version: '2.0.0' }),
      '/version': () => ({ name: 'mqttaudio', version: '2.0.0' }),
    });
    const result = await bootstrap(conn);
    expect(result).toMatchObject({ healthy: true, authRequired: false, service: 'mqttaudio' });
    expect(result.version?.version).toBe('2.0.0');
  });

  it('require_auth daemon: gated probe 401 -> authRequired true, no version yet', async () => {
    const conn = new ScriptedConnection({
      '/health': () => ({ status: 'ok', service: 'mqttaudio', version: '2.0.0' }),
      '/version': () => new HttpError(401, '/version'),
    });
    const result = await bootstrap(conn);
    expect(result).toMatchObject({ healthy: true, authRequired: true });
    expect(result.version).toBeUndefined();
  });

  it('unreachable daemon: /health throws -> healthy false', async () => {
    const conn = new ScriptedConnection({
      '/health': () => new Error('network down'),
    });
    const result = await bootstrap(conn);
    expect(result.healthy).toBe(false);
    expect(result.error).toContain('network');
  });
});
