//! Authenticated native/web actions share the CLI journal and workers, never its admission
//! or DATA transport. Identity, environment and authority come only from the daemon.
//! Preview proofs bind the complete command, expectation and device; they expire in 60s
//! and are consumed once, before reservation. Replays read the journal without new effects.
use super::*;
use corral_ops::agent::app_runtime::{AuthenticatedDevice, input::FreshConfirmation};
use corral_protocol::{
    app::{ActClass, ActPreview, ActPreviewRequest, ActSubmit, ConfirmProof, DeviceAuthority},
    assistant::{PromptAnswer, PromptChoice, PromptPreview},
    cmd::{ActorName, ClientKind, Via},
};
use serde::Serialize;

pub(crate) type AppResult<T> = Result<T, (ErrorCode, String)>;
const PREVIEW_MAX: usize = 2_048;

pub(super) struct Preview {
    device: String,
    digest: String,
    deadline: Instant,
    target: Option<String>,
}

fn error(code: ErrorCode, message: impl Into<String>) -> (ErrorCode, String) {
    (code, message.into())
}

fn authenticate(
    shared: &Shared,
    device: &str,
) -> AppResult<(corral_protocol::device::DeviceInfo, DeviceAuthority)> {
    shared
        .devices()
        .ok_or_else(|| error(ErrorCode::Unavailable, "unavailable"))?
        .authenticated(device)
        .map_err(|problem| problem.wire())
}

fn policy(
    authority: &DeviceAuthority,
    command: &Command,
) -> Result<ActClass, (ErrorCode, &'static str)> {
    let class = command::app_class(command);
    let refusal = match class {
        ActClass::Reply if !authority.reply => Some("authority_reply"),
        ActClass::Act | ActClass::Confirm if !authority.act => Some("authority_act"),
        ActClass::Admin if !authority.admin => Some("admin_only"),
        ActClass::Refused | ActClass::Other => Some("not_app_safe"),
        _ => None,
    };
    match refusal {
        Some(message) => Err((ErrorCode::Refused, message)),
        None => Ok(class),
    }
}

fn digest(value: &impl Serialize) -> AppResult<String> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| error(ErrorCode::Internal, "digest_unavailable"))?;
    Ok(corral_core::agent::paths::sha256_hex(&bytes))
}

fn action_digest(device: &str, command: &Command, expect: Option<&Expect>) -> AppResult<String> {
    digest(&("app.action.v1", device, command, expect))
}

fn require_expect(command: &Command, expect: Option<&Expect>) -> AppResult<()> {
    if let Command::Task(corral_protocol::cmd::task::TaskCommand::Unstick { id }) = command
        && !matches!(expect, Some(Expect::TaskRevision { task, .. }) if task == id)
    {
        return Err(error(ErrorCode::InvalidParams, "task_revision_required"));
    }
    Ok(())
}

fn issue(
    service: &Service,
    device: &str,
    digest: String,
    class: ActClass,
    effects: Vec<String>,
) -> AppResult<ActPreview> {
    let now = Instant::now();
    let mut previews = lock(&service.previews);
    previews.retain(|_, preview| preview.deadline > now);
    if previews.len() >= PREVIEW_MAX {
        return Err(error(ErrorCode::Busy, "preview_limit"));
    }
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| error(ErrorCode::Unavailable, "preview_unavailable"))?;
    let preview_id = crate::devices::hex(&bytes);
    let expires_at = corral_core::clock::now()
        .checked_add(SignedDuration::from_secs(60))
        .unwrap_or(Timestamp::MAX)
        .to_string();
    previews.insert(
        preview_id.clone(),
        Preview {
            device: device.into(),
            digest: digest.clone(),
            deadline: now + Duration::from_secs(60),
            target: None,
        },
    );
    Ok(ActPreview {
        preview_id,
        digest,
        class,
        effects,
        expires_at,
        operation_token: None,
    })
}

