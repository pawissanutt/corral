//! `cmd.submit` / `cmd.result`: owner and agent commands executed by two `corrald` workers,
//! each on its own store handle, behind the idempotency journal in record area
//! `app-commands` (key `local-<uid>/<cmd_id>`), whose only writer is this module.
//!
//! Connection threads check what needs no store (build identity, herdr target, command,
//! expiry, caller) and hand the command to a worker. A worker refuses disabled commands and
//! reused ids, then reserves, checks `expect`, executes, settles, wakes the projector and
//! replies. Never the loop, reader or projector thread; no pass is requested.
use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use corral_core::{Store, backend::ApplyError};
use corral_ops::{
    Ctx,
    command::{self, Invoker, caller::env_from_process, render},
};
use corral_protocol::{
    cmd::{
        COMMANDS_AREA, Caller, CallerEnv, CmdError, CmdOutput, CmdResult, CmdResultQuery, CmdState,
        CmdSubmit, Command, Expect, OutputBody, Principal,
    },
    frame::{DATA_MAX, Frame, FrameKind, write_frame_until},
    rpc::ErrorCode,
};
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;

use crate::server;
use crate::state::Shared;

mod journal;

use journal::{Answer, Flight, Index, Journal, JournalState, Stored, expiry, principal};

const WORKERS: usize = 2;
const QUEUE: usize = 32;
/// How long a connection waits for its command before answering `running` (or, while it is
/// still queued, refusing it with `queue_timeout`).
const REPLY_WAIT: Duration = Duration::from_secs(25);
const RESULT_WAIT_MAX_MS: u32 = 25_000;
const OUTPUT_DEADLINE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(500);
const EVICT_AFTER: SignedDuration = SignedDuration::from_secs(600);
const LIVE_MAX: usize = 2_048;
/// How long an `abandoned` tombstone refuses a late submit of its id.
const TOMBSTONE_TTL: SignedDuration = SignedDuration::from_secs(3_600);

const QUEUED: u8 = 0;
const TAKEN: u8 = 1;
const CANCELLED: u8 = 2;

/// Shared by connection threads and workers; lives in `Shared::commands`.
pub(crate) struct Service {
    jobs: mpsc::SyncSender<Job>,
    flights: Mutex<HashMap<String, Arc<Flight>>>,
    index: Mutex<Index>,
    /// Journal reads and tombstones of `cmd.result`.
    journal: Mutex<Store>,
}

struct Job {
    submit: CmdSubmit,
    key: String,
    digest: String,
    /// `QUEUED` until a worker takes it (`TAKEN`) or its connection gives up (`CANCELLED`).
    handoff: Arc<AtomicU8>,
    reply: mpsc::Sender<Reply>,
}

