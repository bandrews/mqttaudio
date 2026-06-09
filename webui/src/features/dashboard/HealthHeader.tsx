// Persistent health header (Sprint W2, F1): active counts, output channels,
// uptime, and the two health counters — clips (limiter held at ceiling) and
// "stream errors / rebuilds" (cpal fatal stream-error callbacks, NOT per-buffer
// underruns). Counters are cumulative; the Δ chip shows the change since the last
// poll (diffed client-side, DW6), never an instantaneous rate.

import Chip from '@mui/material/Chip';
import Paper from '@mui/material/Paper';
import Stack from '@mui/material/Stack';
import Tooltip from '@mui/material/Tooltip';
import Typography from '@mui/material/Typography';
import { useMetrics, useStatus } from '../../state/queries';
import { useDelta } from '../../state/rates';
import { formatUptime } from '../../utils/format';
import { CacheBudgetGauge } from './CacheBudgetGauge';

function Stat({ label, value }: { label: string; value: number | string }) {
  return (
    <Stack alignItems="center" sx={{ minWidth: 64 }}>
      <Typography variant="h6">{value}</Typography>
      <Typography variant="caption" color="text.secondary">
        {label}
      </Typography>
    </Stack>
  );
}

function CounterChip({
  label,
  tip,
  value,
  delta,
  warn,
}: {
  label: string;
  tip: string;
  value: number;
  delta: number;
  warn: boolean;
}) {
  return (
    <Tooltip title={tip}>
      <Chip
        size="small"
        color={warn && value > 0 ? 'warning' : 'default'}
        variant={value > 0 ? 'filled' : 'outlined'}
        label={`${label}: ${value}${delta > 0 ? ` (+${delta})` : ''}`}
        aria-label={`${label} ${value}`}
      />
    </Tooltip>
  );
}

export function HealthHeader() {
  const status = useStatus();
  const metrics = useMetrics();
  const s = status.data;
  const m = metrics.data;

  const clips = useDelta(m?.clips ?? s?.clip_count ?? 0);
  const xruns = useDelta(m?.xruns ?? s?.xruns ?? 0);

  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Stack direction={{ xs: 'column', md: 'row' }} spacing={3} alignItems="center" flexWrap="wrap">
        <Stack direction="row" spacing={2}>
          <Stat label="samples" value={s?.active_samples ?? 0} />
          <Stat label="voices" value={s?.active_voices ?? 0} />
          <Stat label="inputs" value={s?.active_inputs ?? 0} />
          <Stat label="channels" value={s?.output_channels ?? 0} />
        </Stack>
        <Stack direction="row" spacing={1} alignItems="center">
          <CounterChip
            label="clips"
            tip="Output samples the limiter held at the ceiling (cumulative since startup)"
            value={clips.value}
            delta={clips.delta}
            warn
          />
          <CounterChip
            label="stream errors"
            tip="cpal stream-error callbacks that triggered a stream rebuild (cumulative); not per-buffer underruns"
            value={xruns.value}
            delta={xruns.delta}
            warn
          />
          {m && (
            <Chip
              size="small"
              variant="outlined"
              label={`up ${formatUptime(m.uptime_seconds)}`}
              aria-label="uptime"
            />
          )}
        </Stack>
        <CacheBudgetGauge />
      </Stack>
    </Paper>
  );
}
