// ABOUTME: Voice-activity detection for live inputs.
// ABOUTME: Threshold plus hold-time hysteresis turning capture levels into ducking triggers.

use std::time::{Duration, Instant};

/// Decides when a live input counts as "speaking" from its capture level.
///
/// A level at or above the threshold makes the input active immediately and
/// keeps it active until the level has stayed below the threshold for the
/// hold time, so ducking does not flutter in the pauses between words.
pub struct ActivityDetector {
    threshold: f32,
    hold: Duration,
    active: bool,
    active_until: Instant,
}

impl ActivityDetector {
    pub fn new(threshold: f32, hold_ms: u32, now: Instant) -> Self {
        Self {
            threshold,
            hold: Duration::from_millis(hold_ms as u64),
            active: false,
            active_until: now,
        }
    }

    /// Feed the peak level observed since the last update.
    /// Returns Some(true) on the silent-to-speaking transition, Some(false)
    /// when the hold expires, and None while the state is unchanged.
    pub fn update(&mut self, level: f32, now: Instant) -> Option<bool> {
        if level >= self.threshold {
            self.active_until = now + self.hold;
            if !self.active {
                self.active = true;
                return Some(true);
            }
            None
        } else if self.active && now >= self.active_until {
            self.active = false;
            Some(false)
        } else {
            None
        }
    }

    #[cfg(test)]
    pub fn is_active(&self) -> bool {
        self.active
    }
}

/// Whether one live input counts as active for ducking. With a threshold it
/// follows the capture level through an [`ActivityDetector`]; without one it is
/// active while its stream is open. A muted input counts as silent either way:
/// capture keeps running while muted (the mute is applied in the mix), so the
/// captured level alone would keep ducking the room.
pub struct InputActivity {
    detector: Option<ActivityDetector>,
    active: bool,
}

impl InputActivity {
    /// `threshold` is the activity level and hold time in milliseconds; `None`
    /// makes the input active whenever it is unmuted. Starts inactive.
    pub fn new(threshold: Option<(f32, u32)>, now: Instant) -> Self {
        Self {
            detector: threshold.map(|(level, hold_ms)| ActivityDetector::new(level, hold_ms, now)),
            active: false,
        }
    }

    /// Feed the peak capture level since the last update and whether the input is
    /// muted. Returns `Some(active)` when the input's activity changes.
    pub fn update(&mut self, level: f32, muted: bool, now: Instant) -> Option<bool> {
        let active = match &mut self.detector {
            Some(detector) => {
                detector.update(if muted { 0.0 } else { level }, now);
                detector.active
            }
            None => !muted,
        };
        if active == self.active {
            return None;
        }
        self.active = active;
        Some(active)
    }

    #[cfg(test)]
    pub fn is_active(&self) -> bool {
        self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn test_rising_edge_fires_once() {
        let now = t0();
        let mut det = ActivityDetector::new(0.1, 500, now);

        assert_eq!(det.update(0.5, now), Some(true));
        assert_eq!(det.update(0.5, now + Duration::from_millis(50)), None);
        assert!(det.is_active());
    }

    #[test]
    fn test_below_threshold_never_activates() {
        let now = t0();
        let mut det = ActivityDetector::new(0.1, 500, now);

        assert_eq!(det.update(0.05, now), None);
        assert_eq!(det.update(0.09, now + Duration::from_millis(100)), None);
        assert!(!det.is_active());
    }

    #[test]
    fn test_stays_active_through_short_pauses() {
        let now = t0();
        let mut det = ActivityDetector::new(0.1, 500, now);

        det.update(0.5, now);
        // Silence for less than the hold time: still active
        assert_eq!(det.update(0.0, now + Duration::from_millis(200)), None);
        assert_eq!(det.update(0.0, now + Duration::from_millis(400)), None);
        assert!(det.is_active());
    }

    #[test]
    fn test_deactivates_after_hold_expires() {
        let now = t0();
        let mut det = ActivityDetector::new(0.1, 500, now);

        det.update(0.5, now);
        assert_eq!(
            det.update(0.0, now + Duration::from_millis(600)),
            Some(false)
        );
        assert!(!det.is_active());
        // And it does not fire again while silent
        assert_eq!(det.update(0.0, now + Duration::from_millis(700)), None);
    }

    #[test]
    fn test_speech_extends_the_hold() {
        let now = t0();
        let mut det = ActivityDetector::new(0.1, 500, now);

        det.update(0.5, now);
        // Speaking again at 400ms pushes the hold out to 900ms
        assert_eq!(det.update(0.5, now + Duration::from_millis(400)), None);
        assert_eq!(det.update(0.0, now + Duration::from_millis(700)), None);
        assert!(det.is_active());
        assert_eq!(
            det.update(0.0, now + Duration::from_millis(950)),
            Some(false)
        );
    }

    #[test]
    fn a_muted_input_with_a_threshold_does_not_activate() {
        let now = t0();
        let mut input = InputActivity::new(Some((0.1, 500)), now);

        assert_eq!(input.update(0.9, true, now), None);
        assert!(!input.is_active());
    }

    #[test]
    fn muting_a_speaking_input_releases_it_after_the_hold() {
        let now = t0();
        let mut input = InputActivity::new(Some((0.1, 500)), now);

        assert_eq!(input.update(0.9, false, now), Some(true));
        // Still loud in the room, but muted: it holds, then releases.
        assert_eq!(
            input.update(0.9, true, now + Duration::from_millis(200)),
            None
        );
        assert_eq!(
            input.update(0.9, true, now + Duration::from_millis(600)),
            Some(false)
        );
    }

    #[test]
    fn an_input_without_a_threshold_is_active_while_open_and_unmuted() {
        let now = t0();
        let mut input = InputActivity::new(None, now);

        assert_eq!(input.update(0.0, false, now), Some(true));
        assert_eq!(input.update(0.0, false, now), None);
        assert_eq!(input.update(0.0, true, now), Some(false));
        assert_eq!(input.update(0.0, true, now), None);
        assert_eq!(input.update(0.0, false, now), Some(true));
    }

    #[test]
    fn test_reactivates_after_silence() {
        let now = t0();
        let mut det = ActivityDetector::new(0.1, 500, now);

        det.update(0.5, now);
        det.update(0.0, now + Duration::from_millis(600));
        assert_eq!(
            det.update(0.5, now + Duration::from_millis(1000)),
            Some(true)
        );
    }
}
