// ABOUTME: Resolves cue waypoints, channel aliases and installation places before playback.
// ABOUTME: Converts millisecond timing marks into a point source on the output sample clock.

use crate::audio::panner::{Easing, Panner, PointSource, SpeakerMix};
use crate::config::{ChannelRef, Config};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Waypoint {
    Target(ChannelRef),
    Speaker {
        speaker: ChannelRef,
        at_ms: Option<u32>,
    },
    Place {
        place: String,
        at_ms: Option<u32>,
    },
}

impl Waypoint {
    fn time(&self) -> Option<u32> {
        match self {
            Self::Target(_) => None,
            Self::Speaker { at_ms, .. } | Self::Place { at_ms, .. } => *at_ms,
        }
    }
}

fn full_handoff() -> f64 {
    1.0
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Motion {
    pub path: Vec<Waypoint>,
    pub duration_ms: Option<u32>,
    #[serde(default = "full_handoff")]
    pub handoff: f64,
    #[serde(default)]
    pub easing: Easing,
    pub source_channel: Option<usize>,
}

impl Motion {
    /// Untimed interior points divide a span. The first point is always at zero.
    pub(crate) fn timing_marks(&self) -> Result<Vec<(usize, u32)>, String> {
        if !(0.0..=1.0).contains(&self.handoff) {
            return Err("motion.handoff must be between 0 and 1".into());
        }
        let first = self
            .path
            .first()
            .ok_or("motion.path must have at least one waypoint")?;
        if first.time().unwrap_or(0) != 0 {
            return Err("the first motion waypoint must start at 0 ms".into());
        }
        let mut marks = vec![(0, 0)];
        let last = self.path.len() - 1;
        if last == 0 {
            return Ok(marks);
        }
        let end_time = self.path[last].time();
        let duration = self
            .duration_ms
            .or(end_time)
            .ok_or("motion requires duration_ms or a timed final waypoint")?;
        if end_time.is_some_and(|at| at != duration) {
            return Err("the last motion waypoint must match duration_ms".into());
        }
        for (index, point) in self.path.iter().enumerate().skip(1) {
            let at = if index == last {
                Some(duration)
            } else {
                point.time()
            };
            if let Some(at) = at {
                if at <= marks.last().unwrap().1 {
                    return Err("motion waypoint times must increase strictly".into());
                }
                marks.push((index, at));
            }
        }
        Ok(marks)
    }

    /// Resolve a playable cue against the negotiated device and decoded file.
    pub fn prepare(
        &self,
        config: &Config,
        outputs: usize,
        rate: u32,
        inputs: usize,
    ) -> Result<PointSource, String> {
        if rate == 0 {
            return Err("motion requires a positive output sample rate".into());
        }
        let channel = match (self.source_channel, inputs) {
            (Some(ch), n) if ch < n => Some(ch),
            (Some(ch), n) => {
                return Err(format!(
                    "motion.source_channel {ch} is outside the {n}-channel file"
                ))
            }
            (None, 1) => Some(0),
            (None, 2) => None,
            (None, _) => {
                return Err(format!(
                    "a {inputs}-channel file needs an explicit motion.source_channel"
                ))
            }
        };
        let marks = self
            .timing_marks()?
            .into_iter()
            .map(|(i, ms)| (i, (u64::from(ms) * u64::from(rate) + 500) / 1000))
            .collect();
        let points = self
            .path
            .iter()
            .enumerate()
            .map(|(i, target)| {
                let mix = match target {
                    Waypoint::Place { place, .. } => resolve_place(config, place, Some(outputs)),
                    Waypoint::Target(ChannelRef::Alias(name))
                        if config.audio.places.contains_key(name) =>
                    {
                        resolve_place(config, name, Some(outputs))
                    }
                    Waypoint::Target(speaker) | Waypoint::Speaker { speaker, .. } => {
                        resolve_output(config, speaker, Some(outputs)).map(SpeakerMix::speaker)
                    }
                };
                mix.map_err(|e| format!("motion.path[{i}]: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PointSource {
            panner: Panner::new(points, marks, self.handoff, self.easing),
            channel,
        })
    }
}

fn resolve_output(
    config: &Config,
    speaker: &ChannelRef,
    outputs: Option<usize>,
) -> Result<usize, String> {
    let ch = config.resolve_channel(speaker)?;
    if let Some(n) = outputs {
        if ch >= n {
            return Err(format!(
                "speaker output {ch} is outside the {n} available channels"
            ));
        }
    }
    Ok(ch)
}

pub(crate) fn resolve_place(
    config: &Config,
    name: &str,
    outputs: Option<usize>,
) -> Result<SpeakerMix, String> {
    if name.trim().is_empty()
        || name.parse::<usize>().is_ok()
        || config.audio.channel_aliases.contains_key(name)
    {
        return Err(
            "a place needs a nonempty name distinct from speaker numbers and aliases".into(),
        );
    }
    let place = config
        .audio
        .places
        .get(name)
        .ok_or_else(|| format!("unknown place '{name}'"))?;
    let mut weights = BTreeMap::new();
    for (speaker, &weight) in place {
        let ch = resolve_output(config, &ChannelRef::from_key(speaker), outputs)?;
        if weights.insert(ch, weight).is_some() {
            return Err(format!("place '{name}' specifies output {ch} twice"));
        }
    }
    SpeakerMix::from_weights(weights)
}
