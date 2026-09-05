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
