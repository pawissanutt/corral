//! The `pi_native` inbox adapter: send is bridge `submit_input` with the envelope idempotency
//! key, observe reads the host-written `corral_delivery` session entry whose `text_sha256` is
//! the envelope's SHA-256 (`packages/corral-pi/src/delivery/index.ts` writes it before Pi sees
//! the text). One adapter serves worker runs, review children (`ReviewRun`) and the bound PM.
//! Owner: T-252 child inbox-adapter.
use std::path::Path;

use anyhow::{Context, Result};
use corral_core::agent::{AgentRunState, ContainerRef, HostHealth, pm_state};
use corral_core::inbox::{Envelope, ObserveCursor, Recipient, SendOutcome, SessionBinding};
use serde_json::json;

use crate::Ctx;
use crate::agent::bridge::{BridgeClient, BridgeError, BusyPolicy, Disposition, SubmitInput};
use crate::agent::launch::{SOCKET_FILE, generation_dir};
use crate::delivery::{
    BindOutcome, Capabilities, DeliveryAdapter, Observation, Readiness, RecipientRef,
};

mod observe;

/// Adapter name recorded on every pi-native envelope.
pub const ADAPTER: &str = "pi_native";
/// The T-252 child that lands the adapter.
pub const OWNER: &str = "inbox-adapter";
/// `origin` of every inbox input; the host's delivery module copies it into its entry.
pub const ORIGIN: &str = "corral-inbox";
/// The custom session entry type the host's delivery module writes.
pub const ENTRY_TYPE: &str = "corral_delivery";
/// The host refuses `submit_input` text over 8 KiB (`INPUT_MAX_BYTES` in bridge/protocol.ts).
pub const INPUT_MAX_BYTES: usize = 8 * 1024;

/// Bridge refusals that prove the input itself can never be admitted. Every other failure
/// (closed socket, unreadable reply, a host that is not ready) is uncertain: the host keeps
/// one outcome per key for the generation's lifetime, so a resend of the same key is safe.
const PERMANENT: [&str; 3] = ["bad_input", "frame_too_large", "unknown_command"];

/// The registered pi-native adapter.
pub struct PiNativeAdapter;

/// The bridge idempotency key of an envelope: the host accepts `[A-Za-z0-9._:-]` only, so the
/// envelope key's `@env` separator travels as `:env` (inbox keys never contain `:`).
pub fn wire_key(envelope_key: &str) -> String {
    envelope_key.replacen("@env", ":env", 1)
}

/// The envelope key a bridge idempotency key names, when it names one.
pub fn envelope_key(wire: &str) -> Option<(String, u64)> {
    let (recipient, number) = wire.rsplit_once(":env")?;
    Some((recipient.to_owned(), number.parse().ok()?))
}

/// What the adapter can see of a recipient: its binding state and whether it is closed.
struct Target {
    state: AgentRunState,
    closed: Option<&'static str>,
}

fn container(recipient: &Recipient) -> Option<ContainerRef> {
    match recipient {
        Recipient::Run { task, run } => Some(ContainerRef::Worker {
            task: task.clone(),
            run: run.clone(),
        }),
        Recipient::ReviewRun { task, run, review } => Some(ContainerRef::Reviewer {
            task: task.clone(),
            run: run.clone(),
            review: review.clone(),
        }),
        Recipient::Pm => Some(ContainerRef::Pm),
        Recipient::Assistant { session } => Some(ContainerRef::Assistant {
            session: session.clone(),
        }),
        Recipient::PlanRun { task, plan_run } => Some(ContainerRef::Planner {
            task: task.clone(),
            run: plan_run.clone(),
        }),
        Recipient::PlanReview { task, plan, review } => Some(ContainerRef::PlanReviewer {
            task: task.clone(),
            plan: plan.clone(),
            review: review.clone(),
        }),
    }
}

/// `Err(reason)` when the recipient has no Pi-native binding at all.
fn target(ctx: &Ctx, recipient: &Recipient) -> Result<Result<Target, String>> {
    let Some(container) = container(recipient) else {
        return Ok(Err(
            "pi_native delivery serves worker runs, review children and the PM".into(),
        ));
    };
    if let ContainerRef::Assistant { session } = &container {
        if !ctx.store.assistant_path(session)?.exists() {
            return Ok(Err(format!("host_unbound: no assistant session {session}")));
        }
        let ended = ctx.store.get_assistant(session)?.ended.is_some();
        let state = ctx.store.get_assistant_container(session)?.state;
        return Ok(Ok(Target {
            state,
            closed: ended.then_some("assistant_ended"),
        }));
    }
    match container.task() {
        Some(id) => {
            let task = ctx.store.get(id)?;
            let closed =
                corral_core::agent::container_closure(&task, &container).map(|r| r.as_str());
            Ok(corral_core::agent::container_state(&task, &container)
                .cloned()
                .map(|state| Target { state, closed })
                .ok_or_else(|| format!("host_unbound: {} has no agent binding", container.key())))
        }
        None => Ok(pm_state::load(&ctx.store)?
            .map(|pm| Target {
                state: pm.state,
                closed: None,
            })
            .ok_or_else(|| "host_unbound: no Pi-native PM is bound".into())),
    }
}

