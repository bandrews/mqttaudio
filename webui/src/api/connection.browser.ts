// Browser/proxy implementation of DaemonConnection. This is one of only two
// modules allowed to touch `fetch`/`WebSocket` directly (DW2). When deployed
// behind the reverse-proxy sidecar (DW1) the Connection carries no token and the
// proxy injects auth; a direct Connection with a token attaches the Bearer header
// (and `?token=` for the WebSocket, which cannot carry a header — DW9).

import {
  type Connection,
  type DaemonConnection,
  type Subscription,
  type SubscriptionHandlers,
  HttpError,
  joinUrl,
  toWebSocketUrl,
} from './connection';

export class ProxyBrowserConnection implements DaemonConnection {
  constructor(public readonly connection: Connection) {}

  private headers(extra?: Record<string, string>): Record<string, string> {
    const headers: Record<string, string> = { Accept: 'application/json', ...extra };
    if (this.connection.token) {
      headers.Authorization = `Bearer ${this.connection.token}`;
    }
    return headers;
  }

  async get<T>(path: string): Promise<T> {
    const res = await fetch(joinUrl(this.connection.baseUrl, path), {
      method: 'GET',
      headers: this.headers(),
    });
    return this.parse<T>(res, path);
  }

  async post<T>(path: string, body?: unknown): Promise<T> {
    const res = await fetch(joinUrl(this.connection.baseUrl, path), {
      method: 'POST',
      headers: this.headers({ 'Content-Type': 'application/json' }),
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    return this.parse<T>(res, path);
  }

  private async parse<T>(res: Response, path: string): Promise<T> {
    if (!res.ok) {
      let detail: string | undefined;
      try {
        detail = await res.text();
      } catch {
        detail = undefined;
      }
      throw new HttpError(res.status, path, detail || undefined);
    }
    // 204/empty bodies parse to undefined.
    const text = await res.text();
    return (text ? JSON.parse(text) : undefined) as T;
  }

  /**
   * Resolve the WebSocket URL. A direct (absolute) base URL builds ws(s):// from
   * the daemon host (with ?token= for a direct token, DW9). A relative base URL
   * means the proxy deployment (DW1): the socket is same-origin against the page,
   * independent of the REST /api prefix, and the proxy injects auth — no token in
   * the URL.
   */
  private wsUrl(path: string): string {
    const base = this.connection.baseUrl;
    if (/^https?:\/\//i.test(base)) {
      return toWebSocketUrl(base, path, this.connection.token);
    }
    const loc = window.location;
    const proto = loc.protocol === 'https:' ? 'wss' : 'ws';
    const suffix = path.startsWith('/') ? path : `/${path}`;
    let url = `${proto}://${loc.host}${suffix}`;
    if (this.connection.token) {
      url += `${url.includes('?') ? '&' : '?'}token=${encodeURIComponent(this.connection.token)}`;
    }
    return url;
  }

  subscribe(path: string, handlers: SubscriptionHandlers): Subscription {
    const url = this.wsUrl(path);
    const ws = new WebSocket(url);
    ws.onopen = () => handlers.onOpen?.();
    ws.onmessage = (event: MessageEvent) => {
      let data: unknown = event.data;
      if (typeof event.data === 'string') {
        try {
          data = JSON.parse(event.data);
        } catch {
          data = event.data;
        }
      }
      handlers.onMessage(data);
    };
    ws.onclose = (event: CloseEvent) => handlers.onClose?.({ code: event.code, reason: event.reason });
    ws.onerror = (event: Event) => handlers.onError?.(event);
    return {
      close: () => ws.close(),
    };
  }
}
