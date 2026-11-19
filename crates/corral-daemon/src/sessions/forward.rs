//! One bounded exchange with the host of a resolved session.
use std::{
    fmt, io,
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use corral_engine::{
    refuse,
    runner::pi::{Client, PiRefusal},
};
use corral_protocol::{app::SESSION_PART_MAX, session_host::*};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Map;

use super::{Session, no_pi_host, resolve};
use crate::{state::Shared, views::json_len};

const OLDER_HOST_HINT: &str =
    "this session's host is older than the app; stop it and resume it on the current corral-pi";

pub(crate) enum HostCall {
    Read,
    Act,
}

pub(crate) fn host_call<R: DeserializeOwned + Serialize>(
    shared: &Shared,
    id: &str,
    command: &str,
    args: impl Serialize,
    kind: HostCall,
) -> Result<R> {
    host_call_on(&resolve(shared, id)?, command, args, kind)
}

pub(crate) fn host_call_on<R: DeserializeOwned + Serialize>(
    session: &Session,
    command: &str,
    args: impl Serialize,
    kind: HostCall,
) -> Result<R> {
    let (socket, _) = session.pi_endpoint()?;
    let deadline = Instant::now()
        + match kind {
            HostCall::Read => Duration::from_secs(2),
            HostCall::Act => Duration::from_secs(10),
        };
    let stream = match connect(socket, deadline) {
        Err(error) if missing_host(&error) => return Err(no_pi_host()),
        result => {
            result.with_context(|| format!("connect to Pi session socket {}", socket.display()))?
        }
    };
    exchange(stream, command, args, deadline)
}

fn missing_host(error: &anyhow::Error) -> bool {
    use rustix::io::Errno;

    matches!(
        error.downcast_ref::<Errno>(),
        Some(&Errno::NOENT | &Errno::CONNREFUSED)
    ) || error.downcast_ref::<io::Error>().is_some_and(|error| {
        matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
        )
    })
}

fn exchange<R: DeserializeOwned + Serialize>(
    stream: UnixStream,
    command: &str,
    args: impl Serialize,
    deadline: Instant,
) -> Result<R> {
    let reply = Client::request_on(stream, command, args, deadline).map_err(host_error)?;
    ensure!(
        json_len(&reply).context("sizing typed Pi reply")? <= SESSION_PART_MAX,
        PartTooLarge
    );
    Ok(reply)
}

#[derive(Serialize)]
#[serde(untagged)]
enum ActArgs<'a> {
    QueueKey {
        key: String,
    },
    QueueMove {
        key: String,
        delta: i8,
    },
    Model {
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },
    Card {
        card: String,
        answers: Vec<CardAnswer>,
        dismiss: bool,
        device: &'a str,
    },
    Child {
        child: String,
        #[serde(flatten)]
        action: ChildAction,
    },
    Job {
        job: String,
        action: JobAction,
    },
    Compact {
        #[serde(skip_serializing_if = "Option::is_none")]
        instructions: Option<String>,
    },
    Shell {
        command: String,
        exclude_from_context: bool,
    },
}

pub(crate) fn queue_withdraw(shared: &Shared, request: SessionQueueKey) -> Result<QueueWithdrawn> {
    let SessionQueueKey { id, key } = request;
    host_call(
        shared,
        &id,
        "queue_withdraw",
        ActArgs::QueueKey { key },
        HostCall::Act,
    )
}

pub(crate) fn queue_move(shared: &Shared, request: SessionQueueMove) -> Result<QueueMoved> {
    let SessionQueueMove { id, key, delta } = request;
    host_call(
        shared,
        &id,
        "queue_move",
        ActArgs::QueueMove { key, delta },
        HostCall::Act,
    )
}

pub(crate) fn model(shared: &Shared, request: SessionModel) -> Result<SessionModelSet> {
    let SessionModel { id, model, effort } = request;
    host_call(
        shared,
        &id,
        "set_model",
        ActArgs::Model { model, effort },
        HostCall::Act,
    )
}

pub(crate) fn card_answer(
    shared: &Shared,
    device: &str,
    request: SessionCardAnswer,
) -> Result<SessionCardAnswered> {
    let SessionCardAnswer {
        id,
        card,
        answers,
        dismiss,
    } = request;
    host_call(
        shared,
        &id,
        "card_answer",
        ActArgs::Card {
            card,
            answers,
            dismiss,
            device,
        },
        HostCall::Act,
    )
}

pub(crate) fn child(shared: &Shared, request: SessionChildAct) -> Result<SessionActed> {
    let SessionChildAct { id, child, action } = request;
    ensure!(
        action != ChildAction::Other,
        "invalid: unknown child action"
    );
    host_call(
        shared,
        &id,
        "child_act",
        ActArgs::Child { child, action },
        HostCall::Act,
    )
}

pub(crate) fn job(shared: &Shared, request: SessionJobAct) -> Result<SessionJobActed> {
    let SessionJobAct { id, job, action } = request;
    ensure!(action != JobAction::Other, "invalid: unknown job action");
    host_call(
        shared,
        &id,
        "job_act",
        ActArgs::Job { job, action },
        HostCall::Act,
    )
}

