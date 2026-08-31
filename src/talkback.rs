// ABOUTME: Models an exclusive, expiring talkback lease with deterministic fail-closed transitions.
// ABOUTME: The control loop uses this state machine to mute the microphone when renewal or ownership stops.

use serde::Serialize;

const MIN_LEASE_MS: u64 = 250;
const MAX_LEASE_MS: u64 = 2_000;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Lease {
    id: String,
    owner_client_id: String,
    source_id: String,
    destination: String,
    gain_milli_db: i32,
    expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseError {
    InvalidClient,
    InvalidSource,
    InvalidDestination,
    InvalidDuration,
    AlreadyOwned { owner_client_id: String },
    NotOwner,
    LeaseNotFound,
}

impl std::fmt::Display for LeaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidClient => write!(f, "client_id is required"),
            Self::InvalidSource => write!(f, "source_id must be GM_MIC"),
            Self::InvalidDestination => write!(f, "destination is not allowlisted"),
            Self::InvalidDuration => write!(f, "lease_ms must be between 250 and 2000"),
            Self::AlreadyOwned { owner_client_id } => {
                write!(f, "talkback is owned by {owner_client_id}")
            }
            Self::NotOwner => write!(f, "talkback lease owner mismatch"),
            Self::LeaseNotFound => write!(f, "talkback lease was not found"),
        }
    }
}

impl std::error::Error for LeaseError {}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TalkbackStatus {
    pub state: String,
    pub applied_live: bool,
    pub lease_id: Option<String>,
    pub owner_client_id: Option<String>,
    pub source_id: Option<String>,
    pub destination: Option<String>,
    pub gain: f32,
    pub lease_expires_at_ms: Option<u64>,
    pub last_transition: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseTransition {
    Acquired {
        lease_id: String,
        expires_at_ms: u64,
    },
    Renewed {
        lease_id: String,
        expires_at_ms: u64,
    },
    Released {
        lease_id: String,
    },
    HardMuted,
    Expired {
        lease_id: String,
    },
}

pub struct TalkbackLease {
    active: Option<Lease>,
    counter: u64,
    last_transition: Option<String>,
    last_error: Option<String>,
}

impl Default for TalkbackLease {
    fn default() -> Self {
        Self {
            active: None,
            counter: 0,
            last_transition: None,
            last_error: None,
        }
    }
}

impl TalkbackLease {
    pub fn acquire(
        &mut self,
        client_id: &str,
        source_id: &str,
        destination: &str,
        gain: f32,
        lease_ms: u64,
        now_ms: u64,
    ) -> Result<(LeaseTransition, TalkbackStatus), LeaseError> {
        if client_id.trim().is_empty() {
            return Err(self.fail(LeaseError::InvalidClient));
        }
        if source_id != "GM_MIC" {
            return Err(self.fail(LeaseError::InvalidSource));
        }
        if !allowed_destination(destination) {
            return Err(self.fail(LeaseError::InvalidDestination));
        }
        if !(MIN_LEASE_MS..=MAX_LEASE_MS).contains(&lease_ms)
            || !gain.is_finite()
            || !(-60.0..=12.0).contains(&gain)
        {
            return Err(self.fail(LeaseError::InvalidDuration));
        }
        if let Some(active) = &mut self.active {
            if active.owner_client_id != client_id {
                let error = LeaseError::AlreadyOwned {
                    owner_client_id: active.owner_client_id.clone(),
                };
                self.last_error = Some(error.to_string());
                return Err(error);
            }
            active.expires_at_ms = now_ms.saturating_add(lease_ms);
            active.destination = destination.to_string();
            active.gain_milli_db = (gain * 1000.0).round() as i32;
            self.last_transition = Some("renewed".to_string());
            self.last_error = None;
            let id = active.id.clone();
            return Ok((
                LeaseTransition::Renewed {
                    lease_id: id,
                    expires_at_ms: active.expires_at_ms,
                },
                self.status(now_ms),
            ));
        }
        self.counter = self.counter.saturating_add(1);
        let lease = Lease {
            id: format!("lease-{:04}", self.counter),
            owner_client_id: client_id.to_string(),
            source_id: source_id.to_string(),
            destination: destination.to_string(),
            gain_milli_db: (gain * 1000.0).round() as i32,
            expires_at_ms: now_ms.saturating_add(lease_ms),
        };
        let lease_id = lease.id.clone();
        let expires_at_ms = lease.expires_at_ms;
        self.active = Some(lease);
        self.last_transition = Some("acquired".to_string());
        self.last_error = None;
        Ok((
            LeaseTransition::Acquired {
                lease_id,
                expires_at_ms,
            },
            self.status(now_ms),
        ))
    }

    pub fn release(
        &mut self,
        client_id: &str,
        lease_id: &str,
        now_ms: u64,
    ) -> Result<(LeaseTransition, TalkbackStatus), LeaseError> {
        let Some(active) = &self.active else {
            return Err(self.fail(LeaseError::LeaseNotFound));
        };
        if active.owner_client_id != client_id || active.id != lease_id {
            return Err(self.fail(LeaseError::NotOwner));
        }
        let id = active.id.clone();
        self.active = None;
        self.last_transition = Some("released".to_string());
        self.last_error = None;
        Ok((
            LeaseTransition::Released { lease_id: id },
            self.status(now_ms),
        ))
    }

