// Voice and input control strips (Sprint W5): live volume faders, voice fade-out
// (time_ms), voice stop, and input mute. input_mute restores the prior level on
// the daemon (not a hardcoded 1.0).

import { useState } from 'react';
import Button from '@mui/material/Button';
import Chip from '@mui/material/Chip';
import FormControlLabel from '@mui/material/FormControlLabel';
import Paper from '@mui/material/Paper';
import Slider from '@mui/material/Slider';
import Stack from '@mui/material/Stack';
import Switch from '@mui/material/Switch';
import TextField from '@mui/material/TextField';
import Typography from '@mui/material/Typography';
import type { InputInfo, VoiceInfo } from '../../api/contract';
import { useClient } from '../../state/clientContext';

export function VoiceStrip({ voice }: { voice: VoiceInfo }) {
  const client = useClient();
  const [vol, setVol] = useState(voice.volume);
  const [timeMs, setTimeMs] = useState('2000');
  const ducked = voice.ducking_multiplier < 1;
  return (
    <Paper variant="outlined" sx={{ p: 1.5 }}>
      <Stack direction="row" spacing={1} alignItems="center" mb={0.5}>
        <Typography variant="body2" sx={{ fontWeight: 600, flexGrow: 1 }}>
          {voice.id}
        </Typography>
        <Chip size="small" variant="outlined" label={`${voice.sample_count} playing`} />
        {ducked && <Chip size="small" color="warning" label={`ducked ×${voice.ducking_multiplier.toFixed(2)}`} />}
      </Stack>
      <Stack direction="row" spacing={1} alignItems="center">
        <Typography variant="caption" sx={{ width: 64 }}>
          vol {vol.toFixed(2)}
        </Typography>
        <Slider
          size="small"
          min={0}
          max={1}
          step={0.01}
          value={vol}
          onChange={(_e, v) => setVol(v as number)}
          onChangeCommitted={(_e, v) => client?.voiceVolume({ voice: voice.id, volume: v as number })}
          aria-label={`voice ${voice.id} volume`}
        />
      </Stack>
      <Stack direction="row" spacing={1} alignItems="center" mt={0.5}>
        <TextField size="small" label="time_ms" value={timeMs} onChange={(e) => setTimeMs(e.target.value)} sx={{ width: 110 }} />
        <Button size="small" variant="outlined" onClick={() => client?.voiceFadeOut({ voice: voice.id, time_ms: Number(timeMs) || 0 })}>
          Fade out
        </Button>
        <Button size="small" variant="outlined" color="warning" onClick={() => client?.voiceStop({ voice: voice.id })}>
          Stop
        </Button>
      </Stack>
    </Paper>
  );
}

export function InputStrip({ input }: { input: InputInfo }) {
  const client = useClient();
  const [vol, setVol] = useState(input.volume);
  return (
    <Paper variant="outlined" sx={{ p: 1.5 }}>
      <Stack direction="row" spacing={1} alignItems="center" mb={0.5}>
        <Typography variant="body2" sx={{ fontWeight: 600, flexGrow: 1 }}>
          {input.voice_id} <Typography component="span" variant="caption" color="text.secondary">#{input.index} · {input.channels}ch</Typography>
        </Typography>
        <FormControlLabel
          control={
            <Switch
              size="small"
              checked={input.muted}
              onChange={(e) => client?.inputMute({ input: String(input.index), mute: e.target.checked })}
              inputProps={{ 'aria-label': `mute input ${input.index}` }}
            />
          }
          label={<Typography variant="caption">mute</Typography>}
        />
      </Stack>
      <Stack direction="row" spacing={1} alignItems="center">
        <Typography variant="caption" sx={{ width: 64 }}>
          vol {vol.toFixed(2)}
        </Typography>
        <Slider
          size="small"
          min={0}
          max={1}
          step={0.01}
          value={vol}
          onChange={(_e, v) => setVol(v as number)}
          onChangeCommitted={(_e, v) => client?.inputVolume({ input: String(input.index), volume: v as number })}
          aria-label={`input ${input.index} volume`}
        />
      </Stack>
    </Paper>
  );
}
