// Root app shell. Sprint W0 ships only the connect/bootstrap surface: connect to
// a daemon, then show its identity. Feature UI (dashboard, command bench, matrix
// mixer, transport, telemetry, config) is added by Sprints W2+ inside the
// ConnectedView's body.

import { useState } from 'react';
import AppBar from '@mui/material/AppBar';
import Box from '@mui/material/Box';
import Button from '@mui/material/Button';
import Chip from '@mui/material/Chip';
import Container from '@mui/material/Container';
import Stack from '@mui/material/Stack';
import Tab from '@mui/material/Tab';
import Tabs from '@mui/material/Tabs';
import Toolbar from '@mui/material/Toolbar';
import Typography from '@mui/material/Typography';
import { DaemonClient } from './api/client';
import type { Connection } from './api/connection';
import type { BootstrapResult } from './api/bootstrap';
import { ClientProvider } from './state/QueryProvider';
import { Connect } from './components/Connect';
import { Dashboard } from './features/dashboard/Dashboard';
import { CommandConsole } from './features/console/CommandConsole';
import { MatrixMixer } from './features/matrix/MatrixMixer';

interface Session {
  client: DaemonClient;
  result: BootstrapResult;
  connection: Connection;
}

export function App() {
  const [session, setSession] = useState<Session | null>(null);

  if (!session) {
    return (
      <Connect
        onConnected={(client, result, connection) => setSession({ client, result, connection })}
      />
    );
  }

  return (
    <ClientProvider client={session.client}>
      <ConnectedView session={session} onDisconnect={() => setSession(null)} />
    </ClientProvider>
  );
}

type View = 'monitor' | 'console' | 'matrix';

function ConnectedView({ session, onDisconnect }: { session: Session; onDisconnect: () => void }) {
  const { result, connection } = session;
  const version = result.version;
  const [view, setView] = useState<View>('monitor');
  return (
    <Box>
      <AppBar position="static" color="default" elevation={1}>
        <Toolbar>
          <Typography variant="h6" sx={{ flexGrow: 1 }}>
            mqttaudio
          </Typography>
          <Stack direction="row" spacing={1} alignItems="center">
            <Chip size="small" color="success" label={`connected: ${connection.label}`} />
            {version && (
              <Chip
                size="small"
                variant="outlined"
                label={`v${version.version}${version.git_sha ? ` (${version.git_sha})` : ''}`}
              />
            )}
            <Button size="small" onClick={onDisconnect}>
              Disconnect
            </Button>
          </Stack>
        </Toolbar>
        <Tabs value={view} onChange={(_e, v: View) => setView(v)} sx={{ px: 2 }}>
          <Tab value="monitor" label="Monitor" />
          <Tab value="console" label="Console" />
          <Tab value="matrix" label="Matrix" />
        </Tabs>
      </AppBar>
      <Container maxWidth="xl" sx={{ py: 3 }}>
        {view === 'monitor' && <Dashboard />}
        {view === 'console' && <CommandConsole />}
        {view === 'matrix' && <MatrixMixer />}
      </Container>
    </Box>
  );
}
