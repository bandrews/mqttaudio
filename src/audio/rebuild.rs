// ABOUTME: Exponential-backoff policy for rebuilding a failed audio stream.
// ABOUTME: Pure state machine; the actual rebuild lives in the stream supervisor.

use std::time::Duration;

/// Exponential backoff for stream-rebuild attempts: 250ms, 500ms, 1s, 2s, 4s,
/// 5s (capped), then exhausted. Reset after a successful (re)build so the next
/// failure starts the sequence over.
pub struct RebuildPolicy {
    attempt: u32,
    base: Duration,
    cap: Duration,
    max_attempts: u32,
}

impl RebuildPolicy {
    pub fn new() -> Self {
        Self {
            attempt: 0,
            base: Duration::from_millis(250),
            cap: Duration::from_secs(5),
            max_attempts: 6,
        }
    }

    /// The delay to wait before the next attempt, or `None` once attempts are
    /// exhausted (the caller should then give up / exit for a supervisor restart).
    pub fn next_delay(&mut self) -> Option<Duration> {
        if self.attempt >= self.max_attempts {
            return None;
        }
        let shift = self.attempt;
        self.attempt += 1;
        let millis = (self.base.as_millis() as u64).saturating_mul(2u64.saturating_pow(shift));
        Some(Duration::from_millis(millis).min(self.cap))
    }

    /// Reset after a successful build so the next failure restarts the backoff.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

impl Default for RebuildPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_sequence_caps_then_exhausts() {
        let mut p = RebuildPolicy::new();
        assert_eq!(p.next_delay(), Some(Duration::from_millis(250)));
        assert_eq!(p.next_delay(), Some(Duration::from_millis(500)));
        assert_eq!(p.next_delay(), Some(Duration::from_millis(1000)));
        assert_eq!(p.next_delay(), Some(Duration::from_millis(2000)));
        assert_eq!(p.next_delay(), Some(Duration::from_millis(4000)));
        assert_eq!(p.next_delay(), Some(Duration::from_secs(5))); // capped
        assert_eq!(p.next_delay(), None); // exhausted after 6 attempts
        assert_eq!(p.next_delay(), None); // stays exhausted
    }

    #[test]
    fn reset_restarts_backoff() {
        let mut p = RebuildPolicy::new();
        p.next_delay();
        p.next_delay();
        p.reset();
        assert_eq!(p.next_delay(), Some(Duration::from_millis(250)));
    }
}

/// CPAL has already recovered xruns and default-device route changes. Rebuilding
/// those streams would interrupt working audio; permanent errors need the supervisor.
pub fn requires_rebuild(kind: cpal::ErrorKind) -> bool {
    !matches!(
        kind,
        cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied
    )
}

#[cfg(test)]
mod error_tests {
    #[test]
    fn recovered_events_keep_the_stream_and_permanent_errors_rebuild() {
        for kind in [
            cpal::ErrorKind::Xrun,
            cpal::ErrorKind::DeviceChanged,
            cpal::ErrorKind::RealtimeDenied,
        ] {
            assert!(!super::requires_rebuild(kind));
        }
        for kind in [
            cpal::ErrorKind::DeviceNotAvailable,
            cpal::ErrorKind::StreamInvalidated,
            cpal::ErrorKind::BackendError,
        ] {
            assert!(super::requires_rebuild(kind));
        }
    }
}
