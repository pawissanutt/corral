//! Daemon-owned pane-less hosts. Input is persisted on a bounded worker queue; the
//! independent supervisor pumps children even when no app is connected. Neither waits
//! for model completion, and a delivered/uncertain turn is never automatically resent.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use corral_core::Store;
use corral_ops::{
    Ctx,
    agent::app_runtime::{self as runtime, AuthenticatedDevice, Runtime},
};
use corral_protocol::{assistant as wire, attachment, rpc::ErrorCode, subscription::LiveText};
use serde_json::Value;

use crate::state::Shared;

type Failure = (ErrorCode, String);
const QUEUE: usize = 32;
const HOSTS: usize = 8;
const POLL: Duration = Duration::from_millis(100);
const REQUEST_WAIT: Duration = Duration::from_secs(25);

pub(crate) struct Service {
    jobs: mpsc::SyncSender<Job>,
    live: Mutex<BTreeMap<String, Arc<LiveText>>>,
    quick: Mutex<Option<String>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

struct Job {
    device: String,
    request: Request,
    reply: mpsc::Sender<Result<Value, Failure>>,
}

pub(crate) enum Request {
    Send(wire::AssistantSend),
    Cancel(wire::AssistantCancel),
    AttachBegin(attachment::AttachmentBegin),
    AttachPut(attachment::AttachmentPut),
    AttachDiscard(attachment::AttachmentDiscard),
    Warm(wire::AssistantWarm),
    Stop(wire::AssistantStop),
    Session(wire::AssistantSessionOp),
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|problem| problem.into_inner())
}

fn context(ctx: &Ctx) -> Result<Ctx> {
    Ok(Ctx {
        store: Store::open_unobserved(ctx.store.root())?,
        herdr: ctx.herdr.clone(),
        resource_probe: Arc::clone(&ctx.resource_probe),
    })
}

