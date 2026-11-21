// The mixer (Sprint W5): interactive control over already-playing audio —
// per-sample transport, voice strips, and input strips. Reads the same query
// hooks as the monitor dashboard but adds controls.

import Box from '@mui/material/Box';
import Paper from '@mui/material/Paper';
import Stack from '@mui/material/Stack';
import Typography from '@mui/material/Typography';
import { useStatusSamples, useStatusVoices, useStatusInputs } from '../../state/queries';
import { SampleTransport } from './SampleTransport';
import { VoiceStrip, InputStrip } from './strips';

export function MixerView() {
  const samples = useStatusSamples().data?.samples ?? [];
  const voices = useStatusVoices().data?.voices ?? [];
  const inputs = useStatusInputs().data?.inputs ?? [];

  return (
    <Box sx={{ display: 'grid', gap: 2, gridTemplateColumns: { xs: '1fr', md: '2fr 1fr' } }}>
      <Paper variant="outlined" sx={{ p: 2 }}>
        <Typography variant="subtitle1" gutterBottom>
          Transport ({samples.length})
        </Typography>
        {samples.length === 0 ? (
          <Typography variant="body2" color="text.secondary">
            No samples playing.
          </Typography>
        ) : (
          <Stack spacing={1}>
            {samples.map((s) => (
              <SampleTransport key={s.internal_id} sample={s} />
            ))}
          </Stack>
        )}
      </Paper>
      <Stack spacing={2}>
        <Paper variant="outlined" sx={{ p: 2 }}>
          <Typography variant="subtitle1" gutterBottom>
            Voices ({voices.length})
          </Typography>
          {voices.length === 0 ? (
            <Typography variant="body2" color="text.secondary">
              No active voices.
            </Typography>
          ) : (
            <Stack spacing={1}>
              {voices.map((v) => (
                <VoiceStrip key={v.id} voice={v} />
              ))}
            </Stack>
          )}
        </Paper>
        <Paper variant="outlined" sx={{ p: 2 }}>
          <Typography variant="subtitle1" gutterBottom>
            Inputs ({inputs.length})
          </Typography>
          {inputs.length === 0 ? (
            <Typography variant="body2" color="text.secondary">
              No live inputs.
            </Typography>
          ) : (
            <Stack spacing={1}>
              {inputs.map((i) => (
                <InputStrip key={i.index} input={i} />
              ))}
            </Stack>
          )}
        </Paper>
      </Stack>
    </Box>
  );
}
