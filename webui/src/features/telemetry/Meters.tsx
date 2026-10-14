// Live output meters (Sprint W7), driven by the /ws/state tick channel. Each
// output channel shows its peak as a bar that moves at the tick rate (~15 Hz).
// Off by default; needs Telemetry enabled. Falls back to a hint when off.

import LinearProgress from '@mui/material/LinearProgress';
import Paper from '@mui/material/Paper';
import Stack from '@mui/material/Stack';
import Typography from '@mui/material/Typography';
import { useClient } from '../../state/clientContext';
import { useTelemetry } from '../../state/queries';
import { useStateChannel } from './useStateChannel';

/** Linear amplitude -> 0..100 for the bar (clamped). */
function level(amp: number): number {
  return Math.min(100, Math.max(0, amp * 100));
}

export function Meters() {
  const client = useClient();
  const telemetryOn = useTelemetry().data?.enabled ?? false;
  const { connected, output } = useStateChannel(client, telemetryOn);

  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Stack direction="row" spacing={1} alignItems="center" mb={1}>
        <Typography variant="subtitle1" sx={{ flexGrow: 1 }}>
          Output meters
        </Typography>
        {telemetryOn && (
          <Typography variant="caption" color={connected ? 'success.main' : 'text.secondary'}>
            {connected ? 'live' : 'connecting…'}
          </Typography>
        )}
      </Stack>
      {!telemetryOn ? (
        <Typography variant="body2" color="text.secondary">
          Enable Telemetry (top bar) for live output meters.
        </Typography>
      ) : output.length === 0 ? (
        <Typography variant="body2" color="text.secondary">
          Waiting for audio…
        </Typography>
      ) : (
        <Stack spacing={0.75} aria-label="output meters">
          {output.map((amp, ch) => (
            <Stack key={ch} direction="row" spacing={1} alignItems="center">
              <Typography variant="caption" sx={{ width: 36 }}>
                ch {ch}
              </Typography>
              <LinearProgress
                variant="determinate"
                value={level(amp)}
                sx={{ flexGrow: 1, height: 8, borderRadius: 1 }}
                aria-label={`output meter ${ch}`}
              />
            </Stack>
          ))}
        </Stack>
      )}
    </Paper>
  );
}
