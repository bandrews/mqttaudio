// Top-level providers: color mode (light/dark), MUI theme, and the query layer.
// Kept separate from main.tsx (the entry point) and App (the shell) so each is a
// clean fast-refresh boundary.

import { useMemo, useState } from 'react';
import CssBaseline from '@mui/material/CssBaseline';
import { ThemeProvider, createTheme } from '@mui/material/styles';
import { QueryProvider } from './state/QueryProvider';
import { ColorModeContext, type ColorMode } from './state/colorMode';
import { App } from './App';

export function AppRoot() {
  const [mode, setMode] = useState<ColorMode>('dark');
  const theme = useMemo(() => createTheme({ palette: { mode } }), [mode]);
  const colorMode = useMemo(
    () => ({ mode, toggle: () => setMode((m) => (m === 'dark' ? 'light' : 'dark')) }),
    [mode],
  );

  return (
    <ThemeProvider theme={theme}>
      <CssBaseline />
      <ColorModeContext.Provider value={colorMode}>
        <QueryProvider>
          <App />
        </QueryProvider>
      </ColorModeContext.Provider>
    </ThemeProvider>
  );
}
