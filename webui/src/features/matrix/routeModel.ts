// Pure model for the channel-map matrix (Sprint W4). A Route is one src->dest
// edge with an optional gain. Mixing on the daemon is additive summing, so a dest
// fed by >1 src is a clip risk (API-CONTRACT §4). buildChannelMap emits the
// play.channel_map array (numeric dest by default — always valid; an unknown
// alias would silently abort the play, so alias-as-dest is left to the raw editor
// / config).

import type { ChannelMapping } from '../../api/contract';

export interface Route {
  src: number;
  dest: number;
  gain?: number;
}

export function hasRoute(routes: Route[], src: number, dest: number): boolean {
  return routes.some((r) => r.src === src && r.dest === dest);
}

export function toggleRoute(routes: Route[], src: number, dest: number): Route[] {
  if (hasRoute(routes, src, dest)) {
    return routes.filter((r) => !(r.src === src && r.dest === dest));
  }
  return [...routes, { src, dest }];
}

export function setGain(routes: Route[], src: number, dest: number, gain: number | undefined): Route[] {
  return routes.map((r) => (r.src === src && r.dest === dest ? { ...r, gain } : r));
}

export function getRoute(routes: Route[], src: number, dest: number): Route | undefined {
  return routes.find((r) => r.src === src && r.dest === dest);
}

/** Destinations fed by more than one source (additive sum → clip risk). */
export function summedDests(routes: Route[]): Set<number> {
  const counts = new Map<number, number>();
  for (const r of routes) counts.set(r.dest, (counts.get(r.dest) ?? 0) + 1);
  return new Set([...counts.entries()].filter(([, c]) => c > 1).map(([d]) => d));
}

/** The play.channel_map array. A gain of exactly 1 is omitted (unity). */
export function buildChannelMap(routes: Route[]): ChannelMapping[] {
  return [...routes]
    .sort((a, b) => a.src - b.src || a.dest - b.dest)
    .map((r) => {
      const m: ChannelMapping = { src: r.src, dest: r.dest };
      if (r.gain !== undefined && r.gain !== 1) m.gain = r.gain;
      return m;
    });
}
