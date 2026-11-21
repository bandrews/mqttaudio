// The single transport seam all daemon access goes through (DW2). A `Connection`
// is a first-class entity (base URL + optional token); a `DaemonConnection` is a
// live transport built from one. No React/UI module may import `fetch`/`WebSocket`
// directly — only the connection implementations (connection.browser.ts /
// connection.electron.ts) do, so an Electron repackage is a connection swap, not a
// UI rewrite.

/** A connectable daemon: base URL + optional in-memory Bearer token (DW9). */
export interface Connection {
  id: string;
  label: string;
  baseUrl: string;
  /**
   * Optional Bearer token. When set, a *direct* (non-proxied) connection attaches
   * it as an Authorization header (and, for WebSockets which cannot carry a
   * header, as `?token=`). Behind the reverse-proxy sidecar (DW1) the token is
   * injected server-side, so the Connection carries no token and the SPA never
   * puts it in a URL.
   */
  token?: string;
}

/** Raised by transport implementations on a non-2xx HTTP response. */
export class HttpError extends Error {
  constructor(
    public readonly status: number,
    public readonly path: string,
    message?: string,
  ) {
    super(message ?? `HTTP ${status} for ${path}`);
    this.name = 'HttpError';
  }
}

export interface SubscriptionHandlers {
  onMessage: (data: unknown) => void;
  onOpen?: () => void;
  onClose?: (event: { code?: number; reason?: string }) => void;
  onError?: (error: unknown) => void;
}

export interface Subscription {
  close(): void;
}

/** The transport contract every feature calls through. */
export interface DaemonConnection {
  readonly connection: Connection;
  get<T>(path: string): Promise<T>;
  post<T>(path: string, body?: unknown): Promise<T>;
  subscribe(path: string, handlers: SubscriptionHandlers): Subscription;
}

/**
 * Holds the set of known connections (multi-instance-ready, DW2). v1 wires a
 * single connection; the registry exists so nothing assumes a singleton.
 */
export class ConnectionRegistry {
  private readonly connections = new Map<string, Connection>();

  add(connection: Connection): Connection {
    this.connections.set(connection.id, connection);
    return connection;
  }

  get(id: string): Connection | undefined {
    return this.connections.get(id);
  }

  remove(id: string): boolean {
    return this.connections.delete(id);
  }

  list(): Connection[] {
    return [...this.connections.values()];
  }

  get size(): number {
    return this.connections.size;
  }
}

/** Join a base URL and a path without doubling or dropping the slash. */
export function joinUrl(baseUrl: string, path: string): string {
  const base = baseUrl.replace(/\/+$/, '');
  const suffix = path.startsWith('/') ? path : `/${path}`;
  return `${base}${suffix}`;
}

/** Derive the `ws(s)://` origin for a daemon's HTTP base URL. */
export function toWebSocketUrl(baseUrl: string, path: string, token?: string): string {
  const httpUrl = joinUrl(baseUrl, path);
  const wsUrl = httpUrl.replace(/^http(s?):\/\//i, (_m, s: string) => `ws${s}://`);
  // A browser WebSocket cannot send an Authorization header; a direct connection
  // with a token must pass it as ?token= (DW9). Behind the proxy there is no token.
  if (token) {
    const sep = wsUrl.includes('?') ? '&' : '?';
    return `${wsUrl}${sep}token=${encodeURIComponent(token)}`;
  }
  return wsUrl;
}
