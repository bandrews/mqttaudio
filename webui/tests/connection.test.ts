import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  ConnectionRegistry,
  HttpError,
  joinUrl,
  toWebSocketUrl,
  type Connection,
} from '../src/api/connection';
import { ProxyBrowserConnection } from '../src/api/connection.browser';

describe('Connection entity + registry (DW2)', () => {
  it('round-trips a Connection entity', () => {
    const c: Connection = { id: 'a', label: 'Studio', baseUrl: 'http://host:8080', token: 'secret' };
    expect(c).toMatchObject({ id: 'a', label: 'Studio', baseUrl: 'http://host:8080', token: 'secret' });
  });

  it('holds more than one connection, keyed by id (multi-instance-ready)', () => {
    const reg = new ConnectionRegistry();
    reg.add({ id: 'one', label: 'One', baseUrl: 'http://one:8080' });
    reg.add({ id: 'two', label: 'Two', baseUrl: 'http://two:8080' });
    expect(reg.size).toBe(2);
    expect(reg.get('two')?.baseUrl).toBe('http://two:8080');
    expect(reg.list().map((c) => c.id).sort()).toEqual(['one', 'two']);
    expect(reg.remove('one')).toBe(true);
    expect(reg.size).toBe(1);
  });
});

describe('url helpers', () => {
  it('joins base + path without doubling slashes', () => {
    expect(joinUrl('http://h:8080/', '/status')).toBe('http://h:8080/status');
    expect(joinUrl('http://h:8080', 'status')).toBe('http://h:8080/status');
  });

  it('derives ws:// and wss:// and appends ?token= for a direct connection', () => {
    expect(toWebSocketUrl('http://h:8080', '/ws')).toBe('ws://h:8080/ws');
    expect(toWebSocketUrl('https://h', '/ws')).toBe('wss://h/ws');
    expect(toWebSocketUrl('http://h:8080', '/ws', 'tok en')).toBe('ws://h:8080/ws?token=tok%20en');
  });
});

describe('ProxyBrowserConnection (the only fetch/WebSocket module)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function jsonResponse(body: unknown, init?: { status?: number }) {
    return new Response(JSON.stringify(body), {
      status: init?.status ?? 200,
      headers: { 'Content-Type': 'application/json' },
    });
  }

  it('GET issues the right method/headers and parses JSON', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ status: 'ok' }));
    vi.stubGlobal('fetch', fetchMock);
    const conn = new ProxyBrowserConnection({ id: 'x', label: 'x', baseUrl: 'http://h:8080' });
    const out = await conn.get<{ status: string }>('/health');
    expect(out).toEqual({ status: 'ok' });
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe('http://h:8080/health');
    expect(init.method).toBe('GET');
    expect(init.headers.Authorization).toBeUndefined();
  });

  it('attaches Authorization: Bearer when a direct connection carries a token', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ ok: true }));
    vi.stubGlobal('fetch', fetchMock);
    const conn = new ProxyBrowserConnection({ id: 'x', label: 'x', baseUrl: 'http://h:8080', token: 'sec' });
    await conn.get('/status');
    const init = fetchMock.mock.calls[0]![1];
    expect(init.headers.Authorization).toBe('Bearer sec');
  });

  it('POST sends JSON content-type and a serialized body', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ success: true }));
    vi.stubGlobal('fetch', fetchMock);
    const conn = new ProxyBrowserConnection({ id: 'x', label: 'x', baseUrl: 'http://h:8080' });
    await conn.post('/stopall', { command: 'stopall', message: {} });
    const init = fetchMock.mock.calls[0]![1];
    expect(init.method).toBe('POST');
    expect(init.headers['Content-Type']).toBe('application/json');
    expect(JSON.parse(init.body)).toEqual({ command: 'stopall', message: {} });
  });

  it('throws HttpError with the status on a non-2xx response', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ error: 'nope' }, { status: 401 }));
    vi.stubGlobal('fetch', fetchMock);
    const conn = new ProxyBrowserConnection({ id: 'x', label: 'x', baseUrl: 'http://h:8080' });
    await expect(conn.get('/version')).rejects.toMatchObject({ status: 401 });
    await expect(conn.get('/version')).rejects.toBeInstanceOf(HttpError);
  });

  it('subscribe opens a WebSocket, forwards parsed messages, and closes', () => {
    const instances: FakeWebSocket[] = [];
    class FakeWebSocket {
      onopen: (() => void) | null = null;
      onmessage: ((e: { data: unknown }) => void) | null = null;
      onclose: ((e: { code: number; reason: string }) => void) | null = null;
      onerror: ((e: unknown) => void) | null = null;
      closed = false;
      constructor(public url: string) {
        instances.push(this);
      }
      close() {
        this.closed = true;
      }
    }
    vi.stubGlobal('WebSocket', FakeWebSocket as unknown as typeof WebSocket);

    const conn = new ProxyBrowserConnection({ id: 'x', label: 'x', baseUrl: 'http://h:8080' });
    const messages: unknown[] = [];
    const sub = conn.subscribe('/ws', { onMessage: (d) => messages.push(d) });
    const ws = instances[0]!;
    expect(ws.url).toBe('ws://h:8080/ws');
    ws.onmessage?.({ data: JSON.stringify({ type: 'log', message: 'hi' }) });
    expect(messages).toEqual([{ type: 'log', message: 'hi' }]);
    sub.close();
    expect(ws.closed).toBe(true);
  });

  it('a relative (proxy) base URL resolves the socket same-origin against the page', () => {
    const urls: string[] = [];
    class FakeWS {
      constructor(public url: string) {
        urls.push(url);
      }
      close() {}
    }
    vi.stubGlobal('WebSocket', FakeWS as unknown as typeof WebSocket);
    // Proxy deployment (DW1): REST under /api, but the socket is same-origin /ws.
    const conn = new ProxyBrowserConnection({ id: 'x', label: 'x', baseUrl: '/api' });
    conn.subscribe('/ws', { onMessage: () => {} });
    expect(urls[0]).toMatch(/^wss?:\/\/[^/]+\/ws$/);
    expect(urls[0]).not.toContain('/api');
    expect(urls[0]).not.toContain('token=');
  });
});