fn consume(
    service: &Service,
    device: &str,
    expected: &str,
    proof: Option<&ConfirmProof>,
) -> Result<(), &'static str> {
    let proof = proof.ok_or("confirmation_required")?;
    let mut previews = lock(&service.previews);
    let valid = previews.get(&proof.preview_id).is_some_and(|preview| {
        preview.deadline > Instant::now()
            && preview.device == device
            && preview.digest == expected
            && proof.digest == expected
    });
    if !valid {
        return Err("confirmation_required");
    }
    previews.remove(&proof.preview_id);
    Ok(())
}

pub(crate) fn preview(
    shared: &Shared,
    device: &str,
    mut request: ActPreviewRequest,
) -> AppResult<ActPreview> {
    let (_, authority) = authenticate(shared, device)?;
    let class =
        policy(&authority, &request.command).map_err(|(code, message)| error(code, message))?;
    require_expect(&request.command, request.expect.as_ref())?;
    let service = service(shared).ok_or_else(|| error(ErrorCode::Unavailable, "unavailable"))?;
    let (operation_token, effects) = operation_preview(&service, &mut request.command)?;
    let target = command::app_target_fingerprint(&lock(&service.journal), &request.command)
        .map_err(|problem| error(ErrorCode::Unavailable, format!("{problem:#}")))?;
    let mut preview = issue(
        &service,
        device,
        action_digest(device, &request.command, request.expect.as_ref())?,
        class,
        effects,
    )?;
    preview.operation_token = operation_token;
    if let Some(stored) = lock(&service.previews).get_mut(&preview.preview_id) {
        stored.target = target;
    }
    Ok(preview)
}

/// App proofs supplement, never replace, the canonical finite-scope grant approval.
fn operation_preview(
    service: &Service,
    command: &mut Command,
) -> AppResult<(Option<String>, Vec<String>)> {
    if let Command::Session(corral_protocol::cmd::session::SessionCommand::Stop {
        id,
        generation,
        ..
    }) = command
    {
        let ctx = Ctx::from_store(lock(&service.journal).clone());
        let (resolved_generation, mut effects) =
            corral_ops::session_control::stop_preview(&ctx, id, *generation)
                .map_err(|problem| error(ErrorCode::Conflict, format!("{problem:#}")))?;
        *generation = resolved_generation;
        effects.insert(0, "Durably record the owner's stop before stopping any host or pane; retain the transcript".into());
        return Ok((None, effects));
    }
    let Command::Mode(mode @ corral_protocol::cmd::mode::ModeCommand::Autopilot { .. }) = command
    else {
        return Ok((None, vec![format!("Execute {} as owner", command.name())]));
    };
    let preview = command::mode::autopilot_preview(&lock(&service.journal), mode)
        .map_err(|problem| error(ErrorCode::Conflict, format!("{problem:#}")))?;
    if let corral_protocol::cmd::mode::ModeCommand::Autopilot { confirm, .. } = mode {
        *confirm = preview.id.clone();
    }
    let authority = &preview.policy.authority;
    let scope = &preview.policy.scope;
    let mut effects = vec![
        format!(
            "Autopilot: {} resolved tasks, {} future-project grants",
            scope.tasks.len(),
            scope.projects.len()
        ),
        format!(
            "Authority: dispatch={} accept={} plan_accept={} aggregate_accept={}",
            authority.dispatch, authority.accept, authority.plan_accept, authority.aggregate_accept
        ),
        format!(
            "{} task exclusions; {} selected plan scopes",
            scope.exclusions.len(),
            scope.plans.len()
        ),
        format!("Grant digest: {}", preview.grant_digest),
        format!(
            "Grant limits: days={:?}; max_new_tasks={:?}",
            scope.expires_after_days, scope.max_new_tasks
        ),
    ];
    for id in scope.tasks.keys().take(6) {
        effects.push(format!("Task: {id}"));
    }
    if scope.tasks.len() > 6 {
        effects.push(format!(
            "{} additional resolved tasks bound to this preview",
            scope.tasks.len() - 6
        ));
    }
    for id in scope.projects.keys().take(3) {
        effects.push(format!("Future project: {id}"));
    }
    if scope.projects.len() > 3 {
        effects.push(format!(
            "{} additional future projects bound to this preview",
            scope.projects.len() - 3
        ));
    }
    for effect in &mut effects {
        if effect.len() > 256 {
            let mut end = 253;
            while !effect.is_char_boundary(end) {
                end -= 1;
            }
            effect.truncate(end);
            effect.push_str("...");
        }
    }
    Ok((Some(preview.id), effects))
}