    pub fn hard_mute(&mut self, now_ms: u64) -> (LeaseTransition, TalkbackStatus) {
        self.active = None;
        self.last_transition = Some("hard-muted".to_string());
        self.last_error = None;
        (LeaseTransition::HardMuted, self.status(now_ms))
    }

    pub fn expire(&mut self, now_ms: u64) -> Option<(LeaseTransition, TalkbackStatus)> {
        let active = self.active.as_ref()?;
        if now_ms < active.expires_at_ms {
            return None;
        }
        let id = active.id.clone();
        self.active = None;
        self.last_transition = Some("expired".to_string());
        self.last_error = None;
        Some((
            LeaseTransition::Expired { lease_id: id },
            self.status(now_ms),
        ))
    }

    pub fn active_for(&self, source_id: &str) -> bool {
        self.active
            .as_ref()
            .map(|lease| lease.source_id == source_id)
            .unwrap_or(false)
    }

    pub fn active_source(&self) -> Option<&str> {
        self.active.as_ref().map(|lease| lease.source_id.as_str())
    }

    pub fn status(&self, _now_ms: u64) -> TalkbackStatus {
        match &self.active {
            Some(active) => TalkbackStatus {
                state: "live".to_string(),
                applied_live: true,
                lease_id: Some(active.id.clone()),
                owner_client_id: Some(active.owner_client_id.clone()),
                source_id: Some(active.source_id.clone()),
                destination: Some(active.destination.clone()),
                gain: active.gain_milli_db as f32 / 1000.0,
                lease_expires_at_ms: Some(active.expires_at_ms),
                last_transition: self.last_transition.clone(),
                last_error: self.last_error.clone(),
            },
            None => TalkbackStatus {
                state: "muted".to_string(),
                applied_live: false,
                lease_id: None,
                owner_client_id: None,
                source_id: None,
                destination: None,
                gain: 0.0,
                lease_expires_at_ms: None,
                last_transition: self.last_transition.clone(),
                last_error: self.last_error.clone(),
            },
        }
    }

    fn fail(&mut self, error: LeaseError) -> LeaseError {
        self.last_error = Some(error.to_string());
        error
    }
}

fn allowed_destination(destination: &str) -> bool {
    matches!(
        destination,
        "GUEST_ALL" | "ROOM_1" | "ROOM_2A" | "ROOM_2B" | "ROOM_3" | "ROOM_4"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_is_exclusive_and_renews_for_same_owner() {
        let mut lease = TalkbackLease::default();
        let (first, status) = lease
            .acquire("gm-a", "GM_MIC", "GUEST_ALL", 0.0, 500, 0)
            .unwrap();
        assert_eq!(
            first,
            LeaseTransition::Acquired {
                lease_id: "lease-0001".to_string(),
                expires_at_ms: 500
            }
        );
        assert!(status.applied_live);
        let (renewed, _) = lease
            .acquire("gm-a", "GM_MIC", "ROOM_1", -3.0, 500, 250)
            .unwrap();
        assert_eq!(
            renewed,
            LeaseTransition::Renewed {
                lease_id: "lease-0001".to_string(),
                expires_at_ms: 750
            }
        );
        assert!(matches!(
            lease.acquire("gm-b", "GM_MIC", "GUEST_ALL", 0.0, 500, 250),
            Err(LeaseError::AlreadyOwned { .. })
        ));
    }

    #[test]
    fn lease_expires_and_hard_mute_fail_closed() {
        let mut lease = TalkbackLease::default();
        lease
            .acquire("gm-a", "GM_MIC", "GUEST_ALL", 0.0, 500, 0)
            .unwrap();
        assert!(lease.expire(499).is_none());
        let (expired, status) = lease.expire(500).unwrap();
        assert_eq!(
            expired,
            LeaseTransition::Expired {
                lease_id: "lease-0001".to_string()
            }
        );
        assert!(!status.applied_live);
        lease
            .acquire("gm-a", "GM_MIC", "GUEST_ALL", 0.0, 500, 500)
            .unwrap();
        let (muted, status) = lease.hard_mute(510);
        assert_eq!(muted, LeaseTransition::HardMuted);
        assert!(!status.applied_live);
    }

    #[test]
    fn lease_rejects_invalid_source_destination_duration_and_owner() {
        let mut lease = TalkbackLease::default();
        assert_eq!(
            lease
                .acquire("gm", "mic", "GUEST_ALL", 0.0, 500, 0)
                .unwrap_err(),
            LeaseError::InvalidSource
        );
        assert_eq!(
            lease
                .acquire("gm", "GM_MIC", "UNKNOWN", 0.0, 500, 0)
                .unwrap_err(),
            LeaseError::InvalidDestination
        );
        assert_eq!(
            lease
                .acquire("gm", "GM_MIC", "GUEST_ALL", 0.0, 100, 0)
                .unwrap_err(),
            LeaseError::InvalidDuration
        );
        lease
            .acquire("gm", "GM_MIC", "GUEST_ALL", 0.0, 500, 0)
            .unwrap();
        assert_eq!(
            lease.release("other", "lease-0001", 0).unwrap_err(),
            LeaseError::NotOwner
        );
    }
}
