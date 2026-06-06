// Per-sample transport (Sprint W5): a seek scrubber, a speed control with a
// pitch-correction toggle (which re-clamps the range and disables reverse), and a
// stop. Windowed/streamed voices are forward-only — seek/speed/reverse are
// disabled and a "streamed" badge is shown. Live playhead arrives in Sprint W6;
// the scrubber is set-only here. NOTE: /status/samples has no is-windowed flag
// yet, so windowed is INFERRED from total_frames === 0 (see docs/bugs.md, Sprint
// W5); Sprint W6 F4 adds the real flag.

import { useState } from 'react';
import Box from '@mui/material/Box';
import Button from '@mui/material/Button';
import Chip from '@mui/material/Chip';
import FormControlLabel from '@mui/material/FormControlLabel';
import Paper from '@mui/material/Paper';
import Slider from '@mui/material/Slider';
import Stack from '@mui/material/Stack';
import Switch from '@mui/material/Switch';
import Typography from '@mui/material/Typography';
import type { SampleInfo } from '../../api/contract';
import type { DaemonClient } from '../../api/client';
import { useClient } from '../../state/clientContext';
import { formatDuration } from '../../utils/format';
import { isWindowed } from './windowed';

function basename(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

function SpeedControl({ sample, disabled }: { sample: SampleInfo; disabled: boolean }) {
  const client = useClient();
  const [pitch, setPitch] = useState(false);
  const [speed, setSpeed] = useState(sample.speed || 1);
  const min = pitch ? 0.05 : -100;
  const max = pitch ? 8 : 100;
  const clamped = Math.min(max, Math.max(min, speed));

  function commit(value: number) {
    setSpeed(value);
    void client?.speed({ internal_id: sample.internal_id, speed: value, pitch_correction: pitch });
  }

  function togglePitch(on: boolean) {
    setPitch(on);
    const next = Math.min(on ? 8 : 100, Math.max(on ? 0.05 : -100, speed));
    setSpeed(next);
  }

  return (
    <Stack spacing={0.5}>
      <Stack direction="row" spacing={1} alignItems="center">
        <Typography variant="caption" sx={{ width: 90 }}>
          speed {clamped}×
        </Typography>
        <Slider
          size="small"
          min={min}
          max={max}
          step={pitch ? 0.05 : 0.1}
          value={clamped}
          disabled={disabled}
          marks={pitch ? [{ value: 1, label: '1×' }] : [{ value: 0, label: '0' }, { value: 1, label: '1×' }]}
          onChange={(_e, v) => setSpeed(v as number)}
          onChangeCommitted={(_e, v) => commit(v as number)}
          aria-label={`speed ${sample.internal_id}`}
        />
        <FormControlLabel
          control={<Switch size="small" checked={pitch} disabled={disabled} onChange={(e) => togglePitch(e.target.checked)} />}
          label={<Typography variant="caption">pitch</Typography>}
        />
      </Stack>
      {pitch && <Typography variant="caption" color="text.secondary">reverse (negative) disabled with pitch correction</Typography>}
    </Stack>
  );
}

function SeekControl({ sample, disabled }: { sample: SampleInfo; disabled: boolean }) {
  const client = useClient();
  const [pos, setPos] = useState(0);
  return (
    <Stack direction="row" spacing={1} alignItems="center">
      <Typography variant="caption" sx={{ width: 90 }}>
        seek {formatDuration(pos)}
      </Typography>
      <Slider
        size="small"
        min={0}
        max={sample.total_ms || 1}
        value={pos}
        disabled={disabled}
        onChange={(_e, v) => setPos(v as number)}
        onChangeCommitted={(_e, v) => client?.seek({ internal_id: sample.internal_id, position_ms: v as number })}
        aria-label={`seek ${sample.internal_id}`}
      />
      <Typography variant="caption" color="text.secondary">/ {formatDuration(sample.total_ms)}</Typography>
    </Stack>
  );
}

export function SampleTransport({ sample }: { sample: SampleInfo }) {
  const client: DaemonClient | null = useClient();
  const windowed = isWindowed(sample);

  return (
    <Paper variant="outlined" sx={{ p: 1.5 }}>
      <Stack direction="row" spacing={1} alignItems="center" mb={0.5}>
        <Typography variant="body2" sx={{ fontWeight: 600, flexGrow: 1 }} title={sample.file}>
          {basename(sample.file)}
        </Typography>
        <Chip size="small" variant="outlined" label={`voice: ${sample.voice}`} />
        {sample.loop_mode && <Chip size="small" color="info" variant="outlined" label="loop" />}
        {windowed && <Chip size="small" color="warning" label="streamed" aria-label={`${sample.internal_id} streamed`} />}
        <Button size="small" color="warning" onClick={() => client?.stop({ internal_id: sample.internal_id })}>
          Stop
        </Button>
      </Stack>
      {windowed ? (
        <Typography variant="caption" color="text.secondary">
          Windowed/streamed: forward-only — seek, speed, and reverse do not apply.
        </Typography>
      ) : (
        <Box>
          <SeekControl sample={sample} disabled={false} />
          <SpeedControl sample={sample} disabled={false} />
        </Box>
      )}
    </Paper>
  );
}