/// The only app caller constructor: copy only the daemon's four runtime environment fields.
fn caller(device: &str) -> Caller {
    let daemon = env_from_process();
    Caller {
        principal: Principal::Device { id: device.into() },
        client: ClientKind::App,
        actor: Some(ActorName::Owner),
        via: Via::App,
        assistant: None,
        planning_safe: false,
        cwd: None,
        env: CallerEnv {
            corral_herdr_session: daemon.corral_herdr_session,
            herdr_env: daemon.herdr_env,
            herdr_socket_path: daemon.herdr_socket_path,
            herdr_session: daemon.herdr_session,
            ..CallerEnv::default()
        },
    }
}

pub(crate) fn submit(shared: &Shared, device: &str, request: ActSubmit) -> AppResult<CmdResult> {
    let (_, authority) = authenticate(shared, device)?;
    policy(&authority, &request.command).map_err(|(code, message)| error(code, message))?;
    require_expect(&request.command, request.expect.as_ref())?;
    if !valid_cmd_id(&request.cmd_id) {
        return Err(error(ErrorCode::InvalidParams, "invalid_cmd_id"));
    }
    let (Ok(issued), Ok(expires)) = (
        request.issued_at.parse::<Timestamp>(),
        request.expires_at.parse::<Timestamp>(),
    ) else {
        return Err(error(ErrorCode::InvalidParams, "invalid_expiry"));
    };
    if expires < issued {
        return Err(error(ErrorCode::InvalidParams, "invalid_expiry"));
    }
    if corral_core::clock::now() > expires {
        return Err(error(ErrorCode::Expired, "expired"));
    }
    let service = service(shared).ok_or_else(|| error(ErrorCode::Unavailable, "unavailable"))?;
    let cmd_id = request.cmd_id;
    let digest = action_digest(device, &request.command, request.expect.as_ref())?;
    let payload = Payload::Command {
        submit: CmdSubmit {
            cmd_id: cmd_id.clone(),
            caller: caller(device),
            client_build: None,
            expect: request.expect,
            issued_at: request.issued_at,
            expires_at: request.expires_at,
            command: request.command,
        },
        confirm: request.confirm,
    };
    let key = journal_key(device, &cmd_id);
    queue(&service, cmd_id, key, digest, payload)
}

fn queue(
    service: &Service,
    cmd_id: String,
    key: String,
    digest: String,
    payload: Payload,
) -> AppResult<CmdResult> {
    let handoff = Arc::new(AtomicU8::new(QUEUED));
    let (reply, replies) = mpsc::channel();
    let job = Job {
        key,
        digest,
        payload,
        handoff: Arc::clone(&handoff),
        reply,
    };
    service
        .jobs
        .try_send(job)
        .map_err(|problem| match problem {
            mpsc::TrySendError::Full(_) => error(ErrorCode::Busy, "queue_full"),
            mpsc::TrySendError::Disconnected(_) => error(ErrorCode::Unavailable, "unavailable"),
        })?;
    let deadline = Instant::now() + REPLY_WAIT;
    let answer = match replies.recv_timeout(REPLY_WAIT) {
        Ok(Reply::Answer(answer)) => answer,
        Ok(Reply::Wait(flight)) => match flight.wait(deadline) {
            None => Answer::bare(CmdState::Running),
            Some(Some(answer)) => Answer {
                replayed: true,
                ..answer
            },
            Some(None) => return Err(error(ErrorCode::Busy, "queue_timeout")),
        },
        Ok(Reply::Refuse(code, message)) => return Err(error(code, message)),
        Err(problem) => {
            if handoff
                .compare_exchange(QUEUED, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Err(match problem {
                    mpsc::RecvTimeoutError::Timeout => error(ErrorCode::Busy, "queue_timeout"),
                    mpsc::RecvTimeoutError::Disconnected => {
                        error(ErrorCode::Unavailable, "unavailable")
                    }
                });
            }
            Answer::bare(CmdState::Running)
        }
    };
    Ok(control_result(cmd_id, answer))
}

