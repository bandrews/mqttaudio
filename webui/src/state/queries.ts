// TanStack Query hooks over the typed client, carrying the DW6 poll cadences as
// defaults: /metrics + /status* at 1-2s, /status/cache at 2-5s, /version once.
// Sprint W2 (the dashboard) consumes these; this sprint only ships the seam, so
// the cadences are asserted via the exported option factories.

import { useQuery, type UseQueryOptions } from '@tanstack/react-query';
import type { DaemonClient } from '../api/client';
import { useClient } from './clientContext';

/** DW6 poll cadences (ms). */
export const POLL = {
  /** /metrics and /status* : the 1-2s band. */
  status: 1500,
  /** /status/cache : the 2-5s band. */
  cache: 3000,
  /** /health connection-loss probe : slow. */
  health: 10_000,
} as const;

type Options<T> = UseQueryOptions<T, Error, T, string[]>;

export function versionQueryOptions(client: DaemonClient): Options<Awaited<ReturnType<DaemonClient['version']>>> {
  return {
    queryKey: ['version'],
    queryFn: () => client.version(),
    staleTime: Infinity,
    refetchInterval: false,
  };
}

export function healthQueryOptions(client: DaemonClient): Options<Awaited<ReturnType<DaemonClient['health']>>> {
  return { queryKey: ['health'], queryFn: () => client.health(), refetchInterval: POLL.health };
}

export function statusQueryOptions(client: DaemonClient): Options<Awaited<ReturnType<DaemonClient['status']>>> {
  return { queryKey: ['status'], queryFn: () => client.status(), refetchInterval: POLL.status };
}

export function metricsQueryOptions(client: DaemonClient): Options<Awaited<ReturnType<DaemonClient['metrics']>>> {
  return { queryKey: ['metrics'], queryFn: () => client.metrics(), refetchInterval: POLL.status };
}

export function samplesQueryOptions(client: DaemonClient): Options<Awaited<ReturnType<DaemonClient['statusSamples']>>> {
  return { queryKey: ['status', 'samples'], queryFn: () => client.statusSamples(), refetchInterval: POLL.status };
}

export function voicesQueryOptions(client: DaemonClient): Options<Awaited<ReturnType<DaemonClient['statusVoices']>>> {
  return { queryKey: ['status', 'voices'], queryFn: () => client.statusVoices(), refetchInterval: POLL.status };
}

export function inputsQueryOptions(client: DaemonClient): Options<Awaited<ReturnType<DaemonClient['statusInputs']>>> {
  return { queryKey: ['status', 'inputs'], queryFn: () => client.statusInputs(), refetchInterval: POLL.status };
}

export function cacheQueryOptions(client: DaemonClient): Options<Awaited<ReturnType<DaemonClient['statusCache']>>> {
  return { queryKey: ['status', 'cache'], queryFn: () => client.statusCache(), refetchInterval: POLL.cache };
}

function useClientQuery<T>(factory: (client: DaemonClient) => Options<T>) {
  const client = useClient();
  // The factory needs a non-null client; when there is none, disable the query
  // (queryFn never runs) and feed a stable placeholder factory.
  const options = client
    ? factory(client)
    : ({ queryKey: ['disabled'], queryFn: () => Promise.reject(new Error('no client')) } as Options<T>);
  return useQuery({ ...options, enabled: !!client });
}

export const useVersion = () => useClientQuery(versionQueryOptions);
export const useHealth = () => useClientQuery(healthQueryOptions);
export const useStatus = () => useClientQuery(statusQueryOptions);
export const useMetrics = () => useClientQuery(metricsQueryOptions);
export const useStatusSamples = () => useClientQuery(samplesQueryOptions);
export const useStatusVoices = () => useClientQuery(voicesQueryOptions);
export const useStatusInputs = () => useClientQuery(inputsQueryOptions);
export const useStatusCache = () => useClientQuery(cacheQueryOptions);
