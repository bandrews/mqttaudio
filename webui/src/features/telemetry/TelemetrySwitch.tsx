// Telemetry opt-in toggle (Sprint W6, DW3). OFF by default. When on, the daemon
// publishes live sample positions (and, from Sprint W7, meters + state events).
// The tooltip notes it adds a little RT work, so it is opt-in.

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

  async function onChange(on: boolean) {
    if (!client) return;
    await client.setTelemetry(on);
    queryClient.setQueryData(['telemetry'], { enabled: on });
    void queryClient.invalidateQueries({ queryKey: ['status', 'samples'] });
  }

  return (
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
  );
}
