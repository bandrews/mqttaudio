// Voices rack (Sprint W2, F4): the live voices from /status/voices with their
// volume and resolved ducking multiplier. A multiplier < 1.0 means the voice is
// being ducked.

import Chip from '@mui/material/Chip';
import LinearProgress from '@mui/material/LinearProgress';
import Paper from '@mui/material/Paper';
import Stack from '@mui/material/Stack';
import Typography from '@mui/material/Typography';
import { useStatusVoices } from '../../state/queries';

export function VoicesRack() {
  const { data } = useStatusVoices();
  const voices = data?.voices ?? [];

  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Typography variant="subtitle1" gutterBottom>
        Voices ({voices.length})
      </Typography>
      {voices.length === 0 ? (
        <Typography variant="body2" color="text.secondary">
          No active voices.
        </Typography>
      ) : (
        <Stack spacing={1.5} aria-label="voices">
          {voices.map((v) => {
            const ducked = v.ducking_multiplier < 1;
            return (
              <Stack key={v.id} spacing={0.5}>
                <Stack direction="row" spacing={1} alignItems="center">
                  <Typography variant="body2" sx={{ flexGrow: 1, fontWeight: 600 }}>
                    {v.id}
                  </Typography>
                  <Chip size="small" variant="outlined" label={`${v.sample_count} playing`} />
                  <Chip size="small" variant="outlined" label={`vol ${v.volume.toFixed(2)}`} />
                  {ducked && (
                    <Chip
                      size="small"
                      color="warning"
                      label={`ducked ×${v.ducking_multiplier.toFixed(2)}`}
                      aria-label={`${v.id} ducked`}
                    />
                  )}
                </Stack>
                <LinearProgress
                  variant="determinate"
                  value={Math.min(100, v.volume * v.ducking_multiplier * 100)}
                  sx={{ height: 6, borderRadius: 1 }}
                  aria-label={`${v.id} effective level`}
                />
              </Stack>
            );
          })}
        </Stack>
      )}
    </Paper>
  );
}
