// Raw /command editor (Sprint W3, F3): send arbitrary command JSON to POST
// /command — the universal path that reaches the full play surface
// (channel_map/mode/window_ms/prebuffer_ms/freshness/cacheable) which the typed
// /play endpoint silently drops (DW10). Validates that the body is JSON.

import { useState } from 'react';
import Alert from '@mui/material/Alert';
import Box from '@mui/material/Box';
import Button from '@mui/material/Button';
import Paper from '@mui/material/Paper';
import Stack from '@mui/material/Stack';
import TextField from '@mui/material/TextField';
import Typography from '@mui/material/Typography';
import { useCommandRunner } from './useCommandRunner';

const TEMPLATE = JSON.stringify(
  {
    command: 'play',
    message: {
      file: '/sounds/song.wav',
      voice: 'music',
      channel_map: [{ src: 0, dest: 4, gain: 0.5 }],
      mode: 'stream',
      freshness: 'pinned',
    },
  },
  null,
  2,
);

export function RawCommandEditor() {
  const { state, run } = useCommandRunner();
  const [text, setText] = useState(TEMPLATE);
  const [jsonError, setJsonError] = useState<string | null>(null);

  function send() {
    let payload: Record<string, unknown>;
    try {
      payload = JSON.parse(text);
    } catch (err) {
      setJsonError(err instanceof Error ? err.message : 'Invalid JSON');
      return;
    }
    setJsonError(null);
    void run((c) => c.rawCommand(payload));
  }

  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Typography variant="subtitle1" gutterBottom>
        Raw command (POST /command)
      </Typography>
      <Typography variant="caption" color="text.secondary">
        The full play surface (channel_map, mode, window_ms, prebuffer_ms, freshness, cacheable) is only
        reachable here or via MQTT — the typed /play endpoint drops these fields.
      </Typography>
      <Box mt={1}>
        <TextField
          multiline
          minRows={8}
          fullWidth
          value={text}
          onChange={(e) => setText(e.target.value)}
          inputProps={{ 'aria-label': 'raw command JSON', style: { fontFamily: 'monospace', fontSize: 13 } }}
        />
      </Box>
      <Stack direction="row" spacing={2} alignItems="center" mt={1}>
        <Button variant="contained" disabled={state.status === 'sending'} onClick={send}>
          Send
        </Button>
        {jsonError && <Alert severity="error" sx={{ py: 0 }} aria-label="json error">{jsonError}</Alert>}
        {state.status === 'ok' && <Alert severity="success" sx={{ py: 0 }}>Accepted (enqueued).</Alert>}
        {state.status === 'error' && !jsonError && <Alert severity="error" sx={{ py: 0 }}>{state.message}</Alert>}
      </Stack>
    </Paper>
  );
}