enum Reply {
    Answer(Answer),
    /// The same command is already in flight: wait for it.
    Wait(Arc<Flight>),
    /// A JSON-RPC error: not accepted, nothing recorded.
    Refuse(ErrorCode, &'static str),
}

struct Worker {
    base: Ctx,
    timing: corral_ops::timing::Snapshot,
}

pub(crate) fn start(ctx: &Ctx, shared: &Arc<Shared>) -> Result<()> {
    let root = ctx.store.root();
    let journal = Store::open_unobserved(root).context("opening daemon command journal")?;
    let rows = journal
        .record_rows(COMMANDS_AREA)
        .context("loading daemon command journal")?;
    let (jobs, receiver) = mpsc::sync_channel::<Job>(QUEUE);
    let service = Arc::new(Service {
        jobs,
        flights: Mutex::default(),
        index: Mutex::new(Index::load(rows)),
        journal: Mutex::new(journal),
    });
    let receiver = Arc::new(Mutex::new(receiver));
    for n in 0..WORKERS {
        let mut worker = Worker {
            base: Ctx {
                store: Store::open_unobserved(root).context("opening daemon command store")?,
                herdr: ctx.herdr.clone(),
                resource_probe: Arc::clone(&ctx.resource_probe),
            },
            timing: corral_ops::timing::snapshot(),
        };
        let service = Arc::clone(&service);
        let shared = Arc::clone(shared);
        let receiver = Arc::clone(&receiver);
        std::thread::Builder::new()
            .name(format!("corrald-command-{n}"))
            .spawn(move || {
                while !shared.lock().closed {
                    let received = lock(&receiver).recv_timeout(POLL);
                    let job = match received {
                        Ok(job) => job,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    if job
                        .handoff
                        .compare_exchange(QUEUED, TAKEN, Ordering::AcqRel, Ordering::Acquire)
                        .is_err()
                    {
                        continue;
                    }
                    let reply =
                        service.run(&mut worker, &shared, job.submit, &job.key, &job.digest);
                    let _ = job.reply.send(reply);
                }
            })
            .context("starting daemon command worker")?;
    }
    *lock(&shared.commands) = Some(service);
    Ok(())
}

pub(crate) fn serve_submit(
    stream: &mut UnixStream,
    shared: &Shared,
    id: Value,
    peer_uid: u32,
    mut submit: CmdSubmit,
) -> Result<()> {
    if let Err((code, message)) = admit(&submit) {
        return server::error(stream, id, code, &message);
    }
    submit.caller.principal = Principal::LocalUser { uid: peer_uid };
    let Some(service) = service(shared) else {
        return server::error(stream, id, ErrorCode::Unavailable, "unavailable");
    };
    let cmd_id = submit.cmd_id.clone();
    let handoff = Arc::new(AtomicU8::new(QUEUED));
    let (reply, replies) = mpsc::channel();
    let job = Job {
        key: journal_key(peer_uid, &submit.cmd_id),
        digest: command::digest(&submit),
        submit,
        handoff: Arc::clone(&handoff),
        reply,
    };
    match service.jobs.try_send(job) {
        Ok(()) => {}
        Err(mpsc::TrySendError::Full(_)) => {
            return server::error(stream, id, ErrorCode::Busy, "queue_full");
        }
        Err(mpsc::TrySendError::Disconnected(_)) => {
            return server::error(stream, id, ErrorCode::Unavailable, "unavailable");
        }
    }
    let deadline = Instant::now() + REPLY_WAIT;
    let answer = match replies.recv_timeout(REPLY_WAIT) {
        Ok(Reply::Answer(answer)) => answer,
        Ok(Reply::Wait(flight)) => match flight.wait(deadline) {
            None => Answer::bare(CmdState::Running),
            Some(Some(answer)) => Answer {
                replayed: true,
                ..answer
            },
            // The first submit of this id was refused before it executed; neither ran.
            Some(None) => return server::error(stream, id, ErrorCode::Busy, "queue_timeout"),
        },
        Ok(Reply::Refuse(code, message)) => return server::error(stream, id, code, message),
        Err(problem) => {
            if handoff
                .compare_exchange(QUEUED, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let (code, message) = match problem {
                    mpsc::RecvTimeoutError::Timeout => (ErrorCode::Busy, "queue_timeout"),
                    mpsc::RecvTimeoutError::Disconnected => (ErrorCode::Unavailable, "unavailable"),
                };
                return server::error(stream, id, code, message);
            }
            // Taken: it may still be executing; the client re-queries with `cmd.result`.
            Answer::bare(CmdState::Running)
        }
    };
    write_answer(stream, id, cmd_id, answer)
}

pub(crate) fn serve_result(
    stream: &mut UnixStream,
    shared: &Shared,
    id: Value,
    peer_uid: u32,
    query: CmdResultQuery,
) -> Result<()> {
    if !valid_cmd_id(&query.cmd_id) {
        return server::error(stream, id, ErrorCode::InvalidParams, "invalid_cmd_id");
    }
    let Some(service) = service(shared) else {
        return server::error(stream, id, ErrorCode::Unavailable, "unavailable");
    };
    let key = journal_key(peer_uid, &query.cmd_id);
    let wait = Duration::from_millis(u64::from(query.wait_ms.min(RESULT_WAIT_MAX_MS)));
    match service.query(&key, Instant::now() + wait, query.abandon_if_absent) {
        Ok(answer) => write_answer(stream, id, query.cmd_id, answer),
        Err(problem) => {
            eprintln!("corrald command result {key}: {problem:#}");
            server::error(stream, id, ErrorCode::Unavailable, "journal_unavailable")
        }
    }
}

/// The pre-accept checks of the connection thread, in contract order.
fn admit(submit: &CmdSubmit) -> Result<(), (ErrorCode, String)> {
    match corral_ops::pm_read::client_identity() {
        Ok((exe, build)) => {
            let same = submit
                .client_build
                .as_ref()
                .is_some_and(|client| client.exe == exe && client.build == build);
            if !same {
                return refusal(ErrorCode::Refused, "client_mismatch");
            }
        }
        Err(problem) => {
            return Err((
                ErrorCode::Internal,
                format!("{problem:#}").replace(['\r', '\n'], " "),
            ));
        }
    }
    if command::touches_runtime(&submit.command)
        && !same_herdr(&submit.caller.env, &env_from_process())
    {
        return refusal(ErrorCode::Refused, "herdr_mismatch");
    }
    if !submit.command.is_known() {
        return refusal(ErrorCode::Refused, "unknown_command");
    }
    let (Ok(_), Ok(expires)) = (
        submit.issued_at.parse::<Timestamp>(),
        submit.expires_at.parse::<Timestamp>(),
    ) else {
        return refusal(ErrorCode::InvalidParams, "invalid_expiry");
    };
    if corral_core::clock::now() > expires {
        return refusal(ErrorCode::Expired, "expired");
    }
    if !valid_cmd_id(&submit.cmd_id) {
        return refusal(ErrorCode::InvalidParams, "invalid_cmd_id");
    }
    let caller = &submit.caller;
    if !matches!(
        caller.principal,
        Principal::Unverified | Principal::LocalUser { .. }
    ) {
        return refusal(ErrorCode::Unauthenticated, "unauthenticated");
    }
    let assistant_ok = caller
        .assistant
        .as_deref()
        .is_none_or(|session| corral_core::assistant::validate_id(session).is_ok());
    let cwd_ok = caller
        .cwd
        .as_deref()
        .is_none_or(|cwd| Path::new(cwd).is_absolute());
    if !assistant_ok || !cwd_ok {
        return refusal(ErrorCode::InvalidParams, "invalid_caller");
    }
    Ok(())
}

fn refusal(code: ErrorCode, message: &str) -> Result<(), (ErrorCode, String)> {
    Err((code, message.to_owned()))
}

/// The herdr target `Ctx` derives from the environment (`ctx.rs`) is the daemon's own, so a
/// runtime-touching command runs here only when the caller's four values equal the daemon's.
fn same_herdr(caller: &CallerEnv, daemon: &CallerEnv) -> bool {
    caller.corral_herdr_session == daemon.corral_herdr_session
        && caller.herdr_env == daemon.herdr_env
        && caller.herdr_socket_path == daemon.herdr_socket_path
        && caller.herdr_session == daemon.herdr_session
}

impl Service {
    /// A taken job: everything a worker does before it replies.
    fn run(
        &self,
        worker: &mut Worker,
        shared: &Shared,
        submit: CmdSubmit,
        key: &str,
        digest: &str,
    ) -> Reply {
        // Unobserved refresh: the loop observes config edits; this also reads the switch.
        match worker
            .base
            .store
            .config_layers()
            .and_then(|layers| layers.config())
        {
            Ok(config) => worker.base.store.config = config,
            Err(problem) => {
                eprintln!("corrald command config: {problem:#}");
                return Reply::Refuse(ErrorCode::Unavailable, "config_unavailable");
            }
        }
        if !worker.base.store.config.daemon.commands {
            return Reply::Refuse(ErrorCode::Refused, "commands_disabled");
        }
        let flight = {
            let mut flights = lock(&self.flights);
            if let Some(flight) = flights.get(key) {
                return if flight.digest == digest {
                    Reply::Wait(Arc::clone(flight))
                } else {
                    Reply::Refuse(ErrorCode::InvalidParams, "cmd_id_reused")
                };
            }
            let flight = Arc::new(Flight::new(digest.to_owned()));
            flights.insert(key.to_owned(), Arc::clone(&flight));
            flight
        };
        let (reply, executed) = self.accept(worker, submit, key, digest);
        lock(&self.flights).remove(key);
        // Only waiters that took the flight before its removal hold another reference; later
        // ones read the journal, which is already settled.
        if Arc::strong_count(&flight) > 1 {
            flight.land(match &reply {
                Reply::Answer(answer) => Some(answer.clone()),
                Reply::Wait(_) | Reply::Refuse(..) => None,
            });
        }
        if executed {
            shared.wake_projection();
        }
        reply
    }

    /// Evict, reserve, check `expect`, execute, settle. True when the command executed.
    fn accept(
        &self,
        worker: &mut Worker,
        submit: CmdSubmit,
        key: &str,
        digest: &str,
    ) -> (Reply, bool) {
        let Worker { base, timing } = worker;
        let store = &base.store;
        self.evict(store, key);
        if lock(&self.index).count(principal(key)) >= LIVE_MAX {
            return (Reply::Refuse(ErrorCode::Busy, "queue_full"), false);
        }
        let reserved = Journal {
            digest: digest.to_owned(),
            state: JournalState::Reserved,
            issued_at: submit.issued_at,
            expires_at: submit.expires_at,
            result: None,
        };
        match store.insert_record(COMMANDS_AREA, key, &reserved) {
            Ok(true) => lock(&self.index).live(key),
            Ok(false) => return (existing(store, key, digest), false),
            Err(problem) => {
                eprintln!("corrald reserving command {key}: {problem:#}");
                return (
                    Reply::Refuse(ErrorCode::Unavailable, "journal_unavailable"),
                    false,
                );
            }
        }
        let answer = match expectation(store, submit.expect.as_ref()) {
            Some(answer) => answer,
            None => execute(base, timing, submit.caller, submit.command),
        };
        let expires = expiry(&reserved.expires_at);
        let settled = Journal {
            state: JournalState::Settled,
            result: Some(answer.stored()),
            ..reserved
        };
        if let Err(problem) = base.store.save_record(COMMANDS_AREA, key, &settled) {
            // The reservation stays: replays answer `unknown`, never a second execution.
            eprintln!("corrald settling command {key}: {problem:#}");
        }
        lock(&self.index).done(key, expires);
        (Reply::Answer(answer), true)
    }

    /// Deletes up to 8 final entries of this principal whose expiry is 10 minutes past.
    fn evict(&self, store: &Store, key: &str) {
        let cutoff = corral_core::clock::now()
            .checked_sub(EVICT_AFTER)
            .unwrap_or(Timestamp::MIN);
        let victims = lock(&self.index).evictable(principal(key), cutoff);
        for victim in victims {
            if let Err(problem) = store.delete_record(COMMANDS_AREA, &victim.1) {
                eprintln!("corrald evicting command {}: {problem:#}", victim.1);
                return;
            }
            lock(&self.index).evicted(&victim);
        }
    }

    fn flight(&self, key: &str) -> Option<Arc<Flight>> {
        lock(&self.flights).get(key).cloned()
    }

    /// `cmd.result`: waits on an in-flight key, else answers from the journal; an absent key is
    /// tombstoned when asked, so a late submit of it is refused.
    fn query(&self, key: &str, deadline: Instant, abandon: bool) -> Result<Answer> {
        // A reservation seen while its flight registers or lands is looked at again.
        for _ in 0..3 {
            if let Some(flight) = self.flight(key) {
                match flight.wait(deadline) {
                    None => return Ok(Answer::bare(CmdState::Running)),
                    Some(Some(answer)) => return Ok(answer),
                    Some(None) => {}
                }
            }
            let store = lock(&self.journal);
            let journal = store
                .record::<Journal>(COMMANDS_AREA, key)
                .context("reading command journal")?;
            match journal {
                Some(journal) => match journal.state {
                    JournalState::Settled => {
                        return Ok(journal
                            .result
                            .map_or_else(|| Answer::bare(CmdState::Unknown), Stored::replay));
                    }
                    JournalState::Abandoned => return Ok(Answer::bare(CmdState::Abandoned)),
                    JournalState::Reserved if self.flight(key).is_some() => {}
                    JournalState::Reserved => return Ok(Answer::bare(CmdState::Unknown)),
                },
                None if abandon => {
                    let now = corral_core::clock::now();
                    let expires = now.checked_add(TOMBSTONE_TTL).unwrap_or(Timestamp::MAX);
                    let tombstone = Journal {
                        digest: String::new(),
                        state: JournalState::Abandoned,
                        issued_at: now.to_string(),
                        expires_at: expires.to_string(),
                        result: None,
                    };
                    if store
                        .insert_record(COMMANDS_AREA, key, &tombstone)
                        .context("writing command tombstone")?
                    {
                        lock(&self.index).done(key, expires);
                        return Ok(Answer::bare(CmdState::Abandoned));
                    }
                }
                None => return Ok(Answer::bare(CmdState::Unknown)),
            }
        }
        Ok(Answer::bare(CmdState::Unknown))
    }
}

/// The reservation lost to an existing entry of the same key.
fn existing(store: &Store, key: &str, digest: &str) -> Reply {
    match store.record::<Journal>(COMMANDS_AREA, key) {
        Ok(Some(journal))
            if journal.state == JournalState::Abandoned || journal.digest != digest =>
        {
            Reply::Refuse(ErrorCode::InvalidParams, "cmd_id_reused")
        }
        Ok(Some(journal)) => match (journal.state, journal.result) {
            (JournalState::Settled, Some(stored)) => Reply::Answer(stored.replay()),
            // Reserved by an earlier daemon run, or never settled: never executed again.
            _ => Reply::Answer(Answer::bare(CmdState::Unknown)),
        },
        Ok(None) => Reply::Refuse(ErrorCode::Unavailable, "journal_unavailable"),
        Err(problem) => {
            eprintln!("corrald reading command {key}: {problem:#}");
            Reply::Refuse(ErrorCode::Unavailable, "journal_unavailable")
        }
    }
}

/// `Some` when `expect` refuses execution.
fn expectation(store: &Store, expect: Option<&Expect>) -> Option<Answer> {
    let (task, revision) = match expect? {
        Expect::TaskRevision { task, revision } => (task, revision),
        Expect::Other => return Some(conflict("conflict: unsupported expectation".into())),
    };
    match store
        .get(task)
        .and_then(|current| corral_core::owner_revision(&current))
    {
        Ok(current) if current == *revision => None,
        Ok(_) => Some(conflict(format!(
            "conflict: {task} changed since revision {revision}"
        ))),
        Err(problem) => Some(failed(&render::ErrorStyle::Plain, problem)),
    }
}

fn conflict(message: String) -> Answer {
    Answer {
        state: CmdState::Conflict,
        exit: 1,
        output: Some(CmdOutput {
            stdout: String::new(),
            stderr: format!("error: {message}\n"),
        }),
        error: Some(CmdError {
            code: "conflict".into(),
            message,
        }),
        replayed: false,
    }
}

/// Executes on a fresh attribution of the worker's store, as `local::run` does after its open.
fn execute(
    base: &Ctx,
    timing: &mut corral_ops::timing::Snapshot,
    caller: Caller,
    command: Command,
) -> Answer {
    let started = Instant::now();
    let name = command.name();
    let answer = {
        let _span = corral_core::timing::span("corrald.command");
        let store = base.store.reattributed();
        let invoker = Invoker {
            caller,
            owner_action: None,
        };
        // Only the attribution half of the open spec applies: the loop observes config edits.
        let applied = if command::open_spec(&command).attribute {
            invoker.apply(&store)
        } else {
            Ok(())
        };
        match applied {
            Ok(()) => {
                let ctx = Ctx {
                    store,
                    herdr: base.herdr.clone(),
                    resource_probe: Arc::clone(&base.resource_probe),
                };
                Answer::of(command::execute_in(ctx, &invoker, command))
            }
            Err(problem) => failed(&command::error_style(&command), problem),
        }
    };
    *timing = corral_ops::timing::command(base.store.dir(), started, &name, timing);
    answer
}

/// A failure before the executor ran, classified and rendered like the executor's own.
fn failed(style: &render::ErrorStyle, problem: anyhow::Error) -> Answer {
    let state = match problem
        .chain()
        .find_map(|cause| cause.downcast_ref::<ApplyError>())
    {
        Some(ApplyError::Conflict(_) | ApplyError::Precondition(_)) => CmdState::Conflict,
        Some(ApplyError::Busy { .. }) => CmdState::Busy,
        _ => CmdState::Failed,
    };
    let stderr = render::failure(style, &problem);
    let message = format!("{problem:#}");
    let code = leading_code(&message)
        .unwrap_or(match state {
            CmdState::Conflict => "conflict",
            CmdState::Busy => "busy",
            _ => "failed",
        })
        .to_owned();
    Answer {
        state,
        exit: 1,
        error: Some(CmdError { code, message }),
        output: Some(CmdOutput {
            stdout: String::new(),
            stderr,
        }),
        replayed: false,
    }
}

/// The leading `snake_case:` token of an error message, as the executor reports it.
fn leading_code(message: &str) -> Option<&str> {
    let (token, _) = message.split_once(':')?;
    let mut bytes = token.bytes();
    (bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
    .then_some(token)
}

/// The response, then exactly the announced DATA frames under one deadline, as `read` does.
fn write_answer(stream: &mut UnixStream, id: Value, cmd_id: String, answer: Answer) -> Result<()> {
    let Answer {
        state,
        exit,
        error,
        output,
        replayed,
    } = answer;
    let mut result = CmdResult {
        cmd_id,
        state,
        exit,
        error,
        output: None,
        replayed,
    };
    let Some(output) = output else {
        return server::reply(stream, id, result);
    };
    let bytes = serde_json::to_vec(&output).context("encoding command output")?;
    let stream_id = 1;
    result.output = Some(OutputBody {
        stream: stream_id,
        bytes: bytes.len() as u64,
        frames: u32::try_from(bytes.len().div_ceil(DATA_MAX).max(1))
            .context("counting command output frames")?,
    });
    server::reply(stream, id, result)?;
    let deadline = Instant::now() + OUTPUT_DEADLINE;
    let mut frame = Frame {
        kind: FrameKind::Data,
        stream: stream_id,
        payload: Vec::with_capacity(bytes.len().min(DATA_MAX)),
    };
    for chunk in bytes.chunks(DATA_MAX) {
        frame.payload.clear();
        frame.payload.extend_from_slice(chunk);
        write_frame_until(stream, &frame, deadline).context("writing command output")?;
    }
    Ok(())
}

fn service(shared: &Shared) -> Option<Arc<Service>> {
    lock(&shared.commands).clone()
}

/// 1 to 64 of `[A-Za-z0-9._-]`.
fn valid_cmd_id(cmd_id: &str) -> bool {
    (1..=64).contains(&cmd_id.len())
        && cmd_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn journal_key(uid: u32, cmd_id: &str) -> String {
    format!("local-{uid}/{cmd_id}")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
