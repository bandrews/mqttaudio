import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import CssBaseline from '@mui/material/CssBaseline';
import { ThemeProvider, createTheme } from '@mui/material/styles';
import { QueryProvider } from './state/QueryProvider';
import { App } from './App';

const theme = createTheme({ palette: { mode: 'dark' } });

const rootEl = document.getElementById('root');
if (!rootEl) {
  throw new Error('Root element #root not found');
}

createRoot(rootEl).render(
  <StrictMode>
    <ThemeProvider theme={theme}>
      <CssBaseline />
      <QueryProvider>
        <App />
      </QueryProvider>
    </ThemeProvider>
  </StrictMode>,
);
