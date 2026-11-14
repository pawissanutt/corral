//! In-memory, single-use browser pairing offers. No store work happens under this lock.
use std::{
    collections::BTreeMap,
    sync::{Condvar, Mutex},
    time::{Duration, Instant},
};

use corral_protocol::device::{DeviceInfo, PairState};
use jiff::{SignedDuration, Timestamp};

pub(crate) const OFFER_TTL: Duration = Duration::from_secs(120);
pub(crate) const CONFIRM_TTL: Duration = Duration::from_secs(120);
const MAX_PENDING: usize = 4;
const ENDED_KEEP: Duration = Duration::from_secs(60);
const STATUS_WAIT_MAX: Duration = Duration::from_secs(25);

#[derive(Default)]
pub(crate) struct Pairings {
    inner: Mutex<BTreeMap<String, Pending>>,
    changed: Condvar,
}

struct Pending {
    token: [u8; 32],
    stage: Stage,
    deadline: Instant,
    expires_at: Timestamp,
    ended_at: Option<Instant>,
}

enum Stage {
    Offered,
    Handshaken {
        public: [u8; 32],
        sas: String,
    },
    Claimed {
        public: [u8; 32],
        sas: String,
        name: String,
        app_version: String,
    },
    // Keep the claimed presentation stable while the caller commits the record.
    Confirming {
        sas: String,
        name: String,
        app_version: String,
    },
    Paired(DeviceInfo),
    Ended(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairError {
    NotFound,
    Expired,
    Conflict(&'static str),
    Limit,
}

impl Pairings {
    pub(crate) fn begin(
        &self,
        pair_id: String,
        token: [u8; 32],
        now: Instant,
    ) -> Result<(), PairError> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        sweep(&mut inner, now, &self.changed);
        if inner.contains_key(&pair_id) {
            return Err(PairError::Conflict("pairing_already_exists"));
        }
        if inner
            .values()
            .filter(|pending| is_pending(&pending.stage))
            .count()
            >= MAX_PENDING
        {
            return Err(PairError::Limit);
        }
        inner.insert(
            pair_id,
            Pending {
                token,
                stage: Stage::Offered,
                deadline: now + OFFER_TTL,
                expires_at: expires_at(OFFER_TTL),
                ended_at: None,
            },
        );
        self.changed.notify_all();
        Ok(())
    }

    pub(crate) fn token(&self, pair_id: &str, now: Instant) -> Result<[u8; 32], PairError> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        sweep(&mut inner, now, &self.changed);
        let pending = inner.get(pair_id).ok_or(PairError::NotFound)?;
        match &pending.stage {
            Stage::Offered => Ok(pending.token),
            Stage::Ended(_) => Err(PairError::Expired),
            _ => Err(PairError::NotFound),
        }
    }

    pub(crate) fn handshaken(
        &self,
        pair_id: &str,
        public: [u8; 32],
        sas: String,
        now: Instant,
    ) -> Result<(), PairError> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        sweep(&mut inner, now, &self.changed);
        let pending = inner.get_mut(pair_id).ok_or(PairError::NotFound)?;
        if !matches!(pending.stage, Stage::Offered) {
            return Err(PairError::Conflict("pairing_already_claimed"));
        }
        pending.stage = Stage::Handshaken { public, sas };
        pending.deadline = now + CONFIRM_TTL;
        pending.expires_at = expires_at(CONFIRM_TTL);
        self.changed.notify_all();
        Ok(())
    }