/// Why the current generation cannot take input now, or `None` when its binding says it can.
fn unready(state: &AgentRunState) -> Option<String> {
    let n = state.binding.generation;
    match state.binding.health {
        HostHealth::Healthy => {}
        HostHealth::Unbound => return Some(format!("host_unbound: generation {n}")),
        HostHealth::Lost => return Some(format!("host_lost: generation {n}")),
        HostHealth::Fenced => return Some(format!("host_fenced: generation {n}")),
    }
    match state.current_generation() {
        None => Some(format!("host_unbound: generation {n} was never opened")),
        Some(g) if g.fenced.is_some() => Some(format!("host_fenced: generation {n}")),
        Some(g) if g.bound.is_none() => Some(format!(
            "host_unbound: generation {n} has not completed hello"
        )),
        Some(_) => None,
    }
}

/// The current generation's session: its Pi session id, its socket and its session file.
fn session(
    ctx: &Ctx,
    recipient: &Recipient,
    state: &AgentRunState,
) -> Result<Option<SessionBinding>> {
    let Some(generation) = state.current_generation() else {
        return Ok(None);
    };
    let Some(bound) = &generation.bound else {
        return Ok(None);
    };
    let dir = generation_dir(&ctx.store, recipient, generation.n)?;
    Ok(Some(SessionBinding {
        session: bound.session_id.clone(),
        endpoint: Some(dir.join(SOCKET_FILE)),
        transcript: Some(bound.session_file.clone()),
    }))
}

/// The generation a binding's endpoint (`.../g<n>/host.sock`) names.
fn endpoint_generation(endpoint: &Path) -> Option<(&Path, u32)> {
    let dir = endpoint.parent()?;
    let n = dir.file_name()?.to_str()?.strip_prefix('g')?.parse().ok()?;
    Some((dir, n))
}

fn busy_policy(mode: &str) -> BusyPolicy {
    match mode {
        "steer" => BusyPolicy::Steer,
        _ => BusyPolicy::FollowUp,
    }
}

fn disposition(value: Disposition) -> &'static str {
    match value {
        Disposition::Started => "started",
        Disposition::Queued => "queued",
        Disposition::Handled => "handled",
        Disposition::Rejected => "rejected",
        Disposition::Pending => "pending",
    }
}

impl PiNativeAdapter {
    fn submit(binding: &SessionBinding, envelope: &Envelope, text: String) -> SendOutcome {
        let Some((dir, generation)) = binding.endpoint.as_deref().and_then(endpoint_generation)
        else {
            return SendOutcome::Rejected {
                reason: "pi_native binding has no generation endpoint".into(),
                retryable_after_fix: false,
            };
        };
        let key = wire_key(&envelope.idempotency_key);
        let input = SubmitInput {
            text,
            busy_policy: busy_policy(&envelope.mode),
            idempotency_key: key.clone(),
            origin: ORIGIN.into(),
        };
        let reply = BridgeClient::connect_dir(dir, generation)
            .and_then(|mut client| client.submit_input(&input));
        let reply = match reply {
            Ok(reply) => reply,
            Err(error) => {
                return match error.downcast_ref::<BridgeError>() {
                    Some(refusal) if PERMANENT.contains(&refusal.code.as_str()) => {
                        SendOutcome::Rejected {
                            reason: format!("pi_native submit_input: {refusal}"),
                            retryable_after_fix: false,
                        }
                    }
                    _ => SendOutcome::Uncertain {
                        reason: format!("pi_native submit_input: {error:#}"),
                    },
                };
            }
        };
        if reply.idempotency_key != key {
            return SendOutcome::Uncertain {
                reason: "pi_native submit_input: the reply names another key".into(),
            };
        }
        let reason = reply.reason.clone().unwrap_or_default();
        match reply.disposition {
            Disposition::Rejected if reason == "idempotency_conflict" => SendOutcome::Conflicted {
                reason: "pi_native submit_input: idempotency_conflict".into(),
            },
            Disposition::Rejected => SendOutcome::Rejected {
                reason: format!("pi_native submit_input rejected: {reason}"),
                retryable_after_fix: true,
            },
            other => SendOutcome::Admitted {
                detail: json!({
                    "request_id": reply.request_id, "idempotency_key": reply.idempotency_key,
                    "disposition": disposition(other), "turn": reply.turn,
                    "replayed": reply.replayed, "generation": generation,
                }),
            },
        }
    }
}

