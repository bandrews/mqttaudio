// Now-playing board (Sprint W2, F3): the active samples from /status/samples with
// their static metadata. Live position is NOT available until Sprint W6 (the
// daemon hard-codes position/progress to 0), so this shows "live position
// unavailable" rather than a fake progress bar.

import Box from '@mui/material/Box';
import Chip from '@mui/material/Chip';
import LinearProgress from '@mui/material/LinearProgress';
import Paper from '@mui/material/Paper';
import Stack from '@mui/material/Stack';
import Typography from '@mui/material/Typography';
import { useStatusSamples, useTelemetry } from '../../state/queries';
import { formatDuration } from '../../utils/format';
import { isWindowed } from '../mixer/windowed';
import type { SampleInfo } from '../../api/contract';

function basename(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

function SampleRow({ sample, telemetryOn }: { sample: SampleInfo; telemetryOn: boolean }) {
  const windowed = isWindowed(sample);
  const showProgress = telemetryOn && !windowed && sample.total_ms > 0;
  return (
    <Paper variant="outlined" sx={{ p: 1.5 }}>
      <Stack direction="row" spacing={1} alignItems="center" flexWrap="wrap">
        <Typography variant="body2" sx={{ fontWeight: 600, flexGrow: 1 }} title={sample.file}>
          {basename(sample.file)}
        </Typography>
        <Chip size="small" variant="outlined" label={`voice: ${sample.voice}`} />
        <Chip size="small" variant="outlined" label={`vol ${sample.volume.toFixed(2)}`} />
        {sample.speed !== 1 && <Chip size="small" variant="outlined" label={`${sample.speed}×`} />}
        {sample.loop_mode && <Chip size="small" color="info" variant="outlined" label="loop" />}
        {windowed && <Chip size="small" color="warning" label="streamed" />}
        <Typography variant="caption" color="text.secondary">
          {formatDuration(sample.total_ms)}
        </Typography>
      </Stack>
      {showProgress ? (
        <Box mt={0.5}>
          <LinearProgress
            variant="determinate"
            value={Math.min(100, sample.progress_percent)}
            sx={{ height: 6, borderRadius: 1 }}
            aria-label={`progress ${sample.internal_id}`}
          />
          <Typography variant="caption" color="text.secondary">
            {formatDuration(sample.position_ms)} / {formatDuration(sample.total_ms)}
          </Typography>
        </Box>
      ) : (
        <Typography variant="caption" color="text.secondary">
          {windowed ? 'streamed (forward-only)' : 'live position unavailable (enable Telemetry)'}
        </Typography>
      )}
    </Paper>
  );
}

export function NowPlayingBoard() {
  const { data } = useStatusSamples();
  const telemetryOn = useTelemetry().data?.enabled ?? false;
  const samples = data?.samples ?? [];

  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Typography variant="subtitle1" gutterBottom>
        Now playing ({samples.length})
      </Typography>
      {samples.length === 0 ? (
        <Typography variant="body2" color="text.secondary">
          No samples playing.
        </Typography>
      ) : (
        <Stack spacing={1} aria-label="active samples">
          {samples.map((s) => (
            <SampleRow key={s.internal_id} sample={s} telemetryOn={telemetryOn} />
          ))}
        </Stack>
      )}
      <Box sx={{ height: 0 }} />
    </Paper>
  );
}