    pub(crate) fn claim(
        &self,
        pair_id: &str,
        name: String,
        app_version: String,
        now: Instant,
    ) -> Result<String, PairError> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        sweep(&mut inner, now, &self.changed);
        let pending = inner.get_mut(pair_id).ok_or(PairError::NotFound)?;
        let (public, sas) = match &mut pending.stage {
            Stage::Handshaken { public, sas } => (*public, std::mem::take(sas)),
            Stage::Ended(_) => return Err(PairError::Expired),
            _ => return Err(PairError::Conflict("pairing_already_claimed")),
        };
        pending.stage = Stage::Claimed {
            public,
            sas: sas.clone(),
            name,
            app_version,
        };
        self.changed.notify_all();
        Ok(sas)
    }

    pub(crate) fn begin_confirm(
        &self,
        pair_id: &str,
        now: Instant,
    ) -> Result<([u8; 32], String, String), PairError> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        sweep(&mut inner, now, &self.changed);
        let pending = inner.get_mut(pair_id).ok_or(PairError::NotFound)?;
        let (result, confirming) = match &mut pending.stage {
            Stage::Claimed {
                public,
                sas,
                name,
                app_version,
            } => (
                (*public, name.clone(), app_version.clone()),
                Stage::Confirming {
                    sas: std::mem::take(sas),
                    name: std::mem::take(name),
                    app_version: std::mem::take(app_version),
                },
            ),
            Stage::Offered | Stage::Handshaken { .. } => {
                return Err(PairError::Conflict("pairing_not_claimed"));
            }
            Stage::Ended(_) => return Err(PairError::Expired),
            _ => return Err(PairError::Conflict("pairing_already_claimed")),
        };
        pending.stage = confirming;
        // Confirming remains wire-visible as Claimed, so status waiters need not wake.
        Ok(result)
    }

    pub(crate) fn finish(&self, pair_id: &str, result: Result<DeviceInfo, ()>) {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        sweep(&mut inner, now, &self.changed);
        let Some(pending) = inner.get_mut(pair_id) else {
            return;
        };
        if !matches!(pending.stage, Stage::Confirming { .. }) {
            return;
        }
        pending.stage = match result {
            Ok(device) => Stage::Paired(device),
            Err(()) => Stage::Ended("failed"),
        };
        pending.ended_at = Some(now);
        self.changed.notify_all();
    }

    pub(crate) fn cancel(&self, pair_id: &str, now: Instant) -> Result<(), PairError> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        sweep(&mut inner, now, &self.changed);
        let pending = inner.get_mut(pair_id).ok_or(PairError::NotFound)?;
        if matches!(pending.stage, Stage::Confirming { .. }) {
            return Err(PairError::Conflict("pairing_confirming"));
        }
        if is_pending(&pending.stage) {
            pending.stage = Stage::Ended("cancelled");
            pending.ended_at = Some(now);
            self.changed.notify_all();
        }
        Ok(())
    }

    pub(crate) fn gone(&self, pair_id: &str) {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        sweep(&mut inner, now, &self.changed);
        let Some(pending) = inner.get_mut(pair_id) else {
            return;
        };
        if is_pending(&pending.stage) {
            pending.stage = Stage::Ended("closed");
            pending.ended_at = Some(now);
            self.changed.notify_all();
        }
    }

    pub(crate) fn status(
        &self,
        pair_id: &str,
        now: Instant,
        wait: Duration,
    ) -> Result<PairState, PairError> {
        let started = Instant::now();
        let wait = wait.min(STATUS_WAIT_MAX);
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        sweep(&mut inner, now, &self.changed);
        let pending = inner.get(pair_id).ok_or(PairError::NotFound)?;
        let initial = wire_state(pending);
        let initial_expiry = pending.expires_at;
        if wait.is_zero() {
            return Ok(initial);
        }
        loop {
            let elapsed = started.elapsed();
            let current = now + elapsed;
            sweep(&mut inner, current, &self.changed);
            let pending = inner.get(pair_id).ok_or(PairError::NotFound)?;
            if !same_wire_state(pending, &initial, initial_expiry) {
                return Ok(wire_state(pending));
            }
            let remaining = wait.saturating_sub(elapsed);
            if remaining.is_zero() {
                return Ok(initial);
            }
            let transition = if is_pending(&pending.stage) {
                Some(pending.deadline)
            } else {
                pending.ended_at.map(|ended| ended + ENDED_KEEP)
            };
            let timeout = transition.map_or(remaining, |deadline| {
                remaining.min(deadline.saturating_duration_since(current))
            });
            (inner, _) = self
                .changed
                .wait_timeout(inner, timeout)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

fn is_pending(stage: &Stage) -> bool {
    matches!(
        stage,
        Stage::Offered | Stage::Handshaken { .. } | Stage::Claimed { .. }
    )
}

fn expires_at(ttl: Duration) -> Timestamp {
    Timestamp::now() + SignedDuration::from_secs(ttl.as_secs() as i64)
}

fn sweep(inner: &mut BTreeMap<String, Pending>, now: Instant, changed: &Condvar) {
    let mut swept = false;
    inner.retain(|_, pending| {
        if is_pending(&pending.stage) && now >= pending.deadline {
            pending.stage = Stage::Ended("expired");
            pending.ended_at = Some(pending.deadline);
            swept = true;
        }
        let keep = pending
            .ended_at
            .is_none_or(|ended| now.saturating_duration_since(ended) < ENDED_KEEP);
        swept |= !keep;
        keep
    });
    if swept {
        changed.notify_all();
    }
}

fn wire_state(pending: &Pending) -> PairState {
    match &pending.stage {
        Stage::Offered | Stage::Handshaken { .. } => PairState::Offered {
            expires_at: pending.expires_at.to_string(),
        },
        Stage::Claimed {
            sas,
            name,
            app_version,
            ..
        }
        | Stage::Confirming {
            sas,
            name,
            app_version,
        } => PairState::Claimed {
            name: name.clone(),
            app_version: app_version.clone(),
            sas: sas.clone(),
            expires_at: pending.expires_at.to_string(),
        },
        Stage::Paired(device) => PairState::Paired {
            device: device.clone(),
        },
        Stage::Ended(reason) => PairState::Ended {
            reason: (*reason).into(),
        },
    }
}

fn same_wire_state(pending: &Pending, state: &PairState, expiry: Timestamp) -> bool {
    match (&pending.stage, state) {
        (Stage::Offered | Stage::Handshaken { .. }, PairState::Offered { .. }) => {
            pending.expires_at == expiry
        }
        (
            Stage::Claimed {
                sas,
                name,
                app_version,
                ..
            }
            | Stage::Confirming {
                sas,
                name,
                app_version,
            },
            PairState::Claimed {
                sas: old_sas,
                name: old_name,
                app_version: old_version,
                ..
            },
        ) => {
            sas == old_sas
                && name == old_name
                && app_version == old_version
                && pending.expires_at == expiry
        }
        (Stage::Paired(device), PairState::Paired { device: old }) => device == old,
        (Stage::Ended(reason), PairState::Ended { reason: old }) => *reason == old,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_protocol::device::DevicePlatform;

    fn offer(pairings: &Pairings, now: Instant) {
        pairings.begin("pair".into(), [7; 32], now).unwrap();
    }

    fn handshake(pairings: &Pairings, now: Instant) {
        pairings
            .handshaken("pair", [3; 32], "123 456".into(), now)
            .unwrap();
    }

    fn claim(pairings: &Pairings, now: Instant) {
        pairings
            .claim("pair", "Browser".into(), "1.0".into(), now)
            .unwrap();
    }

    fn device() -> DeviceInfo {
        DeviceInfo {
            id: "dev-0123456789abcdef".into(),
            name: "Browser".into(),
            platform: DevicePlatform::Web,
            app_version: "1.0".into(),
            enrolled_at: "2026-01-01T00:00:00Z".into(),
            revoked_at: None,
        }
    }

    #[test]
    fn second_claimant_cannot_reuse_a_burned_token() {
        let pairings = Pairings::default();
        let now = Instant::now();
        offer(&pairings, now);
        // Both sockets can fetch the token before either finishes its handshake.
        assert_eq!(pairings.token("pair", now), Ok([7; 32]));
        assert_eq!(pairings.token("pair", now), Ok([7; 32]));
        handshake(&pairings, now);
        assert_eq!(pairings.token("pair", now), Err(PairError::NotFound));
        assert_eq!(
            pairings.handshaken("pair", [9; 32], "999 999".into(), now),
            Err(PairError::Conflict("pairing_already_claimed")),
        );
        assert_eq!(
            pairings.claim("pair", "Browser".into(), "1.0".into(), now),
            Ok("123 456".into()),
        );
        assert_eq!(
            pairings.claim("pair", "Intruder".into(), "2.0".into(), now),
            Err(PairError::Conflict("pairing_already_claimed")),
        );
        assert_eq!(
            pairings.begin_confirm("pair", now),
            Ok(([3; 32], "Browser".into(), "1.0".into())),
        );
    }

    #[test]
    fn offer_and_claim_expire_after_their_ttl() {
        let now = Instant::now();
        let offers = Pairings::default();
        offer(&offers, now);
        assert_eq!(
            offers.token("pair", now + OFFER_TTL - Duration::from_nanos(1)),
            Ok([7; 32])
        );
        assert_eq!(
            offers.token("pair", now + OFFER_TTL),
            Err(PairError::Expired)
        );
        assert_eq!(
            offers.status("pair", now + OFFER_TTL, Duration::ZERO),
            Ok(PairState::Ended {
                reason: "expired".into()
            }),
        );
        assert_eq!(
            offers.status("pair", now + OFFER_TTL + ENDED_KEEP, Duration::ZERO),
            Err(PairError::NotFound),
        );

        let claims = Pairings::default();
        offer(&claims, now);
        let handshaken_at = now + OFFER_TTL - Duration::from_secs(1);
        handshake(&claims, handshaken_at);
        let deadline = handshaken_at + CONFIRM_TTL;
        claim(&claims, deadline - Duration::from_nanos(1));
        assert!(matches!(
            claims.status("pair", deadline - Duration::from_nanos(1), Duration::ZERO),
            Ok(PairState::Claimed { .. }),
        ));
        assert_eq!(
            claims.begin_confirm("pair", deadline),
            Err(PairError::Expired)
        );

        let unclaimed = Pairings::default();
        offer(&unclaimed, now);
        handshake(&unclaimed, now);
        assert_eq!(
            unclaimed.claim("pair", "Browser".into(), "1.0".into(), now + CONFIRM_TTL),
            Err(PairError::Expired),
        );
        for index in 0..MAX_PENDING {
            unclaimed
                .begin(format!("other-{index}"), [0; 32], now + CONFIRM_TTL)
                .unwrap();
        }
        assert_eq!(
            unclaimed.begin("overflow".into(), [0; 32], now + CONFIRM_TTL),
            Err(PairError::Limit),
        );
        unclaimed
            .begin(
                "after-expiry".into(),
                [0; 32],
                now + CONFIRM_TTL + OFFER_TTL,
            )
            .unwrap();
    }

    #[test]
    fn confirm_refuses_until_claimed_and_after_cancel() {
        let pairings = Pairings::default();
        let now = Instant::now();
        offer(&pairings, now);
        assert_eq!(
            pairings.begin_confirm("pair", now),
            Err(PairError::Conflict("pairing_not_claimed")),
        );
        handshake(&pairings, now);
        assert_eq!(
            pairings.begin_confirm("pair", now),
            Err(PairError::Conflict("pairing_not_claimed")),
        );
        claim(&pairings, now);
        pairings.cancel("pair", now).unwrap();
        pairings.cancel("pair", now).unwrap();
        assert_eq!(pairings.begin_confirm("pair", now), Err(PairError::Expired));
        // A late/disconnected claimant cannot overwrite an existing terminal result.
        pairings.gone("pair");
        assert_eq!(
            pairings.status("pair", now, Duration::ZERO),
            Ok(PairState::Ended {
                reason: "cancelled".into()
            }),
        );

        let confirming = Pairings::default();
        offer(&confirming, now);
        handshake(&confirming, now);
        claim(&confirming, now);
        let before = confirming.status("pair", now, Duration::ZERO).unwrap();
        confirming.begin_confirm("pair", now).unwrap();
        assert_eq!(
            confirming.cancel("pair", now),
            Err(PairError::Conflict("pairing_confirming"))
        );
        confirming.gone("pair");
        assert_eq!(
            confirming.status("pair", now + CONFIRM_TTL + ENDED_KEEP, Duration::ZERO),
            Ok(before),
        );
        confirming.finish("pair", Ok(device()));
        assert_eq!(
            confirming.status("pair", now, Duration::ZERO),
            Ok(PairState::Paired { device: device() }),
        );
        confirming.cancel("pair", now).unwrap();

        let failed = Pairings::default();
        offer(&failed, now);
        handshake(&failed, now);
        claim(&failed, now);
        failed.begin_confirm("pair", now).unwrap();
        failed.finish("pair", Err(()));
        assert_eq!(
            failed.status("pair", now, Duration::ZERO),
            Ok(PairState::Ended {
                reason: "failed".into()
            }),
        );
    }
}
