// Memory-cache budget gauge (Sprint W2, F2): resident bytes vs the resolved cap
// with headroom, from /metrics. A null cap means an unlimited budget.

import Box from '@mui/material/Box';
import LinearProgress from '@mui/material/LinearProgress';
import Stack from '@mui/material/Stack';
import Typography from '@mui/material/Typography';
import { useMetrics } from '../../state/queries';
import { formatBytes } from '../../utils/format';

export function CacheBudgetGauge() {
  const { data } = useMetrics();
  const cache = data?.cache;
  const used = cache?.memory_bytes ?? 0;
  const cap = cache?.memory_cap_bytes ?? null;
  const unlimited = cap === null || cap === undefined;
  const pct = !unlimited && cap > 0 ? Math.min(100, (used / cap) * 100) : 0;

  return (
    <Box sx={{ minWidth: 220 }} aria-label="cache memory budget">
      <Stack direction="row" justifyContent="space-between">
        <Typography variant="caption" color="text.secondary">
          Memory cache
        </Typography>
        <Typography variant="caption" color="text.secondary">
          {formatBytes(used)} {unlimited ? '/ unlimited' : `/ ${formatBytes(cap)}`}
        </Typography>
      </Stack>
      <LinearProgress
        variant={unlimited ? 'indeterminate' : 'determinate'}
        value={pct}
        sx={{ height: 8, borderRadius: 1, mt: 0.5 }}
        aria-label="memory cache usage"
      />
      {!unlimited && cache?.memory_headroom_bytes != null && (
        <Typography variant="caption" color="text.secondary">
          {formatBytes(cache.memory_headroom_bytes)} free · disk {formatBytes(cache.disk_bytes)}
        </Typography>
      )}
    </Box>
  );
}
