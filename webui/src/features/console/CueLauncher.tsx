// Cue launcher (Sprint W3, F4): compose a play from a file + options and fire it.
// Routed through client.play() -> POST /command, so the full surface (mode,
// freshness, window/prebuffer, cacheable) reaches the daemon (DW10). A live JSON
// preview shows exactly what will be sent. channel_map is the matrix mixer's job
// (Sprint W4); macro definitions come from config (Sprint W8) — listing macro
// names here adds the `macro` field, and command params win over macros.

import { useMemo, useState } from 'react';
import Alert from '@mui/material/Alert';
import Box from '@mui/material/Box';
import Button from '@mui/material/Button';
import FormControlLabel from '@mui/material/FormControlLabel';
import MenuItem from '@mui/material/MenuItem';
import Paper from '@mui/material/Paper';
import Stack from '@mui/material/Stack';
import Switch from '@mui/material/Switch';
import TextField from '@mui/material/TextField';
import Typography from '@mui/material/Typography';
import type { Freshness, LoadMode, PlayParams } from '../../api/contract';
import { validateVolume } from './validation';
import { parseMacroList } from './macroMerge';
import { useCommandRunner } from './useCommandRunner';

function numOrUndef(s: string): number | undefined {
  if (s.trim() === '') return undefined;
  const n = Number(s);
  return Number.isFinite(n) ? n : undefined;
}

export function CueLauncher() {
  const { state, run } = useCommandRunner();
  const [file, setFile] = useState('');
  const [id, setId] = useState('');
  const [voice, setVoice] = useState('');
  const [volume, setVolume] = useState('');
  const [loop, setLoop] = useState(false);
  const [crossfade, setCrossfade] = useState('');
  const [fadeIn, setFadeIn] = useState('');
  const [startPos, setStartPos] = useState('');
  const [mode, setMode] = useState<LoadMode | ''>('');
  const [windowMs, setWindowMs] = useState('');
  const [prebufferMs, setPrebufferMs] = useState('');
  const [freshness, setFreshness] = useState<Freshness | ''>('');
  const [cacheable, setCacheable] = useState(true);
  const [macros, setMacros] = useState('');

  const params = useMemo<PlayParams & { macro?: string[] }>(() => {
    const p: PlayParams & { macro?: string[] } = { file: file.trim() };
    if (id.trim()) p.id = id.trim();
    if (voice.trim()) p.voice = voice.trim();
    const v = numOrUndef(volume);
    if (v !== undefined) p.volume = v;
    if (loop) p.loop = true;
    const cf = numOrUndef(crossfade);
    if (cf !== undefined) p.crossfade_ms = cf;
    const fi = numOrUndef(fadeIn);
    if (fi !== undefined) p.fade_in = fi;
    const sp = numOrUndef(startPos);
    if (sp !== undefined) p.start_position_ms = sp;
    if (mode) p.mode = mode;
    const w = numOrUndef(windowMs);
    if (w !== undefined) p.window_ms = w;
    const pb = numOrUndef(prebufferMs);
    if (pb !== undefined) p.prebuffer_ms = pb;
    if (freshness) p.freshness = freshness;
    if (!cacheable) p.cacheable = false;
    const macroList = parseMacroList(macros);
    if (macroList.length > 0) p.macro = macroList;
    return p;
  }, [file, id, voice, volume, loop, crossfade, fadeIn, startPos, mode, windowMs, prebufferMs, freshness, cacheable, macros]);

  const volIssue = volume.trim() ? validateVolume(Number(volume)) : {};
  const preview = JSON.stringify({ command: 'play', message: params }, null, 2);
  const streamGainNote = mode === 'stream';

  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Typography variant="subtitle1" gutterBottom>
        Cue launcher (play)
      </Typography>
      <Stack spacing={1.5}>
        <TextField
          label="file (path or http/https URL)"
          required
          value={file}
          onChange={(e) => setFile(e.target.value)}
          inputProps={{ 'aria-label': 'file' }}
        />
        <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1}>
          <TextField size="small" label="id" value={id} onChange={(e) => setId(e.target.value)} />
          <TextField size="small" label="voice" value={voice} onChange={(e) => setVoice(e.target.value)} />
          <TextField
            size="small"
            label="volume"
            value={volume}
            onChange={(e) => setVolume(e.target.value)}
            error={!!volIssue.error}
            helperText={volIssue.error ?? volIssue.warning}
            inputProps={{ 'aria-label': 'volume' }}
          />
        </Stack>
        <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1}>
          <TextField size="small" label="fade_in (ms)" value={fadeIn} onChange={(e) => setFadeIn(e.target.value)} />
          <TextField size="small" label="crossfade_ms" value={crossfade} onChange={(e) => setCrossfade(e.target.value)} />
          <TextField size="small" label="start_position_ms" value={startPos} onChange={(e) => setStartPos(e.target.value)} />
          <FormControlLabel control={<Switch checked={loop} onChange={(e) => setLoop(e.target.checked)} />} label="loop" />
        </Stack>
        <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1}>
          <TextField select size="small" label="mode" value={mode} onChange={(e) => setMode(e.target.value as LoadMode | '')} sx={{ minWidth: 120 }}>
            <MenuItem value="">auto (default)</MenuItem>
            <MenuItem value="auto">auto</MenuItem>
            <MenuItem value="full">full</MenuItem>
            <MenuItem value="stream">stream</MenuItem>
          </TextField>
          <TextField select size="small" label="freshness" value={freshness} onChange={(e) => setFreshness(e.target.value as Freshness | '')} sx={{ minWidth: 120 }}>
            <MenuItem value="">config default</MenuItem>
            <MenuItem value="trusting">trusting</MenuItem>
            <MenuItem value="dev">dev</MenuItem>
            <MenuItem value="pinned">pinned</MenuItem>
          </TextField>
          <TextField size="small" label="window_ms" value={windowMs} onChange={(e) => setWindowMs(e.target.value)} />
          <TextField size="small" label="prebuffer_ms" value={prebufferMs} onChange={(e) => setPrebufferMs(e.target.value)} />
          <FormControlLabel control={<Switch checked={cacheable} onChange={(e) => setCacheable(e.target.checked)} />} label="cacheable" />
        </Stack>
        <TextField size="small" label="macro(s) — comma separated" value={macros} onChange={(e) => setMacros(e.target.value)} helperText="Command params win over macros; macro definitions come from config (Sprint W8)." />
        {streamGainNote && (
          <Alert severity="info">mode=stream is forward-only: seek, loop-crossfade, reverse, variable speed, and per-route channel gain do not apply.</Alert>
        )}
        <Box component="pre" sx={{ m: 0, p: 1, bgcolor: 'background.default', borderRadius: 1, fontSize: 12, overflowX: 'auto' }} aria-label="play JSON preview">
          {preview}
        </Box>
        <Stack direction="row" spacing={2} alignItems="center">
          <Button variant="contained" disabled={!file.trim() || state.status === 'sending'} onClick={() => run((c) => c.rawCommand({ command: 'play', message: params }))}>
            Play
          </Button>
          {state.status === 'ok' && <Alert severity="success" sx={{ py: 0 }}>Accepted (enqueued — confirm on the dashboard).</Alert>}
          {state.status === 'error' && <Alert severity="error" sx={{ py: 0 }}>{state.message}</Alert>}
        </Stack>
      </Stack>
    </Paper>
  );
}
