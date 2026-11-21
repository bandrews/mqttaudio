// ABOUTME: Edits speaker and place waypoints for a play cue.
// ABOUTME: Keeps optional arrival times, handoff length and show duration in the wire format.

import Button from '@mui/material/Button';
import FormControlLabel from '@mui/material/FormControlLabel';
import Stack from '@mui/material/Stack';
import Switch from '@mui/material/Switch';
import TextField from '@mui/material/TextField';
import type { SpeakerMotion, Waypoint } from '../../api/contract';

import { waypointFields } from './validation';

const row = { xs: 'column', sm: 'row' } as const;

function optionalNumber(text: string) {
  return text === '' ? undefined : Number(text);
}

export function SpeakerPath({
  motion,
  change,
}: {
  motion?: SpeakerMotion;
  change: (motion?: SpeakerMotion) => void;
}) {
  function setPoint(index: number, waypoint: Waypoint) {
    if (motion)
      change({ ...motion, path: motion.path.map((w, i) => (i === index ? waypoint : w)) });
  }

  return (
    <Stack spacing={1}>
      <FormControlLabel
        label="Speaker motion"
        control={
          <Switch
            checked={!!motion}
            onChange={(_, on) => change(on ? { path: [0, 1], duration_ms: 5000 } : undefined)}
          />
        }
      />
      {motion && (
        <>
          {motion.path.map((waypoint, i) => {
            const w = waypointFields(waypoint);
            const isPlace = 'place' in w;
            const name = isPlace ? w.place : w.speaker;
            return (
              <Stack key={i} direction={row} spacing={1}>
                <TextField
                  select
                  SelectProps={{ native: true }}
                  size="small"
                  label={`Destination type ${i + 1}`}
                  value={isPlace ? 'place' : 'speaker'}
                  onChange={(e) =>
                    setPoint(
                      i,
                      e.target.value === 'place'
                        ? { place: String(name), at_ms: w.at_ms }
                        : { speaker: name, at_ms: w.at_ms },
                    )
                  }
                >
                  <option value="speaker">Speaker</option>
                  <option value="place">Place</option>
                </TextField>
                <TextField
                  size="small"
                  label={`${isPlace ? 'Place' : 'Speaker'} ${i + 1}`}
                  value={name}
                  onChange={(e) =>
                    setPoint(
                      i,
                      isPlace ? { ...w, place: e.target.value } : { ...w, speaker: e.target.value },
                    )
                  }
                />
                <TextField
                  size="small"
                  type="number"
                  label={`Arrival ${i + 1} (ms)`}
                  value={w.at_ms ?? ''}
                  placeholder="Automatic"
                  inputProps={{ min: 0, step: 1 }}
                  onChange={(e) => setPoint(i, { ...w, at_ms: optionalNumber(e.target.value) })}
                />
                <Button
                  disabled={motion.path.length === 1}
                  aria-label={`Remove waypoint ${i + 1}`}
                  onClick={() => change({ ...motion, path: motion.path.filter((_, j) => j !== i) })}
                >
                  Remove
                </Button>
              </Stack>
            );
          })}
          <div>
            <Button onClick={() => change({ ...motion, path: [...motion.path, { speaker: '' }] })}>
              Add waypoint
            </Button>
          </div>
          <Stack direction={row} spacing={1}>
            <TextField
              size="small"
              type="number"
              label="Motion duration (ms)"
              value={motion.duration_ms ?? ''}
              inputProps={{ min: 0, step: 1 }}
              onChange={(e) => change({ ...motion, duration_ms: optionalNumber(e.target.value) })}
            />
            <TextField
              size="small"
              type="number"
              label="Handoff (%)"
              value={motion.handoff === undefined ? '' : Number((motion.handoff * 100).toFixed(4))}
              placeholder="100"
              inputProps={{ min: 0, max: 100, step: 'any' }}
              onChange={(e) =>
                change({
                  ...motion,
                  handoff: e.target.value === '' ? undefined : Number(e.target.value) / 100,
                })
              }
            />
            <TextField
              select
              size="small"
              SelectProps={{ native: true }}
              label="Easing"
              value={motion.easing ?? 'linear'}
              onChange={(e) =>
                change({ ...motion, easing: e.target.value as SpeakerMotion['easing'] })
              }
            >
              <option value="linear">Linear</option>
              <option value="ease_in">Ease in</option>
              <option value="ease_out">Ease out</option>
              <option value="ease_in_out">Ease in and out</option>
            </TextField>
            <TextField
              size="small"
              type="number"
              label="Source channel"
              value={motion.source_channel ?? ''}
              placeholder="Automatic"
              inputProps={{ min: 0, step: 1 }}
              onChange={(e) =>
                change({ ...motion, source_channel: optionalNumber(e.target.value) })
              }
            />
          </Stack>
        </>
      )}
    </Stack>
  );
}