pub(crate) fn start(ctx: &Ctx, shared: &Arc<Shared>) -> Result<()> {
    let (jobs, receiver) = mpsc::sync_channel(QUEUE);
    let service = Arc::new(Service {
        jobs,
        live: Mutex::new(BTreeMap::new()),
        quick: Mutex::new(None),
        threads: Mutex::new(Vec::new()),
    });
    let mut input_ctx = context(ctx)?;
    let input_shared = Arc::clone(shared);
    let input = std::thread::Builder::new()
        .name("corrald-app-input".into())
        .spawn(move || {
            while !input_shared.lock().closed {
                let job: Job = match receiver.recv_timeout(POLL) {
                    Ok(job) => job,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let result = input_ctx
                    .reload_config()
                    .map_err(failure)
                    .and_then(|_| execute(&input_ctx, &input_shared, &job.device, job.request));
                if result.is_ok() {
                    input_shared.wake_projection();
                }
                let _ = job.reply.send(result);
            }
        })
        .context("starting app input worker")?;
    lock(&service.threads).push(input);
    // Publish before spawning so a partial startup is still joined by Cleanup.
    *lock(&shared.assistant) = Some(Arc::clone(&service));
    let host_ctx = context(ctx)?;
    let host_shared = Arc::clone(shared);
    let hosts = std::thread::Builder::new()
        .name("corrald-assistant-hosts".into())
        .spawn(move || {
            supervise(host_ctx, host_shared, service);
        })
        .context("starting pane-less assistant supervisor")?;
    if let Some(service) = lock(&shared.assistant).as_ref() {
        lock(&service.threads).push(hosts);
    }
    Ok(())
}

pub(crate) fn available(shared: &Shared) -> bool {
    lock(&shared.assistant).is_some()
}

pub(crate) fn request(shared: &Shared, device: &str, request: Request) -> Result<Value, Failure> {
    let service = lock(&shared.assistant)
        .clone()
        .ok_or((ErrorCode::Unavailable, "assistant_unavailable".into()))?;
    let (reply, receive) = mpsc::channel();
    service
        .jobs
        .try_send(Job {
            device: device.into(),
            request,
            reply,
        })
        .map_err(|problem| match problem {
            mpsc::TrySendError::Full(_) => (ErrorCode::Busy, "assistant_queue_full".into()),
            mpsc::TrySendError::Disconnected(_) => {
                (ErrorCode::Unavailable, "assistant_unavailable".into())
            }
        })?;
    receive.recv_timeout(REQUEST_WAIT).map_err(|_| {
        (
            ErrorCode::Unavailable,
            "assistant_result_uncertain_recover_same_id".into(),
        )
    })?
}

fn authenticated(shared: &Shared, id: &str) -> Result<AuthenticatedDevice, Failure> {
    let devices = shared
        .devices()
        .ok_or((ErrorCode::Unavailable, "devices_unavailable".into()))?;
    let (info, authority) = devices
        .authenticated(id)
        .map_err(|problem| problem.wire())?;
    Ok(AuthenticatedDevice {
        id: info.id,
        name: info.name,
        reply: authority.reply,
        act: authority.act,
        assistant: authority.assistant,
        assistant_act: authority.assistant_act,
    })
}

fn execute(ctx: &Ctx, shared: &Shared, id: &str, request: Request) -> Result<Value, Failure> {
    if !ctx.store.config.daemon.commands {
        return Err((ErrorCode::Refused, "commands_disabled".into()));
    }
    // Do not trust a connection-time grant: queued input can outlive device revocation.
    let device = authenticated(shared, id)?;
    if !device.assistant {
        return Err((ErrorCode::Refused, "authority_assistant".into()));
    }
    match request {
        Request::Send(request) => {
            // Accepted sends always replay before journal tombstones or capacity checks.
            if ctx
                .store
                .app_send_entry(id, &request.cmd_id)
                .map_err(failure)?
                .is_none()
            {
                crate::commands::assistant::reserve_send(shared, id, &request)?;
            }
            runtime::send(ctx, &device, &request)
                .and_then(value)
                .map_err(failure)
        }
        Request::Cancel(request) => {
            if let Some(ack) = runtime::cancel(ctx, &device, &request.cmd_id).map_err(failure)? {
                return value(ack).map_err(failure);
            }
            crate::commands::assistant::cancel(shared, id, &request.cmd_id)?;
            value(wire::CancelAck {
                cancelled: true,
                entry: None,
                state: None,
            })
            .map_err(failure)
        }
        Request::AttachBegin(request) => runtime::attachments::begin(ctx, &device, &request)
            .and_then(value)
            .map_err(failure),
        Request::AttachPut(request) => runtime::attachments::put(ctx, &device, &request)
            .and_then(value)
            .map_err(failure),
        Request::AttachDiscard(request) => runtime::attachments::discard(ctx, &device, &request)
            .and_then(|()| value(serde_json::json!({})))
            .map_err(failure),
        Request::Warm(request) => runtime::warm(ctx, &device, &request.thread)
            .and_then(value)
            .map_err(failure),
        Request::Stop(request) => runtime::stop(ctx, &device, &request.session, request.turn)
            .and_then(value)
            .map_err(failure),
        Request::Session(request) => runtime::session(ctx, &device, &request)
            .and_then(value)
            .map_err(failure),
    }
}

fn value(value: impl serde::Serialize) -> Result<Value> {
    serde_json::to_value(value).context("encoding assistant result")
}

fn failure(problem: anyhow::Error) -> Failure {
    let message = problem.to_string();
    let code = message.split(':').next().unwrap_or("");
    let kind = match code {
        "conflict" | "stale_revision" | "payload_mismatch" | "digest_mismatch"
        | "offset_mismatch" | "cmd_id_in_use" => ErrorCode::Conflict,
        "not_found" => ErrorCode::NotFound,
        "refused"
        | "authority_assistant"
        | "authority_reply"
        | "authority_act"
        | "not_app_safe"
        | "attachment_type_unsupported"
        | "attachment_too_large"
        | "attachment_quota"
        | "attachment_expired"
        | "attachment_unknown"
        | "attachments_unsupported_target"
        | "cancelled" => ErrorCode::Refused,
        "confirm_required" => ErrorCode::ConfirmRequired,
        "invalid_params" | "invalid_scope" => ErrorCode::InvalidParams,
        "busy" => ErrorCode::Busy,
        _ => ErrorCode::Unavailable,
    };
    (kind, bounded(&message, 512).to_owned())
}

fn supervise(mut ctx: Ctx, shared: Arc<Shared>, service: Arc<Service>) {
    let mut hosts = BTreeMap::<String, Runtime>::new();
    let mut next_scan = Instant::now();
    let mut next_sweep = Instant::now();
    while !shared.lock().closed {
        if Instant::now() >= next_sweep {
            if let Err(problem) = ctx
                .store
                .app_upload_sweep(|id| runtime::input::device_by_id(&ctx.store, id).is_ok())
            {
                eprintln!("corrald attachment cleanup: {problem:#}");
            }
            next_sweep = Instant::now() + Duration::from_secs(60);
        }
        if Instant::now() >= next_scan {
            // Configuration changes affect new provider sessions, without retargeting a live turn.
            match ctx
                .reload_config()
                .and_then(|_| runtime::pending_scopes(&ctx))
            {
                Ok(scopes) => {
                    for scope in scopes {
                        if hosts.len() >= HOSTS {
                            break;
                        }
                        if hosts.contains_key(&scope) {
                            continue;
                        }
                        match runtime::start(&ctx, &scope) {
                            Ok(host) => {
                                if let Ok(Some(live)) = runtime::live(&ctx, &scope) {
                                    merge_live(&service, &scope, live);
                                }
                                hosts.insert(scope, host);
                            }
                            Err(problem) => eprintln!("corrald assistant start: {problem:#}"),
                        }
                    }
                }
                Err(problem) => eprintln!("corrald assistant scheduling: {problem:#}"),
            }
            if let Ok(scope) = runtime::resolve_scope(&ctx, "quick") {
                *lock(&service.quick) = scope;
            }
            next_scan = Instant::now() + Duration::from_secs(1);
        }
        let mut exited = Vec::new();
        for (scope, host) in &mut hosts {
            match runtime::pump(&ctx, host) {
                Ok(update) => {
                    for live in update.live {
                        merge_live(&service, scope, live);
                    }
                    if update.changed {
                        shared.wake_projection();
                    }
                    if update.exited {
                        exited.push(scope.clone());
                    }
                }
                Err(problem) => {
                    eprintln!("corrald assistant pump: {problem:#}");
                    let _ = runtime::shutdown(&ctx, host);
                    exited.push(scope.clone());
                    shared.wake_projection();
                }
            }
        }
        for scope in exited {
            hosts.remove(&scope);
            if let Some(live) = lock(&service.live).get_mut(&scope) {
                Arc::make_mut(live).done = true;
            }
        }
        shared.wait(POLL, false);
    }
    for host in hosts.values_mut() {
        if let Err(problem) = runtime::shutdown(&ctx, host) {
            eprintln!("corrald assistant shutdown: {problem:#}");
        }
    }
}

fn merge_live(service: &Service, scope: &str, incoming: LiveText) {
    let mut streams = lock(&service.live);
    if !streams.contains_key(scope) && streams.len() >= HOSTS * 4 {
        // Only completed streams can be dropped; live hosts are capped separately.
        if let Some(victim) = streams
            .iter()
            .find(|(_, live)| live.done)
            .map(|(key, _)| key.clone())
        {
            streams.remove(&victim);
        } else {
            return;
        }
    }
    let entry = streams.entry(scope.into()).or_insert_with(|| {
        Arc::new(LiveText {
            entry: incoming.entry.clone(),
            turn: incoming.turn,
            offset: 0,
            text: String::new(),
            done: false,
        })
    });
    let entry = Arc::make_mut(entry);
    if entry.entry != incoming.entry || entry.turn != incoming.turn {
        *entry = LiveText {
            entry: incoming.entry.clone(),
            turn: incoming.turn,
            offset: 0,
            text: String::new(),
            done: false,
        };
    }
    // Ignore duplicated prefixes, never splice text across a missing streamed range.
    let end = entry.text.len() as u64;
    if incoming.offset > end {
        return;
    }
    let overlap = (end - incoming.offset) as usize;
    if overlap < incoming.text.len() && incoming.text.is_char_boundary(overlap) {
        let remaining = corral_protocol::assistant::TURN_TEXT_MAX.saturating_sub(entry.text.len());
        entry
            .text
            .push_str(bounded(&incoming.text[overlap..], remaining));
    }
    entry.done |= incoming.done;
}

fn bounded(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Cumulative turn buffer. Topics transfer it from their acknowledged byte offset.
pub(crate) fn live(shared: &Shared, scope: &str) -> Option<Arc<LiveText>> {
    lock(&shared.assistant).as_ref().and_then(|service| {
        let quick = lock(&service.quick);
        let scope = if scope == "quick" {
            quick.as_deref()?
        } else {
            scope
        };
        lock(&service.live).get(scope).cloned()
    })
}

pub(crate) fn join(shared: &Shared) {
    let Some(service) = lock(&shared.assistant).clone() else {
        return;
    };
    let threads = std::mem::take(&mut *lock(&service.threads));
    for thread in threads {
        if thread.join().is_err() {
            eprintln!("corrald assistant worker failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_replay_merges_only_contiguous_utf8_and_bounds_the_turn() {
        let (jobs, _) = mpsc::sync_channel(1);
        let service = Service {
            jobs,
            live: Mutex::default(),
            quick: Mutex::default(),
            threads: Mutex::default(),
        };
        let event = |offset, text: &str, done| LiveText {
            entry: "entry-1".into(),
            turn: 1,
            offset,
            text: text.into(),
            done,
        };
        merge_live(&service, "A-1", event(0, "hé", false));
        merge_live(&service, "A-1", event(0, "hé", false));
        merge_live(&service, "A-1", event(9, "gap", true));
        assert_eq!(lock(&service.live)["A-1"].text, "hé");
        assert!(!lock(&service.live)["A-1"].done);
        merge_live(&service, "A-1", event(3, "llo", true));
        assert_eq!(lock(&service.live)["A-1"].text, "héllo");
        assert!(lock(&service.live)["A-1"].done);
        let oversized = "é".repeat(wire::TURN_TEXT_MAX);
        merge_live(&service, "A-1", event(6, &oversized, true));
        let streams = lock(&service.live);
        assert!(streams["A-1"].text.len() <= wire::TURN_TEXT_MAX);
        assert!(
            streams["A-1"]
                .text
                .is_char_boundary(streams["A-1"].text.len())
        );
    }

    #[test]
    fn accepted_send_recovery_precedes_cancel_journal_admission() {
        let mut fixture = crate::tests::Fixture::new();
        fixture.ctx.store.config.daemon.commands = true;
        let shared = Arc::new(Shared::new());
        crate::devices::start(&fixture.ctx, &shared, None).unwrap();
        let info = shared
            .devices()
            .unwrap()
            .enrol_native("Owner", "test", "test")
            .unwrap();
        let device = authenticated(&shared, &info.id).unwrap();
        let send = wire::AssistantSend {
            cmd_id: "accepted".into(),
            thread: "quick".into(),
            target: wire::AskTarget::Assistant,
            text: "typed".into(),
            typed_at: corral_core::clock::now().to_string(),
            context: Vec::new(),
            attachments: Vec::new(),
        };
        let accepted = runtime::send(&fixture.ctx, &device, &send).unwrap();
        fixture
            .ctx
            .store
            .update_app_message(&accepted.entry, |message| {
                message.state = "delivered".into();
                Ok(())
            })
            .unwrap();
        // No command service exists: any accidental fresh-id journal admission would fail.
        let cancel: wire::CancelAck = serde_json::from_value(
            execute(
                &fixture.ctx,
                &shared,
                &info.id,
                Request::Cancel(wire::AssistantCancel {
                    cmd_id: send.cmd_id.clone(),
                }),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(!cancel.cancelled);
        assert_eq!(cancel.entry.as_deref(), Some(accepted.entry.as_str()));
        let replay: wire::SendAck = serde_json::from_value(
            execute(&fixture.ctx, &shared, &info.id, Request::Send(send)).unwrap(),
        )
        .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.entry, accepted.entry);
        shared.close();
    }
}
