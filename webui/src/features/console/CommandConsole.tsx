// Command test-bench (Sprint W3): forms for every runtime command with client-
// side validation, the OR-logic selector with its empty-selector warning, the cue
// launcher (full play surface via /command), and the raw /command editor.

import { useState } from 'react';
import Alert from '@mui/material/Alert';
import Button from '@mui/material/Button';
import Divider from '@mui/material/Divider';
import FormControlLabel from '@mui/material/FormControlLabel';
import Paper from '@mui/material/Paper';
import Stack from '@mui/material/Stack';
import Switch from '@mui/material/Switch';
import TextField from '@mui/material/TextField';
import Typography from '@mui/material/Typography';
import type { SampleSelector } from '../../api/contract';
import { CueLauncher } from './CueLauncher';
import { RawCommandEditor } from './RawCommandEditor';
import { SelectorField } from './SelectorField';
import { selectorToParams } from './selector';
import { useCommandRunner, type RunState } from './useCommandRunner';
import { validateSpeed, validateVolume } from './validation';

function Result({ state }: { state: RunState }) {
  if (state.status === 'ok') return <Alert severity="success" sx={{ py: 0 }}>{state.message}</Alert>;
  if (state.status === 'error') return <Alert severity="error" sx={{ py: 0 }}>{state.message}</Alert>;
  return null;
}

function num(s: string, fallback = 0): number {
  const n = Number(s);
  return Number.isFinite(n) ? n : fallback;
}

export function SampleControl() {
  const { state, run } = useCommandRunner();
  const [sel, setSel] = useState<SampleSelector>({});
  const [fadeOut, setFadeOut] = useState('');
  const [volume, setVolume] = useState('0.5');
  const [positionMs, setPositionMs] = useState('0');
  const [speed, setSpeed] = useState('1');
  const [pitch, setPitch] = useState(false);
  const s = selectorToParams(sel);
  const volIssue = validateVolume(num(volume));
  const speedIssue = validateSpeed(num(speed), pitch);

  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Typography variant="subtitle1" gutterBottom>
        Sample control (stop / volume / seek / speed)
      </Typography>
      <Stack spacing={1.5}>
        <SelectorField value={sel} onChange={setSel} />
        <Divider />
        <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1} alignItems="center">
          <TextField size="small" label="fade_out_ms" value={fadeOut} onChange={(e) => setFadeOut(e.target.value)} sx={{ width: 140 }} />
          <Button variant="outlined" onClick={() => run((c) => c.stop({ ...s, ...(fadeOut.trim() ? { fade_out_ms: num(fadeOut) } : {}) }))}>Stop</Button>
        </Stack>
        <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1} alignItems="center">
          <TextField size="small" label="volume" value={volume} onChange={(e) => setVolume(e.target.value)} error={!!volIssue.error} helperText={volIssue.error ?? volIssue.warning} sx={{ width: 200 }} />
          <Button variant="outlined" onClick={() => run((c) => c.volume({ ...s, volume: num(volume) }))}>Set volume</Button>
        </Stack>
        <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1} alignItems="center">
          <TextField size="small" label="position_ms" value={positionMs} onChange={(e) => setPositionMs(e.target.value)} sx={{ width: 140 }} />
          <Button variant="outlined" onClick={() => run((c) => c.seek({ ...s, position_ms: num(positionMs) }))}>Seek</Button>
        </Stack>
        <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1} alignItems="center">
          <TextField size="small" label="speed" value={speed} onChange={(e) => setSpeed(e.target.value)} error={!!speedIssue.error} helperText={speedIssue.error ?? speedIssue.warning} sx={{ width: 220 }} />
          <FormControlLabel control={<Switch checked={pitch} onChange={(e) => setPitch(e.target.checked)} />} label="pitch correction" />
          <Button variant="outlined" disabled={!!speedIssue.error} onClick={() => run((c) => c.speed({ ...s, speed: num(speed), pitch_correction: pitch }))}>Set speed</Button>
        </Stack>
        <Result state={state} />
      </Stack>
    </Paper>
  );
}

