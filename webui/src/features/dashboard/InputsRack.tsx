// Inputs rack (Sprint W2, F5): configured live inputs from /status/inputs with
// their volume and mute state. NOTE: `muted` is derived by the daemon as
// `volume == 0.0`, so a deliberately-zeroed input is indistinguishable from a
// muted one — surfaced here as a caveat.

import Chip from '@mui/material/Chip';
import Paper from '@mui/material/Paper';
import Stack from '@mui/material/Stack';
import Tooltip from '@mui/material/Tooltip';
import Typography from '@mui/material/Typography';
import { useStatusInputs } from '../../state/queries';

export function InputsRack() {
  const { data } = useStatusInputs();
  const inputs = data?.inputs ?? [];

  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Typography variant="subtitle1" gutterBottom>
        Inputs ({inputs.length})
      </Typography>
      {inputs.length === 0 ? (
        <Typography variant="body2" color="text.secondary">
          No live inputs configured.
        </Typography>
      ) : (
        <Stack spacing={1} aria-label="inputs">
          {inputs.map((input) => (
            <Stack key={input.index} direction="row" spacing={1} alignItems="center">
              <Typography variant="body2" sx={{ flexGrow: 1, fontWeight: 600 }}>
                {input.voice_id} <Typography component="span" variant="caption" color="text.secondary">#{input.index}</Typography>
              </Typography>
              <Chip size="small" variant="outlined" label={`${input.channels} ch`} />
              <Chip size="small" variant="outlined" label={`vol ${input.volume.toFixed(2)}`} />
              {input.muted ? (
                <Tooltip title="Derived as volume == 0.0">
                  <Chip size="small" color="default" label="muted" aria-label={`${input.voice_id} muted`} />
                </Tooltip>
              ) : (
                <Chip size="small" color="success" variant="outlined" label="live" />
              )}
            </Stack>
          ))}
        </Stack>
      )}
    </Paper>
  );
}
