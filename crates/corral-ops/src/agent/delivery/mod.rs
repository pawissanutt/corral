//! The `pi_native` inbox adapter: send is bridge `submit_input` with the envelope idempotency
//! key, observe reads a host-written `corral_delivery` session entry.
//! Owner: T-252 child inbox-adapter. Until it lands the registered adapter is unavailable, so a
//! message to a pi-native recipient stays queued and is never sent.
use anyhow::Result;
use corral_core::inbox::{Envelope, ObserveCursor, SendOutcome, SessionBinding};

use crate::Ctx;
use crate::delivery::{
    BindOutcome, Capabilities, DeliveryAdapter, Observation, Readiness, RecipientRef,
};

/// Adapter name recorded on every pi-native envelope.
pub const ADAPTER: &str = "pi_native";
/// The T-252 child that lands the adapter.
pub const OWNER: &str = "inbox-adapter";

/// The registered pi-native adapter before inbox-adapter lands: never ready, never sends.
pub struct Unavailable;

impl Unavailable {
    pub fn reason() -> String {
        format!("delivery_unavailable, owner {OWNER}")
    }
}

impl DeliveryAdapter for Unavailable {
    fn harness(&self) -> &'static str {
        ADAPTER
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            idempotent_retry: false,
            admission: false,
            non_waking: false,
            max_payload: corral_core::inbox::MAX_PAYLOAD,
            send_budget: None,
            transcript_reader: false,
        }
    }

    fn readiness(&self, _ctx: &Ctx, _recipient: &RecipientRef) -> Result<Readiness> {
        Ok(Readiness::Unavailable {
            reason: Self::reason(),
        })
    }

    /// It can neither confirm nor deny a session, so it never answers Unbound: that would
    /// orphan an attempted envelope of a live recipient. Delivery blocks and keeps the message.
    fn bind(&self, _ctx: &Ctx, _recipient: &RecipientRef) -> Result<BindOutcome> {
        Ok(BindOutcome::Ambiguous {
            candidates: vec![Self::reason()],
        })
    }

    fn send(&self, _ctx: &Ctx, _binding: &SessionBinding, _envelope: &Envelope) -> SendOutcome {
        SendOutcome::Rejected {
            reason: Self::reason(),
            retryable_after_fix: true,
        }
    }

    fn observe(
        &self,
        _ctx: &Ctx,
        _binding: &SessionBinding,
        _cursor: &mut ObserveCursor,
    ) -> Result<Vec<Observation>> {
        Ok(Vec::new())
    }
}
