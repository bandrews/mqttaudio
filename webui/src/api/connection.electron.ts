// Reserved Electron transport seam (DW2/DW13). v1 does not implement it; this
// stub exists only to prove the seam type-checks: an Electron build supplies a
// real implementation here (native header-capable WebSocket, multi-instance
// discovery) with no change to any UI module. It is one of the two modules
// allowed to use `fetch`/`WebSocket`, though it uses neither yet.

import {
  type Connection,
  type DaemonConnection,
  type Subscription,
  type SubscriptionHandlers,
} from './connection';

export class ElectronConnection implements DaemonConnection {
  constructor(public readonly connection: Connection) {}

  get<T>(_path: string): Promise<T> {
    throw new Error('Electron connection not implemented in v1');
  }

  post<T>(_path: string, _body?: unknown): Promise<T> {
    throw new Error('Electron connection not implemented in v1');
  }

  subscribe(_path: string, _handlers: SubscriptionHandlers): Subscription {
    throw new Error('Electron connection not implemented in v1');
  }
}
