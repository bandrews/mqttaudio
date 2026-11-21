// Config visibility & tuning (Sprint W8). The daemon reads config once at startup
// (no hot-reload, DW8), so this DISPLAYS the running config (GET /config, secrets
// redacted) and produces validated config-JSON snippets flagged "restart
// required". The only live-tunable config-derived state is per-input volume/mute —
// done in the Mixer tab. Ducking rules are config-bound, but their effect is shown
// live from /metrics.

import { useMemo, useState } from 'react';
import Alert from '@mui/material/Alert';
import Box from '@mui/material/Box';
import Button from '@mui/material/Button';
import Chip from '@mui/material/Chip';
import Divider from '@mui/material/Divider';
import MenuItem from '@mui/material/MenuItem';
import Paper from '@mui/material/Paper';
import Slider from '@mui/material/Slider';
import Stack from '@mui/material/Stack';
import TextField from '@mui/material/TextField';
import Typography from '@mui/material/Typography';
import { useConfig, useMetrics } from '../../state/queries';

type Json = Record<string, unknown>;

function asObject(value: unknown): Json {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as Json) : {};
}

function RestartBanner() {
  return (
    <Alert severity="info">
      These settings are read once at startup — editing here produces a config snippet to merge into your
      config file and <strong>restart</strong> the daemon. The only config-derived state you can change live is
      per-input volume/mute (in the <strong>Mixer</strong> tab).
    </Alert>
  );
}

function Snippet({ label, value }: { label: string; value: unknown }) {
  const text = JSON.stringify(value, null, 2);
  return (
    <Box>
      <Stack direction="row" spacing={1} alignItems="center" mb={0.5}>
        <Typography variant="caption" color="text.secondary" sx={{ flexGrow: 1 }}>
          {label} — restart required
        </Typography>
        <Button size="small" onClick={() => void navigator.clipboard?.writeText(text)}>
          Copy
        </Button>
      </Stack>
      <Box
        component="pre"
        aria-label={`${label} snippet`}
        sx={{ m: 0, p: 1, bgcolor: 'background.default', borderRadius: 1, fontSize: 12, overflowX: 'auto' }}
      >
        {text}
      </Box>
    </Box>
  );
}

function OutputStageEditor({ audio }: { audio: Json }) {
  const [masterGain, setMasterGain] = useState(typeof audio.master_gain === 'number' ? audio.master_gain : 1);
  const [ceiling, setCeiling] = useState(
    typeof audio.output_ceiling_db === 'number' ? audio.output_ceiling_db : -1,
  );
  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Typography variant="subtitle1" gutterBottom>
        Output stage (limiter)
      </Typography>
      <Stack spacing={1}>
        <Stack direction="row" spacing={2} alignItems="center">
          <Typography variant="caption" sx={{ width: 150 }}>
            master_gain {masterGain.toFixed(2)}
          </Typography>
          <Slider min={0} max={8} step={0.05} value={masterGain} onChange={(_e, v) => setMasterGain(v as number)} aria-label="master_gain" />
        </Stack>
        <Stack direction="row" spacing={2} alignItems="center">
          <Typography variant="caption" sx={{ width: 150 }}>
            output_ceiling_db {ceiling.toFixed(1)}
          </Typography>
          <Slider min={-60} max={0} step={0.5} value={ceiling} onChange={(_e, v) => setCeiling(v as number)} aria-label="output_ceiling_db" />
        </Stack>
        <Snippet label="audio output stage" value={{ audio: { master_gain: masterGain, output_ceiling_db: ceiling } }} />
      </Stack>
    </Paper>
  );
}

function SectionEditor({ config }: { config: Json }) {
  const sections = Object.keys(config);
  const [section, setSection] = useState(sections.includes('ducking_rules') ? 'ducking_rules' : (sections[0] ?? ''));
  const [text, setText] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [snippet, setSnippet] = useState<unknown>(null);

  const current = useMemo(() => JSON.stringify(config[section] ?? null, null, 2), [config, section]);

  function load() {
    setText(current);
    setError(null);
    setSnippet(null);
  }

  function build() {
    try {
      const parsed = JSON.parse(text || current);
      setSnippet({ [section]: parsed });
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Invalid JSON');
      setSnippet(null);
    }
  }

  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Typography variant="subtitle1" gutterBottom>
        Section editor (ducking rules, bass/LFE, channel aliases, calibration, macros, inputs…)
      </Typography>
      <Stack spacing={1}>
        <Stack direction="row" spacing={1} alignItems="center">
          <TextField select size="small" label="section" value={section} onChange={(e) => { setSection(e.target.value); setText(''); setSnippet(null); }} sx={{ minWidth: 200 }}>
            {sections.map((s) => (
              <MenuItem key={s} value={s}>
                {s}
              </MenuItem>
            ))}
          </TextField>
          <Button size="small" onClick={load}>
            Load current
          </Button>
          <Button size="small" variant="outlined" onClick={build}>
            Build snippet
          </Button>
        </Stack>
        <TextField
          multiline
          minRows={6}
          value={text || current}
          onChange={(e) => setText(e.target.value)}
          inputProps={{ 'aria-label': 'section json', style: { fontFamily: 'monospace', fontSize: 12 } }}
        />
        {error && <Alert severity="error" aria-label="section json error">{error}</Alert>}
        {snippet != null && <Snippet label={section} value={snippet} />}
      </Stack>
    </Paper>
  );
}

function DuckingViz() {
  const { data } = useMetrics();
  const ducking = data?.ducking ?? {};
  const entries = Object.entries(ducking);
  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Typography variant="subtitle1" gutterBottom>
        Ducking (live)
      </Typography>
      {entries.length === 0 ? (
        <Typography variant="body2" color="text.secondary">
          No voices are currently ducked. Mic-triggered ducking holds while the input stream is open (not
          signal-gated).
        </Typography>
      ) : (
        <Stack direction="row" spacing={1} flexWrap="wrap" useFlexGap>
          {entries.map(([voice, mult]) => (
            <Chip key={voice} color="warning" label={`${voice}: ×${Number(mult).toFixed(2)}`} />
          ))}
        </Stack>
      )}
    </Paper>
  );
}

export function ConfigView() {
  const { data: config, isLoading } = useConfig();

  if (isLoading) {
    return <Typography variant="body2" color="text.secondary">Loading config…</Typography>;
  }
  if (!config) {
    return (
      <Alert severity="warning">
        The daemon did not return a config (older build without <code>GET /config</code>, or it is gated).
      </Alert>
    );
  }
  const audio = asObject((config as Json).audio);

  return (
    <Stack spacing={2}>
      <RestartBanner />
      <DuckingViz />
      <OutputStageEditor audio={audio} />
      <SectionEditor config={config as Json} />
      <Paper variant="outlined" sx={{ p: 2 }}>
        <Typography variant="subtitle1" gutterBottom>
          Running config (read-only, secrets redacted)
        </Typography>
        <Divider sx={{ mb: 1 }} />
        <Box component="pre" aria-label="running config" sx={{ m: 0, fontSize: 12, overflowX: 'auto', maxHeight: 360 }}>
          {JSON.stringify(config, null, 2)}
        </Box>
      </Paper>
    </Stack>
  );
}