impl DeliveryAdapter for PiNativeAdapter {
    fn harness(&self) -> &'static str {
        ADAPTER
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // The host answers a replayed key with its first outcome and never re-submits it.
            idempotent_retry: true,
            admission: true,
            non_waking: false,
            max_payload: INPUT_MAX_BYTES.min(corral_core::inbox::MAX_PAYLOAD),
            send_budget: None,
            transcript_reader: false,
        }
    }

    /// Native only while the recipient is open, its current generation is bound and healthy,
    /// and that host answers hello now: a message is never attempted into a host that is gone,
    /// so it stays pending for the next generation instead of turning uncertain.
    fn readiness(&self, ctx: &Ctx, recipient: &RecipientRef) -> Result<Readiness> {
        let target = match target(ctx, recipient)? {
            Ok(target) => target,
            Err(reason) => return Ok(Readiness::Unavailable { reason }),
        };
        if let Some(reason) = target.closed {
            return Ok(Readiness::Unavailable {
                reason: format!("recipient_closed: {reason}"),
            });
        }
        if let Some(reason) = unready(&target.state) {
            return Ok(Readiness::Unavailable { reason });
        }
        let generation = target.state.binding.generation;
        let hello = BridgeClient::connect(&ctx.store, recipient, generation)
            .and_then(|mut client| client.hello());
        Ok(match hello {
            Ok(hello) if hello["bound"] == true => Readiness::Native,
            Ok(_) => Readiness::Unavailable {
                reason: format!("host_unbound: generation {generation} is not bound yet"),
            },
            Err(error) => Readiness::Unavailable {
                reason: format!("host_unreachable: generation {generation}: {error:#}"),
            },
        })
    }

    /// The current generation's session, even while its host is lost: an attempted envelope
    /// stays with its generation and only a replacement generation orphans it.
    fn bind(&self, ctx: &Ctx, recipient: &RecipientRef) -> Result<BindOutcome> {
        let target = match target(ctx, recipient)? {
            Ok(target) => target,
            Err(reason) => return Ok(BindOutcome::Unbound { reason }),
        };
        if let Some(reason) = target.closed {
            return Ok(BindOutcome::Unbound {
                reason: format!("recipient_closed: {reason}"),
            });
        }
        Ok(match session(ctx, recipient, &target.state)? {
            Some(binding) => BindOutcome::Bound(binding),
            None => BindOutcome::Unbound {
                reason: format!(
                    "host_unbound: generation {} has not completed hello",
                    target.state.binding.generation
                ),
            },
        })
    }

    fn send(&self, ctx: &Ctx, binding: &SessionBinding, envelope: &Envelope) -> SendOutcome {
        let text = match std::fs::read_to_string(&envelope.payload_path) {
            Ok(text) if observe::sha256(&text) == envelope.payload_sha256 => text,
            Ok(_) | Err(_) => {
                return SendOutcome::Rejected {
                    reason: "frozen envelope payload unreadable or changed".into(),
                    retryable_after_fix: true,
                };
            }
        };
        if text.len() > INPUT_MAX_BYTES {
            return SendOutcome::Rejected {
                reason: format!("pi_native input exceeds {INPUT_MAX_BYTES} bytes"),
                retryable_after_fix: false,
            };
        }
        // A closed container is refused at the last moment too; its messages are cancelled
        // `recipient_closed` by question terminalization.
        match Recipient::parse_key(envelope.idempotency_key.split('@').next().unwrap_or(""))
            .context("pi_native envelope key")
            .and_then(|recipient| target(ctx, &recipient))
        {
            Ok(Ok(Target {
                closed: Some(reason),
                ..
            })) => {
                return SendOutcome::Rejected {
                    reason: format!("recipient_closed: {reason}"),
                    retryable_after_fix: false,
                };
            }
            Ok(_) => {}
            Err(error) => {
                return SendOutcome::Uncertain {
                    reason: format!("pi_native recipient unreadable: {error:#}"),
                };
            }
        }
        Self::submit(binding, envelope, text)
    }

    fn observe(
        &self,
        ctx: &Ctx,
        binding: &SessionBinding,
        cursor: &mut ObserveCursor,
    ) -> Result<Vec<Observation>> {
        observe::observe(ctx, binding, cursor)
    }
}

#[cfg(all(test, unix))]
mod tests;