pub(crate) fn result(shared: &Shared, device: &str, query: CmdResultQuery) -> AppResult<CmdResult> {
    authenticate(shared, device)?;
    if !valid_cmd_id(&query.cmd_id) {
        return Err(error(ErrorCode::InvalidParams, "invalid_cmd_id"));
    }
    let service = service(shared).ok_or_else(|| error(ErrorCode::Unavailable, "unavailable"))?;
    let deadline =
        Instant::now() + Duration::from_millis(u64::from(query.wait_ms.min(RESULT_WAIT_MAX_MS)));
    let answer = service
        .query(
            &journal_key(device, &query.cmd_id),
            deadline,
            query.abandon_if_absent,
        )
        .map_err(|problem| {
            let (code, message) = problem.wire();
            error(code, message)
        })?;
    Ok(control_result(query.cmd_id, answer))
}

fn control_result(cmd_id: String, answer: Answer) -> CmdResult {
    CmdResult {
        cmd_id,
        state: answer.state,
        exit: answer.exit,
        error: answer.error,
        output: None,
        replayed: answer.replayed,
    }
}

fn journal_key(device: &str, cmd_id: &str) -> String {
    format!("dev-{device}/{cmd_id}")
}

pub(super) fn authorize(
    shared: &Shared,
    submit: &CmdSubmit,
) -> Result<(), (ErrorCode, &'static str)> {
    let Principal::Device { id } = &submit.caller.principal else {
        return Err((ErrorCode::Unauthenticated, "unauthenticated"));
    };
    let (_, authority) =
        authenticate(shared, id).map_err(|(code, _)| (code, "device_unavailable"))?;
    policy(&authority, &submit.command)?;
    if corral_core::clock::now() > expiry(&submit.expires_at) {
        return Err((ErrorCode::Expired, "expired"));
    }
    Ok(())
}

pub(super) fn consume_confirmation(
    service: &Service,
    store: &Store,
    submit: &CmdSubmit,
    proof: Option<&ConfirmProof>,
) -> Result<Option<String>, (ErrorCode, &'static str)> {
    if !matches!(
        command::app_class(&submit.command),
        ActClass::Confirm | ActClass::Admin
    ) {
        return Ok(None);
    }
    let refused = (ErrorCode::ConfirmRequired, "confirmation_required");
    let Principal::Device { id } = &submit.caller.principal else {
        return Err(refused);
    };
    let proof = proof.ok_or(refused)?;
    let digest = action_digest(id, &submit.command, submit.expect.as_ref()).map_err(|_| refused)?;
    let target = lock(&service.previews)
        .get(&proof.preview_id)
        .filter(|preview| {
            preview.device == *id
                && preview.digest == digest
                && proof.digest == digest
                && preview.deadline > Instant::now()
        })
        .map(|preview| preview.target.clone())
        .ok_or(refused)?;
    let current = command::app_target_fingerprint(store, &submit.command)
        .map_err(|_| (ErrorCode::Unavailable, "preview_target_unavailable"))?;
    if current != target {
        return Err((ErrorCode::Conflict, "preview_changed"));
    }
    consume(service, id, &digest, Some(proof)).map_err(|_| refused)?;
    Ok(target)
}

pub(super) fn check_target(
    store: &Store,
    command: &Command,
    target: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(target) = target {
        let current = command::app_target_fingerprint(store, command)?;
        if current.as_deref() != Some(target) {
            return Err(
                corral_core::backend::ApplyError::Precondition("preview_changed".into()).into(),
            );
        }
    }
    Ok(())
}