pub(crate) fn action(shared: &Shared, request: SessionActionRequest) -> Result<SessionActionDone> {
    let SessionActionRequest { id, action } = request;
    let (command, args) = match action {
        SessionAction::ExitOpen => ("exit_open", None),
        SessionAction::BoardRefresh => ("board_refresh", None),
        SessionAction::Compact { instructions } => {
            ("compact", Some(ActArgs::Compact { instructions }))
        }
        SessionAction::Export => ("export", None),
        SessionAction::Shell {
            command,
            exclude_from_context,
        } => (
            "owner_bash",
            Some(ActArgs::Shell {
                command,
                exclude_from_context,
            }),
        ),
        SessionAction::Other => anyhow::bail!("invalid: unknown session action"),
    };
    match args {
        Some(args) => host_call(shared, &id, command, args, HostCall::Act),
        None => host_call(shared, &id, command, Map::new(), HostCall::Act),
    }
}

fn host_error(error: anyhow::Error) -> anyhow::Error {
    match error.downcast::<PiRefusal>() {
        Ok(error) if error.code == "unknown_command" => refuse(OLDER_HOST_HINT, vec![]),
        Ok(error) => refuse(error.reason, vec![]),
        Err(error) => error,
    }
}

#[derive(Debug)]
pub(crate) struct PartTooLarge;

impl fmt::Display for PartTooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("session part exceeds its byte bound")
    }
}

impl std::error::Error for PartTooLarge {}

pub(crate) fn connect(socket: &Path, deadline: Instant) -> Result<UnixStream> {
    use rustix::io::Errno;
    use rustix::net::{self, AddressFamily, SocketAddrUnix, SocketType};

    // A blocking connect can wait indefinitely when the listen queue is full.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let stream = net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        net::SocketFlags::CLOEXEC | net::SocketFlags::NONBLOCK,
        None,
    )?;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let stream = {
        let fd = net::socket(AddressFamily::UNIX, SocketType::STREAM, None)?;
        rustix::io::fcntl_setfd(&fd, rustix::io::FdFlags::CLOEXEC)?;
        fd
    };
    let stream = UnixStream::from(stream);
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    stream.set_nonblocking(true)?;
    let address = SocketAddrUnix::new(socket)?;
    match net::connect(&stream, &address) {
        Ok(()) => {}
        Err(Errno::INPROGRESS) => {
            use rustix::event::{PollFd, PollFlags, Timespec, poll};
            let remaining = deadline.saturating_duration_since(Instant::now());
            let mut fds = [PollFd::new(&stream, PollFlags::OUT)];
            let timeout = Timespec {
                tv_sec: remaining
                    .as_secs()
                    .try_into()
                    .context("Pi connect timeout overflow")?,
                tv_nsec: remaining.subsec_nanos().into(),
            };
            ensure!(
                poll(&mut fds, Some(&timeout))? != 0,
                "Pi connect deadline elapsed"
            );
            if let Some(error) = stream.take_error()? {
                return Err(error.into());
            }
        }
        Err(error) => return Err(error.into()),
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};

    use super::*;
    use corral_protocol::detail::JobOutputPage;
    use corral_types::Refusal;
    use serde_json::{Value, json};

    fn exchange_with_reply<R: DeserializeOwned + Serialize>(
        command: &str,
        reply: Value,
    ) -> Result<R> {
        let (client, server) = UnixStream::pair().unwrap();
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let mut server = BufReader::new(server);
                let mut line = String::new();
                server.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["command"], command);
                let mut reply = reply;
                reply["id"] = request["id"].clone();
                reply["version"] = json!(2);
                let stream = server.get_mut();
                serde_json::to_writer(&mut *stream, &reply).unwrap();
                stream.write_all(b"\n").unwrap();
            });
            exchange(
                client,
                command,
                Map::new(),
                Instant::now() + Duration::from_secs(2),
            )
        })
    }

    #[test]
    fn an_older_host_is_refused_with_a_restart_hint() {
        let error = exchange_with_reply::<SessionActionDone>(
            "compact",
            json!({ "ok": false, "error": { "code": "unknown_command", "reason": "unsupported" } }),
        )
        .unwrap_err();
        assert_eq!(error.downcast::<Refusal>().unwrap().reason, OLDER_HOST_HINT);
    }

    #[test]
    fn a_part_over_the_bound_is_refused_not_truncated() {
        let mut reply = JobOutputPage {
            text: String::new(),
            offset: 0,
            bytes: SESSION_PART_MAX as u64,
            live: false,
        };
        let len = SESSION_PART_MAX - json_len(&reply).unwrap();
        reply.text = "x".repeat(len);
        let response = |reply: &JobOutputPage| {
            let mut result = serde_json::to_value(reply).unwrap();
            // The typed bound is measured after dropping unknown fields.
            result["future_field"] = json!(true);
            json!({ "ok": true, "result": result })
        };
        let exact = exchange_with_reply::<JobOutputPage>("job_output", response(&reply)).unwrap();
        assert_eq!(exact, reply);
        reply.text.push('x');
        let error =
            exchange_with_reply::<JobOutputPage>("job_output", response(&reply)).unwrap_err();
        assert!(error.is::<PartTooLarge>());
    }
}
