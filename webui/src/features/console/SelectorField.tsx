// The OR-logic sample selector (internal_id/id/file/voice). Warns loudly when no
// criterion is set — an empty selector matches nothing and is a silent no-op on
// the daemon (API-CONTRACT §3).

import Alert from '@mui/material/Alert';
import Stack from '@mui/material/Stack';
import TextField from '@mui/material/TextField';
import type { SampleSelector } from '../../api/contract';
import { validateInternalId } from './validation';
import { isSelectorEmpty } from './selector';

export function SelectorField({
  value,
  onChange,
}: {
  value: SampleSelector;
  onChange: (s: SampleSelector) => void;
}) {
  const empty = isSelectorEmpty(value);
  const idIssue = validateInternalId(value.internal_id ?? '');
  const set = (key: keyof SampleSelector, v: string) => onChange({ ...value, [key]: v });

  return (
    <Stack spacing={1}>
      <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1}>
        <TextField
          size="small"
          label="internal_id"
          value={value.internal_id ?? ''}
          onChange={(e) => set('internal_id', e.target.value)}
          error={!!idIssue.warning}
          helperText={idIssue.warning}
          inputProps={{ 'aria-label': 'internal_id' }}
        />
        <TextField
          size="small"
          label="id"
          value={value.id ?? ''}
          onChange={(e) => set('id', e.target.value)}
          inputProps={{ 'aria-label': 'id' }}
        />
        <TextField
          size="small"
          label="file"
          value={value.file ?? ''}
          onChange={(e) => set('file', e.target.value)}
          inputProps={{ 'aria-label': 'file selector' }}
        />
        <TextField
          size="small"
          label="voice"
          value={value.voice ?? ''}
          onChange={(e) => set('voice', e.target.value)}
          inputProps={{ 'aria-label': 'voice selector' }}
        />
      </Stack>
      {empty && (
        <Alert severity="warning" aria-label="empty selector warning">
          No selector set — the daemon will match nothing (a silent no-op). Set at least one of internal_id /
          id / file / voice.
        </Alert>
      )}
    </Stack>
  );
}
