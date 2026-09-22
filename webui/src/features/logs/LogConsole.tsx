// Live log console over the daemon's /ws stream (Sprint W1, F3). Shows a
// connection indicator and the streamed tracing lines with autoscroll. These are
// LOGS, not playback/state events (Sprint W7 owns state events).

import { useEffect, useRef } from 'react';
import Box from '@mui/material/Box';
import Chip from '@mui/material/Chip';
import Paper from '@mui/material/Paper';
import Stack from '@mui/material/Stack';
import Typography from '@mui/material/Typography';
import { useClient } from '../../state/clientContext';
import type { ConnectionState } from '../../api/client';
import { useLogStream, type LogLine } from './useLogStream';

const INDICATOR: Record<ConnectionState, { label: string; color: 'success' | 'warning' | 'default' | 'error' }> = {
  live: { label: 'live', color: 'success' },
  connecting: { label: 'connecting…', color: 'warning' },
  offline: { label: 'offline — reconnecting', color: 'default' },
  unauthorized: { label: 'unauthorized', color: 'error' },
};

function lineColor(kind: LogLine['kind']): string {
  if (kind === 'gap') return 'warning.main';
  if (kind === 'system') return 'info.main';
  return 'text.primary';
}

export function LogConsole() {
  const client = useClient();
  const { state, version, lines } = useLogStream(client);
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = scrollRef.current;
    if (el) {
      el.scrollTop = el.scrollHeight;
    }
  }, [lines]);

  const indicator = INDICATOR[state];

  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Stack direction="row" spacing={1} alignItems="center" mb={1}>
        <Typography variant="subtitle1" sx={{ flexGrow: 1 }}>
          Log stream
        </Typography>
        {version && <Chip size="small" variant="outlined" label={`daemon v${version}`} />}
        <Chip size="small" color={indicator.color} label={indicator.label} aria-label={`stream ${state}`} />
      </Stack>
      <Box
        ref={scrollRef}
        role="log"
        aria-label="log stream"
        sx={{
          height: 320,
          overflowY: 'auto',
          fontFamily: 'monospace',
          fontSize: 13,
          bgcolor: 'background.default',
          p: 1,
          borderRadius: 1,
        }}
      >
        {lines.length === 0 ? (
          <Typography variant="body2" color="text.secondary">
            {state === 'unauthorized'
              ? 'Not authorized to read the log stream.'
              : 'Waiting for log output…'}
          </Typography>
        ) : (
          lines.map((line) => (
            <Box
              key={line.id}
              component="pre"
              sx={{ m: 0, whiteSpace: 'pre-wrap', color: lineColor(line.kind) }}
            >
              {line.text}
            </Box>
          ))
        )}
      </Box>
    </Paper>
  );
}
