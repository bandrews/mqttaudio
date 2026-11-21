// ABOUTME: Renders a mono point source through timed output gain mixes.
// ABOUTME: The audio clock drives movement independently of the source transport.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Relative amplitudes on physical outputs, normalized to unit power.
#[derive(Debug)]
pub struct SpeakerMix(Vec<(usize, f32)>);

impl SpeakerMix {
    pub(crate) fn speaker(channel: usize) -> Self {
        Self(vec![(channel, 1.0)])
    }

    pub(crate) fn from_weights(weights: BTreeMap<usize, f32>) -> Result<Self, String> {
        if weights.values().any(|w| !w.is_finite() || *w < 0.0) {
            return Err("speaker weights must be finite and nonnegative".into());
        }
        let norm = weights
            .values()
            .map(|&w| f64::from(w).powi(2))
            .sum::<f64>()
            .sqrt();
        if norm == 0.0 {
            return Err("at least one speaker must have positive weight".into());
        }
        Ok(Self(
            weights
                .into_iter()
                .filter(|(_, w)| *w > 0.0)
                .map(|(channel, w)| (channel, (f64::from(w) / norm) as f32))
                .collect(),
        ))
    }

    fn dot(&self, other: &Self) -> f32 {
        self.0
            .iter()
            .filter_map(|&(channel, gain)| {
                other
                    .0
                    .binary_search_by_key(&channel, |&(ch, _)| ch)
                    .ok()
                    .map(|index| gain * other.0[index].1)
            })
            .sum()
    }

    fn mix(&self, sample: f32, output: &mut [f32]) {
        for &(channel, gain) in &self.0 {
            if let Some(dest) = output.get_mut(channel) {
                *dest += sample * gain;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Easing {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl Easing {
    fn progress(self, t: f64) -> f64 {
        match self {
            Self::Linear => t,
            Self::EaseIn => t * t,
            Self::EaseOut => t * (2.0 - t),
            Self::EaseInOut => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// Keyframes and timestamps are supplied in output space. The renderer has no
/// dependency on how an authoring system chooses those speaker mixes.
#[derive(Debug)]
pub struct Panner {
    points: Vec<SpeakerMix>,
    shared_power: Vec<f32>,
    /// (point index, output frame) for each explicit timing mark, including the endpoints.
    marks: Vec<(usize, u64)>,
    span: usize,
    elapsed: u64,
    handoff: f64,
    easing: Easing,
}

impl Panner {
    pub(crate) fn new(
        points: Vec<SpeakerMix>,
        marks: Vec<(usize, u64)>,
        handoff: f64,
        easing: Easing,
    ) -> Self {
        let shared_power = points.windows(2).map(|p| p[0].dot(&p[1])).collect();
        Self {
            points,
            marks,
            shared_power,
            span: 0,
            elapsed: 0,
            handoff,
            easing,
        }
    }

    /// A starved source keeps show time once it has mixed its first source frame.
    pub(crate) fn skip_silence(&mut self, frames: usize) {
        if self.started() {
            self.elapsed = self.elapsed.saturating_add(frames as u64);
        }
    }

    pub(crate) fn started(&self) -> bool {
        self.elapsed > 0
    }

    fn mix(&mut self, mono: f32, output: &mut [f32]) {
        let frame = self.elapsed;
        self.elapsed = frame.saturating_add(1);
        while let Some(&(_, end)) = self.marks.get(self.span + 1) {
            if frame < end {
                break;
            }
            self.span += 1;
        }
        let (first, start) = self.marks[self.span];
        let Some(&(last, end)) = self.marks.get(self.span + 1) else {
            self.points[first].mix(mono, output);
            return;
        };
        let time = (frame - start) as f64 / (end - start) as f64;
        let position = self.easing.progress(time) * (last - first) as f64;
        let offset = (position as usize).min(last - first - 1);
        let leg = first + offset;
        let t = position - offset as f64;
        let blend = if self.handoff == 0.0 {
            f64::from(t >= 0.5)
        } else {
            ((t - (1.0 - self.handoff) * 0.5) / self.handoff).clamp(0.0, 1.0)
        };
        if blend == 0.0 {
            self.points[leg].mix(mono, output);
        } else if blend == 1.0 {
            self.points[leg + 1].mix(mono, output);
        } else {
            let (b, a) = (blend as f32 * std::f32::consts::FRAC_PI_2).sin_cos();
            // ||a*A + b*B||² includes a cross term when both places use an output.
            let level = mono / (1.0 + 2.0 * a * b * self.shared_power[leg]).sqrt();
            self.points[leg].mix(level * a, output);
            self.points[leg + 1].mix(level * b, output);
        }
    }
}

#[derive(Debug)]
pub struct PointSource {
    pub(crate) panner: Panner,
    /// None folds a stereo pair. Mono and explicitly selected channels use Some.
    pub(crate) channel: Option<usize>,
}

impl PointSource {
    pub(crate) fn mix(
        &mut self,
        gain: f32,
        output: &mut [f32],
        channels: usize,
        read: impl Fn(usize) -> f32,
    ) {
        let mono = match self.channel {
            Some(ch) if ch < channels => read(ch),
            None if channels >= 2 => 0.5 * read(0) + 0.5 * read(1),
            // Progressive buffer metadata can be temporarily unavailable to the callback.
            _ => {
                self.panner.skip_silence(1);
                return;
            }
        };
        self.panner.mix(mono * gain, output);
    }
}