pub(super) fn execution_invoker(
    shared: &Shared,
    caller: Caller,
    command: &Command,
) -> anyhow::Result<Invoker> {
    let Principal::Device { id } = &caller.principal else {
        return Ok(Invoker::from_caller(caller));
    };
    let (info, authority) =
        authenticate(shared, id).map_err(|(_, message)| anyhow::anyhow!("{message}"))?;
    policy(&authority, command).map_err(|(_, message)| anyhow::anyhow!("{message}"))?;
    let admin = authority.admin;
    let device = AuthenticatedDevice {
        id: id.clone(),
        name: info.name,
        assistant: authority.assistant,
        assistant_act: authority.assistant_act,
        reply: authority.reply,
        act: authority.act,
    };
    Invoker::authenticated_device(caller, device, admin)
}

pub(super) fn retention(key: &str) -> Option<String> {
    key.starts_with("dev-").then(|| {
        corral_core::clock::now()
            .checked_add(SignedDuration::from_secs(
                corral_protocol::app::DEVICE_COMMAND_RETENTION_SECS as i64,
            ))
            .unwrap_or(Timestamp::MAX)
            .to_string()
    })
}

fn prompt_digest(device: &str, key: &str, rev: u32, choice: &PromptChoice) -> AppResult<String> {
    digest(&("app.prompt.v1", device, key, rev, choice))
}

pub(crate) fn prompt_preview(
    shared: &Shared,
    device: &str,
    request: PromptPreview,
) -> AppResult<ActPreview> {
    let (_, authority) = authenticate(shared, device)?;
    if !authority.assistant || !authority.act {
        return Err(error(ErrorCode::Refused, "authority_act"));
    }
    if matches!(request.choice, PromptChoice::Other) {
        return Err(error(ErrorCode::InvalidParams, "invalid_prompt_choice"));
    }
    let service = service(shared).ok_or_else(|| error(ErrorCode::Unavailable, "unavailable"))?;
    issue(
        &service,
        device,
        prompt_digest(device, &request.key, request.rev, &request.choice)?,
        ActClass::Confirm,
        vec![format!("Answer prompt revision {}", request.rev)],
    )
}

pub(crate) fn consume_prompt(
    shared: &Shared,
    device: &str,
    answer: &PromptAnswer,
) -> AppResult<Option<FreshConfirmation>> {
    let (_, authority) = authenticate(shared, device)?;
    if !authority.assistant || !authority.reply {
        return Err(error(ErrorCode::Refused, "authority_reply"));
    }
    if matches!(answer.choice, PromptChoice::Deny | PromptChoice::Decline) {
        return Ok(None);
    }
    if matches!(answer.choice, PromptChoice::Other) {
        return Err(error(ErrorCode::InvalidParams, "invalid_prompt_choice"));
    }
    if !authority.act {
        return Err(error(ErrorCode::Refused, "authority_act"));
    }
    let service = service(shared).ok_or_else(|| error(ErrorCode::Unavailable, "unavailable"))?;
    let digest = prompt_digest(device, &answer.key, answer.rev, &answer.choice)?;
    consume(&service, device, &digest, answer.confirm.as_ref())
        .map_err(|message| error(ErrorCode::ConfirmRequired, message))?;
    Ok(Some(FreshConfirmation::validated(
        device,
        &answer.key,
        answer.rev,
        answer.choice,
    )))
}

/// Prompt answers settle in the same device journal and worker queue as actions.
pub(crate) fn prompt_submit(
    shared: &Shared,
    device: &str,
    request: PromptAnswer,
) -> AppResult<CmdResult> {
    authenticate(shared, device)?;
    if !valid_cmd_id(&request.cmd_id) {
        return Err(error(ErrorCode::InvalidParams, "invalid_cmd_id"));
    }
    let service = service(shared).ok_or_else(|| error(ErrorCode::Unavailable, "unavailable"))?;
    let digest = prompt_digest(device, &request.key, request.rev, &request.choice)?;
    let key = journal_key(device, &request.cmd_id);
    queue(
        &service,
        request.cmd_id.clone(),
        key,
        digest,
        Payload::Prompt {
            device: device.into(),
            request,
        },
    )
}

