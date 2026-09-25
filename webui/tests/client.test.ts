// ABOUTME: Unit tests for DaemonClient using a connection that records requests.
// ABOUTME: Checks command routes and bodies, input warnings, read paths and the connection probe.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { DaemonClient } from '../src/api/client';
import {
  HttpError,
  type DaemonConnection,
  type Subscription,
  type SubscriptionHandlers,
} from '../src/api/connection';

interface PostCall {
  path: string;
  body: unknown;
}

class RecordingConnection implements DaemonConnection {
  connection = { id: 't', label: 't', baseUrl: 'http://h:8080' };
  posts: PostCall[] = [];
  gets: string[] = [];
  getResult: unknown = {};

  async get<T>(path: string): Promise<T> {
    this.gets.push(path);
    return this.getResult as T;
  }
  async post<T>(path: string, body?: unknown): Promise<T> {
    this.posts.push({ path, body });
    return { success: true } as T;
  }
  subscribe(_path: string, _handlers: SubscriptionHandlers): Subscription {
    return { close: () => {} };
  }
  last(): PostCall {
    return this.posts[this.posts.length - 1]!;
  }
}

describe('DaemonClient command JSON (API-CONTRACT §2, DW10)', () => {
  let conn: RecordingConnection;
  let client: DaemonClient;
  beforeEach(() => {
    conn = new RecordingConnection();
    client = new DaemonClient(conn);
  });

  it('routes the full play surface through /command as nested {command, message}', async () => {
    await client.play({
      file: '/s.wav',
      volume: 0.5,
      loop: true,
      channel_map: [{ src: 0, dest: 4, gain: 0.5 }],
      mode: 'stream',
      freshness: 'pinned',
    });
    expect(conn.last().path).toBe('/command');
    expect(conn.last().body).toEqual({
      command: 'play',
      message: {
        file: '/s.wav',
        volume: 0.5,
        loop: true,
        channel_map: [{ src: 0, dest: 4, gain: 0.5 }],
        mode: 'stream',
        freshness: 'pinned',
      },
    });
  });

  it('warns when play volume is outside [0,4] but still sends it (daemon clamps)', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await client.play({ file: '/s.wav', volume: 2 });
    expect(warn).not.toHaveBeenCalled();
    await client.play({ file: '/s.wav', volume: 5 });
    expect(warn).toHaveBeenCalledOnce();
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('outside [0,4]'));
    expect((conn.last().body as { message: { volume: number } }).message.volume).toBe(5);
    warn.mockRestore();
  });

  it('warns when a volume command is outside [0,4] but still sends it (daemon clamps)', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await client.volume({ id: 's', volume: 3 });
    expect(warn).not.toHaveBeenCalled();
    await client.volume({ id: 's', volume: -1 });
    expect(warn).toHaveBeenCalledOnce();
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('outside [0,4]'));
    expect(conn.last().body).toEqual({ id: 's', volume: -1 });
    warn.mockRestore();
  });

  it('stop warns on an empty selector and does not fabricate one', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await client.stop({});
    expect(warn).toHaveBeenCalledOnce();
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('the daemon refuses it with a 400'));
    expect(conn.last().path).toBe('/stop');
    expect(conn.last().body).toEqual({});
    warn.mockRestore();
  });

  it('keeps internal_id as a string in the selector', async () => {
    await client.stop({ internal_id: '7', fade_out_ms: 500 });
    expect(conn.last().body).toEqual({ internal_id: '7', fade_out_ms: 500 });
  });

  it('voiceFadeOut uses the typed body key time_ms (not the wire key time)', async () => {
    await client.voiceFadeOut({ voice: 'music', time_ms: 2000 });
    expect(conn.last()).toEqual({ path: '/voice/fade_out', body: { voice: 'music', time_ms: 2000 } });
  });

  it('input is a string for input_volume/input_mute', async () => {
    await client.inputVolume({ input: '0', volume: 0.5 });
    expect(conn.last().body).toEqual({ input: '0', volume: 0.5 });
    await client.inputMute({ input: 'mic', mute: true });
    expect(conn.last().body).toEqual({ input: 'mic', mute: true });
  });

  it('speed warns outside the pitch-correction-gated range', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await client.speed({ id: 's', speed: 9, pitch_correction: true });
    expect(warn).toHaveBeenCalledOnce();
    expect(warn).toHaveBeenLastCalledWith(expect.stringContaining('the daemon will clamp it'));
    await client.speed({ id: 's', speed: -2, pitch_correction: false });
    expect(warn).toHaveBeenCalledTimes(1); // -2 is within [-100,100], no extra warn
    warn.mockRestore();
  });

  it('speed warns that the daemon refuses speed 0 and reverse with pitch correction', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await client.speed({ id: 's', speed: -2, pitch_correction: true });
    expect(warn).toHaveBeenLastCalledWith(
      expect.stringContaining('the daemon refuses it with a 400'),
    );
    await client.speed({ id: 's', speed: 0, pitch_correction: false });
    expect(warn).toHaveBeenLastCalledWith(
      expect.stringContaining('the daemon refuses it with a 400'),
    );
    expect(warn).toHaveBeenCalledTimes(2);
    expect(conn.last().body).toEqual({ id: 's', speed: 0, pitch_correction: false });
    warn.mockRestore();
  });

  it('stopall and cache_clear post empty bodies to their routes', async () => {
    await client.stopAll();
    expect(conn.last()).toEqual({ path: '/stopall', body: {} });
    await client.cacheClear();
    expect(conn.last()).toEqual({ path: '/cache/clear', body: {} });
  });

  it('rawCommand forwards an arbitrary payload to /command', async () => {
    await client.rawCommand({ command: 'play', message: { file: '/x.wav' } });
    expect(conn.last()).toEqual({ path: '/command', body: { command: 'play', message: { file: '/x.wav' } } });
  });
});

