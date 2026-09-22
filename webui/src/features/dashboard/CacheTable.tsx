// Cache table (Sprint W2, F6): memory and disk cache totals from /status/cache.

import Paper from '@mui/material/Paper';
import Table from '@mui/material/Table';
import TableBody from '@mui/material/TableBody';
import TableCell from '@mui/material/TableCell';
import TableHead from '@mui/material/TableHead';
import TableRow from '@mui/material/TableRow';
import Typography from '@mui/material/Typography';
import { useStatusCache } from '../../state/queries';
import { formatBytes } from '../../utils/format';

export function CacheTable() {
  const { data } = useStatusCache();
  const rows = [
    { tier: 'Memory', entries: data?.memory.entries ?? 0, bytes: data?.memory.size_bytes ?? 0 },
    { tier: 'Disk', entries: data?.disk.entries ?? 0, bytes: data?.disk.size_bytes ?? 0 },
  ];

  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Typography variant="subtitle1" gutterBottom>
        Cache
      </Typography>
      <Table size="small" aria-label="cache">
        <TableHead>
          <TableRow>
            <TableCell>Tier</TableCell>
            <TableCell align="right">Entries</TableCell>
            <TableCell align="right">Size</TableCell>
          </TableRow>
        </TableHead>
        <TableBody>
          {rows.map((r) => (
            <TableRow key={r.tier}>
              <TableCell>{r.tier}</TableCell>
              <TableCell align="right">{r.entries}</TableCell>
              <TableCell align="right">{formatBytes(r.bytes)}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </Paper>
  );
}
