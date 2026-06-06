// Channel-map matrix mixer (Sprint W4): a src×dest grid that builds a
// play.channel_map and fires it via /command (DW10 — the typed /play drops
// channel_map). Additive summing means a destination fed by >1 source is a
// clip-risk (badged). Emits numeric destinations (always valid); an unknown alias
// would silently abort the play (API-CONTRACT §4), so alias-as-dest is left to the
// raw editor + config.

import { useMemo, useState } from 'react';
import Alert from '@mui/material/Alert';
import Box from '@mui/material/Box';
import Button from '@mui/material/Button';
import Checkbox from '@mui/material/Checkbox';
import Chip from '@mui/material/Chip';
import Paper from '@mui/material/Paper';
import Stack from '@mui/material/Stack';
import Table from '@mui/material/Table';
import TableBody from '@mui/material/TableBody';
import TableCell from '@mui/material/TableCell';
import TableHead from '@mui/material/TableHead';
import TableRow from '@mui/material/TableRow';
import TextField from '@mui/material/TextField';
import Typography from '@mui/material/Typography';
import { useStatus } from '../../state/queries';
import { useCommandRunner } from '../console/useCommandRunner';
import {
  buildChannelMap,
  getRoute,
  hasRoute,
  setGain,
  summedDests,
  toggleRoute,
  type Route,
} from './routeModel';

function range(n: number): number[] {
  return Array.from({ length: Math.max(0, n) }, (_, i) => i);
}

export function MatrixMixer() {
  const { data: status } = useStatus();
  const { state, run } = useCommandRunner();
  const destCount = status?.output_channels && status.output_channels > 0 ? status.output_channels : 8;

  const [srcCount, setSrcCount] = useState(2);
  const [file, setFile] = useState('');
  const [voice, setVoice] = useState('');
  const [labels, setLabels] = useState('');
  const [routes, setRoutes] = useState<Route[]>([]);

  const labelList = labels.split(',').map((s) => s.trim());
  const destLabel = (d: number) => (labelList[d] ? labelList[d] : `ch ${d}`);
  const clipDests = useMemo(() => summedDests(routes), [routes]);
  const channelMap = useMemo(() => buildChannelMap(routes), [routes]);

  const message = useMemo(() => {
    const m: Record<string, unknown> = { file: file.trim() };
    if (voice.trim()) m.voice = voice.trim();
    m.channel_map = channelMap;
    return m;
  }, [file, voice, channelMap]);

  const toggle = (src: number, dest: number) => setRoutes((r) => toggleRoute(r, src, dest));
  const changeGain = (src: number, dest: number, raw: string) => {
    const g = raw.trim() === '' ? undefined : Number(raw);
    setRoutes((r) => setGain(r, src, dest, Number.isFinite(g as number) ? g : undefined));
  };

  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Typography variant="subtitle1" gutterBottom>
        Channel-map matrix mixer
      </Typography>
      <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1} mb={1}>
        <TextField size="small" label="file" value={file} onChange={(e) => setFile(e.target.value)} inputProps={{ 'aria-label': 'matrix file' }} sx={{ flexGrow: 1 }} />
        <TextField size="small" label="voice" value={voice} onChange={(e) => setVoice(e.target.value)} />
        <TextField size="small" label="source channels" type="number" value={srcCount} onChange={(e) => setSrcCount(Math.max(1, Math.min(64, Number(e.target.value) || 1)))} sx={{ width: 150 }} />
      </Stack>
      <TextField size="small" fullWidth label="destination labels (comma-separated, optional)" value={labels} onChange={(e) => setLabels(e.target.value)} sx={{ mb: 1 }} helperText={`${destCount} output channels detected; labels are display-only (numeric destinations are sent)`} />

      <Box sx={{ overflowX: 'auto' }}>
        <Table size="small" aria-label="channel routing matrix">
          <TableHead>
            <TableRow>
              <TableCell>src \ dest</TableCell>
              {range(destCount).map((d) => (
                <TableCell key={d} align="center">
                  <Stack alignItems="center" spacing={0.5}>
                    <Typography variant="caption">{destLabel(d)}</Typography>
                    {clipDests.has(d) && <Chip size="small" color="warning" label="sum" aria-label={`dest ${d} clip risk`} />}
                  </Stack>
                </TableCell>
              ))}
            </TableRow>
          </TableHead>
          <TableBody>
            {range(srcCount).map((s) => (
              <TableRow key={s}>
                <TableCell>src {s}</TableCell>
                {range(destCount).map((d) => {
                  const on = hasRoute(routes, s, d);
                  const route = getRoute(routes, s, d);
                  return (
                    <TableCell key={d} align="center" sx={{ p: 0.5 }}>
                      <Checkbox size="small" checked={on} onChange={() => toggle(s, d)} inputProps={{ 'aria-label': `route ${s} to ${d}` }} />
                      {on && (
                        <TextField
                          size="small"
                          variant="standard"
                          placeholder="gain"
                          value={route?.gain ?? ''}
                          onChange={(e) => changeGain(s, d, e.target.value)}
                          inputProps={{ 'aria-label': `gain ${s} to ${d}`, style: { width: 44, fontSize: 12 } }}
                        />
                      )}
                    </TableCell>
                  );
                })}
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </Box>

      <Stack spacing={1} mt={1}>
        {clipDests.size > 0 && (
          <Alert severity="warning">
            Destinations summed from multiple sources can clip ({[...clipDests].map(destLabel).join(', ')}). Use
            per-route gain to attenuate.
          </Alert>
        )}
        <Alert severity="info">
          Per-route gain is ignored on windowed/streamed plays (mode=stream or an auto-windowed large file). An
          unknown channel alias silently aborts the play; numeric destinations always apply, and out-of-range
          destinations are skipped.
        </Alert>
        <Box component="pre" sx={{ m: 0, p: 1, bgcolor: 'background.default', borderRadius: 1, fontSize: 12, overflowX: 'auto' }} aria-label="matrix JSON preview">
          {JSON.stringify({ command: 'play', message }, null, 2)}
        </Box>
        <Stack direction="row" spacing={2} alignItems="center">
          <Button variant="contained" disabled={!file.trim() || channelMap.length === 0 || state.status === 'sending'} onClick={() => run((c) => c.rawCommand({ command: 'play', message }))}>
            Play routed
          </Button>
          {channelMap.length === 0 && <Typography variant="caption" color="text.secondary">Select at least one route.</Typography>}
          {state.status === 'ok' && <Alert severity="success" sx={{ py: 0 }}>Accepted (enqueued).</Alert>}
          {state.status === 'error' && <Alert severity="error" sx={{ py: 0 }}>{state.message}</Alert>}
        </Stack>
      </Stack>
    </Paper>
  );
}
