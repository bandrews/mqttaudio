// Connection bootstrap + auth detection (DW9). Reads /health (always open) for
// liveness, then probes a gated endpoint (/version, in the status group that
// require_auth gates) to decide whether a token is needed: a 200 means open, a
// 401 means require_auth.

import { HttpError, type DaemonConnection } from './connection';
import type { HealthInfo, VersionInfo } from './contract';

export interface BootstrapResult {
  healthy: boolean;
  authRequired: boolean;
  service?: string;
  version?: VersionInfo;
  error?: string;
}

function errMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export async function bootstrap(conn: DaemonConnection): Promise<BootstrapResult> {
  let service: string | undefined;
  try {
    const health = await conn.get<HealthInfo>('/health');
    service = health.service;
  } catch (err) {
    return { healthy: false, authRequired: false, error: errMessage(err) };
  }

  try {
    const version = await conn.get<VersionInfo>('/version');
    return { healthy: true, authRequired: false, service, version };
  } catch (err) {
    if (err instanceof HttpError && err.status === 401) {
      return { healthy: true, authRequired: true, service };
    }
    return { healthy: true, authRequired: false, service, error: errMessage(err) };
  }
}
