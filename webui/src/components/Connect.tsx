// The connect/bootstrap surface (Sprint W0, DW9). Takes a base URL and an
// optional Bearer token, runs bootstrap (reads /health + /version, detects open
// vs require_auth), and reveals the token field only when the daemon requires
// auth. On success it hands the caller a connected DaemonClient. It renders no
// feature UI.

import { useState } from 'react';
import Alert from '@mui/material/Alert';
import Box from '@mui/material/Box';
import Button from '@mui/material/Button';
import CircularProgress from '@mui/material/CircularProgress';
import Paper from '@mui/material/Paper';
import Stack from '@mui/material/Stack';
import TextField from '@mui/material/TextField';
import Typography from '@mui/material/Typography';
import { DaemonClient } from '../api/client';
import type { Connection, DaemonConnection } from '../api/connection';
import { ProxyBrowserConnection } from '../api/connection.browser';
import { bootstrap, type BootstrapResult } from '../api/bootstrap';

export interface ConnectProps {
  onConnected: (client: DaemonClient, result: BootstrapResult, connection: Connection) => void;
  /** Override the transport factory (tests inject a mock connection). */
  makeTransport?: (connection: Connection) => DaemonConnection;
}

function defaultMakeTransport(connection: Connection): DaemonConnection {
  return new ProxyBrowserConnection(connection);
}

export function Connect({ onConnected, makeTransport = defaultMakeTransport }: ConnectProps) {
  const [baseUrl, setBaseUrl] = useState('http://localhost:8080');
  const [token, setToken] = useState('');
  const [authRequired, setAuthRequired] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(event: React.FormEvent) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    const connection: Connection = {
      id: baseUrl,
      label: baseUrl,
      baseUrl,
      token: token.trim() ? token.trim() : undefined,
    };
    const transport = makeTransport(connection);
    try {
      const result = await bootstrap(transport);
      if (!result.healthy) {
        setError(result.error ?? 'Daemon is not reachable.');
        return;
      }
      if (result.authRequired && !connection.token) {
        setAuthRequired(true);
        setError('This daemon requires authentication. Enter a Bearer token.');
        return;
      }
      if (result.authRequired && !result.version) {
        setError('Authentication failed — check the token.');
        return;
      }
      onConnected(new DaemonClient(transport), result, connection);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Box display="flex" justifyContent="center" pt={8}>
      <Paper sx={{ p: 4, width: 420 }} component="form" onSubmit={handleSubmit}>
        <Stack spacing={2}>
          <Typography variant="h5">Connect to mqttaudio</Typography>
          <TextField
            label="Daemon URL"
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
            fullWidth
            autoFocus
            inputProps={{ 'aria-label': 'Daemon URL' }}
          />
          {authRequired && (
            <TextField
              label="Bearer token"
              type="password"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              fullWidth
              inputProps={{ 'aria-label': 'Bearer token' }}
            />
          )}
          {error && <Alert severity={authRequired && !token ? 'info' : 'error'}>{error}</Alert>}
          <Button type="submit" variant="contained" disabled={submitting}>
            {submitting ? <CircularProgress size={22} /> : 'Connect'}
          </Button>
        </Stack>
      </Paper>
    </Box>
  );
}
