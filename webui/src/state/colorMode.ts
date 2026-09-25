// ABOUTME: React context for the light/dark color mode and its toggle.
// ABOUTME: useColorMode reads it; AppRoot provides the value.

// Light/dark color-mode context (Sprint W9). Kept in its own module so the
// provider component (AppRoot.tsx) and the toggle stay fast-refresh-friendly.

import { createContext, useContext } from 'react';

export type ColorMode = 'light' | 'dark';

export interface ColorModeContextValue {
  mode: ColorMode;
  toggle: () => void;
}

export const ColorModeContext = createContext<ColorModeContextValue>({
  mode: 'dark',
  toggle: () => {},
});

export function useColorMode(): ColorModeContextValue {
  return useContext(ColorModeContext);
}