pub(super) fn accept_prompt(
    service: &Service,
    worker: &mut Worker,
    shared: &Shared,
    device: &str,
    request: PromptAnswer,
    key: &str,
    digest: &str,
) -> (Reply, bool) {
    let mut index = lock(&service.index);
    match worker.base.store.record::<Journal>(COMMANDS_AREA, key) {
        Ok(Some(_)) => return (existing(&worker.base.store, key, digest), false),
        Ok(None) => {}
        Err(_) => {
            return (
                Reply::Refuse(ErrorCode::Unavailable, "journal_unavailable"),
                false,
            );
        }
    }
    service.evict(&worker.base.store, key, &mut index);
    if index.full(key) {
        return (Reply::Refuse(ErrorCode::Busy, "queue_full"), false);
    }
    let confirmation = match consume_prompt(shared, device, &request) {
        Ok(proof) => proof,
        Err((code, message)) => {
            let reason = match message.as_str() {
                "authority_act" => "authority_act",
                "authority_reply" => "authority_reply",
                "confirmation_required" => "confirmation_required",
                "device_revoked" => "device_revoked",
                "invalid_prompt_choice" => "invalid_prompt_choice",
                _ => "device_unavailable",
            };
            return (Reply::Refuse(code, reason), false);
        }
    };
    let now = corral_core::clock::now();
    let reserved = Journal {
        digest: digest.into(),
        state: JournalState::Reserved,
        issued_at: now.to_string(),
        expires_at: now
            .checked_add(SignedDuration::from_secs(120))
            .unwrap_or(Timestamp::MAX)
            .to_string(),
        result: None,
        retain_until: retention(key),
    };
    match worker
        .base
        .store
        .insert_record(COMMANDS_AREA, key, &reserved)
    {
        Ok(true) => index.live(key),
        Ok(false) => return (existing(&worker.base.store, key, digest), false),
        Err(_) => {
            return (
                Reply::Refuse(ErrorCode::Unavailable, "journal_unavailable"),
                false,
            );
        }
    }
    drop(index);
    let outcome = (|| -> anyhow::Result<()> {
        // Reload immediately before the canonical side effect, not from the queued session.
        let (info, authority) =
            authenticate(shared, device).map_err(|(_, message)| anyhow::anyhow!("{message}"))?;
        anyhow::ensure!(authority.assistant && authority.reply, "authority_reply");
        if !matches!(request.choice, PromptChoice::Deny | PromptChoice::Decline) {
            anyhow::ensure!(authority.act, "authority_act");
        }
        let admin = authority.admin;
        let device = AuthenticatedDevice {
            id: device.into(),
            name: info.name,
            assistant: authority.assistant,
            assistant_act: authority.assistant_act,
            reply: authority.reply,
            act: authority.act,
        };
        let invoker = Invoker::authenticated_device(caller(&device.id), device, admin)?;
        let store = worker.base.store.reattributed();
        invoker.apply(&store)?;
        let ctx = Ctx {
            store,
            herdr: worker.base.herdr.clone(),
            resource_probe: Arc::clone(&worker.base.resource_probe),
        };
        corral_ops::agent::app_runtime::input::prompt_answer(
            &ctx,
            invoker.device()?,
            &request,
            confirmation.as_ref(),
        )?;
        Ok(())
    })();
    let mut answer = match outcome {
        Ok(()) => Answer::bare(CmdState::Done),
        Err(problem) => failed(&render::ErrorStyle::Plain, problem),
    };
    if answer
        .error
        .as_ref()
        .is_some_and(|error| error.code == "conflict")
    {
        answer.state = CmdState::Conflict;
    }
    answer.output = None;
    let settled = Journal {
        state: JournalState::Settled,
        result: Some(answer.stored()),
        retain_until: retention(key),
        ..reserved
    };
    if let Err(problem) = worker.base.store.save_record(COMMANDS_AREA, key, &settled) {
        eprintln!("corrald settling prompt {key}: {problem:#}");
    }
    lock(&service.index).done(key, journal::eviction_time(key, &settled));
    (Reply::Answer(answer), true)
}
#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
