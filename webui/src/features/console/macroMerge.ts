// Macro precedence preview (API-CONTRACT §1): the daemon resolves a play's
// effective params as command-params > macro[0] > macro[1] > ... > macro[n]
// (earlier macros win; explicit command params win over all). The actual macro
// definitions come from the daemon config (GET /config, Sprint W8); this pure
// helper reproduces the precedence so the cue launcher can preview the merge.

export type Params = Record<string, unknown>;

export function mergeMacros(
  commandParams: Params,
  macros: string[],
  defs: Record<string, Params>,
): Params {
  const out: Params = {};
  // Apply macros in reverse so an earlier macro overwrites a later one.
  for (let i = macros.length - 1; i >= 0; i--) {
    const name = macros[i];
    const def = name ? defs[name] : undefined;
    if (def) {
      for (const key of Object.keys(def)) {
        out[key] = def[key];
      }
    }
  }
  // Explicit command params win over every macro.
  for (const key of Object.keys(commandParams)) {
    if (commandParams[key] !== undefined) {
      out[key] = commandParams[key];
    }
  }
  return out;
}

/** Parse a comma/space-separated macro list into names (dropping blanks). */
export function parseMacroList(input: string): string[] {
  return input
    .split(/[,\s]+/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}
