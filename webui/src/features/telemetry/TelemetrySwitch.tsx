// ABOUTME: Top-bar switch that turns daemon telemetry on or off via POST /telemetry.
// ABOUTME: Updates the telemetry cache and refreshes samples on a change; shows the error on failure.

// Telemetry opt-in toggle (Sprint W6, DW3). OFF by default. When on, the daemon
// publishes live sample positions (and, from Sprint W7, meters + state events).
// The tooltip notes it adds a little RT work, so it is opt-in. A failed change
// leaves the cached state alone and shows the error beside the switch.

import { useState } from 'react';
import Alert from '@mui/material/Alert';
import FormControlLabel from '@mui/material/FormControlLabel';
import Switch from '@mui/material/Switch';
import Tooltip from '@mui/material/Tooltip';
import Typography from '@mui/material/Typography';
import { useQueryClient } from '@tanstack/react-query';
import { useClient } from '../../state/clientContext';
import { useTelemetry } from '../../state/queries';

export function TelemetrySwitch() {
  const client = useClient();
  const queryClient = useQueryClient();
  const { data } = useTelemetry();
  const enabled = data?.enabled ?? false;
  const [error, setError] = useState<string | null>(null);

  async function onChange(on: boolean) {
    if (!client) return;
    setError(null);
    try {
      await client.setTelemetry(on);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      return;
    }
    queryClient.setQueryData(['telemetry'], { enabled: on });
    void queryClient.invalidateQueries({ queryKey: ['status', 'samples'] });
  }

  return (
    <>
      <Tooltip title="Opt in to live telemetry (sample positions + meters). Off by default; it adds a little real-time work, so enable it only while watching.">
        <FormControlLabel
          sx={{ mr: 1 }}
          control={
            <Switch
              size="small"
              checked={enabled}
              onChange={(e) => onChange(e.target.checked)}
              inputProps={{ 'aria-label': 'telemetry' }}
            />
          }
          label={<Typography variant="caption">Telemetry</Typography>}
        />
      </Tooltip>
      {error && (
        <Alert severity="error" sx={{ py: 0 }}>
          {error}
        </Alert>
      )}
    </>
  );
}
