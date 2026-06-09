// The live monitoring dashboard (Sprint W2): the persistent health header over a
// responsive grid of the now-playing board, voices/inputs racks, cache table, and
// the log console. All poll-driven via the TanStack Query layer (DW6); no faked
// data.

import Box from '@mui/material/Box';
import Stack from '@mui/material/Stack';
import { HealthHeader } from './HealthHeader';
import { NowPlayingBoard } from './NowPlayingBoard';
import { VoicesRack } from './VoicesRack';
import { InputsRack } from './InputsRack';
import { CacheTable } from './CacheTable';
import { LogConsole } from '../logs/LogConsole';
import { Meters } from '../telemetry/Meters';

export function Dashboard() {
  return (
    <Stack spacing={2}>
      <HealthHeader />
      <Box
        sx={{
          display: 'grid',
          gap: 2,
          gridTemplateColumns: { xs: '1fr', md: '2fr 1fr' },
        }}
      >
        <Stack spacing={2}>
          <NowPlayingBoard />
          <LogConsole />
        </Stack>
        <Stack spacing={2}>
          <Meters />
          <VoicesRack />
          <InputsRack />
          <CacheTable />
        </Stack>
      </Box>
    </Stack>
  );
}
