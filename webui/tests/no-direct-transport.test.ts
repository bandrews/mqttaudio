import { describe, it, expect } from 'vitest';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

// DW2 Electron seam: no module under src/ may touch fetch/WebSocket directly,
// except the two connection implementations. This belt-and-braces test complements
// the eslint no-restricted-globals rule. Vitest runs from the webui package root.
const SRC = join(process.cwd(), 'src');
const ALLOWED = new Set(['connection.browser.ts', 'connection.electron.ts']);

function walk(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) out.push(...walk(full));
    else if (/\.(ts|tsx)$/.test(name)) out.push(full);
  }
  return out;
}

describe('no direct fetch/WebSocket outside the connection layer (DW2)', () => {
  it('finds zero stray transport calls in src/', () => {
    const offenders: string[] = [];
    for (const file of walk(SRC)) {
      if (ALLOWED.has(file.split('/').pop()!)) continue;
      const text = readFileSync(file, 'utf8');
      if (/\bfetch\s*\(/.test(text) || /\bnew\s+WebSocket\s*\(/.test(text)) {
        offenders.push(file);
      }
    }
    expect(offenders).toEqual([]);
  });
});