describe('DaemonClient read endpoints (§5)', () => {
  it('reads hit the right paths and return the parsed payload', async () => {
    const conn = new RecordingConnection();
    const client = new DaemonClient(conn);
    conn.getResult = { status: 'running' };
    await client.status();
    await client.statusSamples();
    await client.statusVoices();
    await client.statusInputs();
    await client.statusCache();
    await client.metrics();
    await client.version();
    await client.health();
    expect(conn.gets).toEqual([
      '/status',
      '/status/samples',
      '/status/voices',
      '/status/inputs',
      '/status/cache',
      '/metrics',
      '/version',
      '/health',
    ]);
  });
});

// Answers each GET from a script; an Error result is thrown, a missing path is a 404.
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

describe('DaemonClient.probeConnection', () => {
  const version = () => ({ name: 'mqttaudio', version: '2.0.0' });

  it('is unauthorized when the /ws probe 401s, even though /version is open', async () => {
    const client = new DaemonClient(
      new ScriptedConnection({ '/ws': () => new HttpError(401, '/ws'), '/version': version }),
    );
    expect(await client.probeConnection()).toBe('unauthorized');
  });

  it('is live when /ws refuses only the missing upgrade', async () => {
    const client = new DaemonClient(
      new ScriptedConnection({ '/ws': () => new HttpError(400, '/ws'), '/version': version }),
    );
    expect(await client.probeConnection()).toBe('live');
  });

  it('falls back to /version when websockets are disabled (/ws 404)', async () => {
    const gated = new DaemonClient(
      new ScriptedConnection({ '/version': () => new HttpError(401, '/version') }),
    );
    expect(await gated.probeConnection()).toBe('unauthorized');
    const open = new DaemonClient(new ScriptedConnection({ '/version': version }));
    expect(await open.probeConnection()).toBe('live');
  });

  it('is offline when the daemon cannot be reached', async () => {
    const client = new DaemonClient(
      new ScriptedConnection({
        '/ws': () => new TypeError('Failed to fetch'),
        '/version': () => new TypeError('Failed to fetch'),
      }),
    );
    expect(await client.probeConnection()).toBe('offline');
  });
});
