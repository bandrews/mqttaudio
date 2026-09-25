// ABOUTME: Connection bootstrap: checks /health for liveness, probes /ws for auth, reads /version.
// ABOUTME: A 401 on /ws (or /version if /ws cannot tell) means a token is required; else errors.

// Connection bootstrap + auth detection (DW9). Reads /health (always open) for
// liveness, then asks whether the daemon refuses this connection's credentials.
// With http.auth_token set, the command routes and WebSockets need the token,
// but the status routes (/version among them) only when require_auth is also
// set — so the probe is a GET /ws without upgrade headers, which the daemon
// answers with no side effects: 401 when the token is missing or wrong,
// otherwise a refusal of the missing upgrade. When /ws cannot tell (404:
// websockets disabled), a 401 on /version (require_auth) is the fallback.
// /version is read either way for the daemon's identity.

import { HttpError, type DaemonConnection } from './connection';
import type { HealthInfo, VersionInfo } from './contract';

export interface BootstrapResult {
  healthy: boolean;
  /** The daemon refused this connection's credentials (none, or a wrong token) with a 401. */
  authRequired: boolean;
  service?: string;
  version?: VersionInfo;
  error?: string;
}

/**
 * The statuses the daemon's WebSocket handler gives a GET without upgrade
 * headers. It only reaches that handler once the token check has passed.
 */
const UPGRADE_REFUSED = new Set([400, 405, 426]);

function errMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/**
 * Whether the daemon refuses this connection's credentials, from a GET /ws
 * without upgrade headers: true on a 401, false when only the upgrade is
 * refused. Undefined when /ws cannot tell: 404 (websockets disabled) or any
 * other answer, such as a network failure or a proxy error.
 */
export async function probeTokenRefused(conn: DaemonConnection): Promise<boolean | undefined> {
  try {
    await conn.get('/ws');
  } catch (err) {
    if (err instanceof HttpError && err.status === 401) {
      return true;
    }
    if (err instanceof HttpError && UPGRADE_REFUSED.has(err.status)) {
      return false;
    }
  }
  return undefined;
}

export async function bootstrap(conn: DaemonConnection): Promise<BootstrapResult> {
  let service: string | undefined;
  try {
    const health = await conn.get<HealthInfo>('/health');
    service = health.service;
  } catch (err) {
    return { healthy: false, authRequired: false, error: errMessage(err) };
  }

  const tokenRefused = await probeTokenRefused(conn);
  try {
    const version = await conn.get<VersionInfo>('/version');
    return { healthy: true, authRequired: tokenRefused === true, service, version };
  } catch (err) {
    if (err instanceof HttpError && err.status === 401) {
      return { healthy: true, authRequired: true, service };
    }
    return { healthy: true, authRequired: tokenRefused === true, service, error: errMessage(err) };
  }
}