export function VoiceControl() {
  const { state, run } = useCommandRunner();
  const [voice, setVoice] = useState('');
  const [volume, setVolume] = useState('0.5');
  const [timeMs, setTimeMs] = useState('2000');
  const disabled = !voice.trim();
  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Typography variant="subtitle1" gutterBottom>Voice control</Typography>
      <Stack spacing={1.5}>
        <TextField size="small" label="voice" value={voice} onChange={(e) => setVoice(e.target.value)} inputProps={{ 'aria-label': 'voice' }} />
        <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1} alignItems="center">
          <TextField size="small" label="volume" value={volume} onChange={(e) => setVolume(e.target.value)} sx={{ width: 140 }} />
          <Button variant="outlined" disabled={disabled} onClick={() => run((c) => c.voiceVolume({ voice: voice.trim(), volume: num(volume) }))}>Set volume</Button>
          <TextField size="small" label="time_ms" value={timeMs} onChange={(e) => setTimeMs(e.target.value)} sx={{ width: 120 }} />
          <Button variant="outlined" disabled={disabled} onClick={() => run((c) => c.voiceFadeOut({ voice: voice.trim(), time_ms: num(timeMs) }))}>Fade out</Button>
          <Button variant="outlined" color="warning" disabled={disabled} onClick={() => run((c) => c.voiceStop({ voice: voice.trim() }))}>Stop</Button>
        </Stack>
        <Result state={state} />
      </Stack>
    </Paper>
  );
}

export function InputControl() {
  const { state, run } = useCommandRunner();
  const [input, setInput] = useState('');
  const [volume, setVolume] = useState('0.5');
  const [mute, setMute] = useState(false);
  const disabled = !input.trim();
  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Typography variant="subtitle1" gutterBottom>Input control</Typography>
      <Stack spacing={1.5}>
        <TextField size="small" label="input (index or voice_id, a string)" value={input} onChange={(e) => setInput(e.target.value)} inputProps={{ 'aria-label': 'input' }} />
        <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1} alignItems="center">
          <TextField size="small" label="volume" value={volume} onChange={(e) => setVolume(e.target.value)} sx={{ width: 140 }} />
          <Button variant="outlined" disabled={disabled} onClick={() => run((c) => c.inputVolume({ input: input.trim(), volume: num(volume) }))}>Set volume</Button>
          <FormControlLabel control={<Switch checked={mute} onChange={(e) => setMute(e.target.checked)} />} label="mute" />
          <Button variant="outlined" disabled={disabled} onClick={() => run((c) => c.inputMute({ input: input.trim(), mute }))}>Apply mute</Button>
        </Stack>
        <Result state={state} />
      </Stack>
    </Paper>
  );
}

export function CacheControl() {
  const { state, run } = useCommandRunner();
  const [file, setFile] = useState('');
  const disabled = !file.trim();
  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Typography variant="subtitle1" gutterBottom>Cache & transport</Typography>
      <Stack spacing={1.5}>
        <TextField size="small" label="file (path or URL)" value={file} onChange={(e) => setFile(e.target.value)} inputProps={{ 'aria-label': 'cache file' }} />
        <Stack direction="row" spacing={1} flexWrap="wrap" useFlexGap>
          <Button variant="outlined" disabled={disabled} onClick={() => run((c) => c.precache({ file: file.trim() }))}>Precache</Button>
          <Button variant="outlined" disabled={disabled} onClick={() => run((c) => c.cacheInvalidate({ file: file.trim() }))}>Invalidate</Button>
          <Button variant="outlined" disabled={disabled} onClick={() => run((c) => c.cacheReload({ file: file.trim() }))}>Reload</Button>
          <Button variant="outlined" color="warning" onClick={() => run((c) => c.cacheClear())}>Clear cache</Button>
          <Button variant="contained" color="error" onClick={() => run((c) => c.stopAll())}>Stop all</Button>
        </Stack>
        <Result state={state} />
      </Stack>
    </Paper>
  );
}

export function CommandConsole() {
  return (
    <Stack spacing={2}>
      <CueLauncher />
      <SampleControl />
      <VoiceControl />
      <InputControl />
      <CacheControl />
      <RawCommandEditor />
    </Stack>
  );
}
