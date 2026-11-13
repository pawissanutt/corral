//! Where a command executes: through `corrald` when it provably can, else in this process.
//!
//! A command runs locally only when the daemon provably did not accept it: kill switch, owner
//! action, pinned clock, no daemon, no `cmd.v1`, an oversized request, a refusing JSON-RPC error,
//! or a lost answer whose id `cmd.result` tombstoned. Once the `cmd.submit` write has started and
//! no JSON-RPC error came back, the daemon's answer stands; when none arrives before the submit's
//! own `expires_at` (or the interactive budget), the result is reported unknown or still running.
use std::path::Path;

use corral_protocol::cmd::{CmdError, CmdState};
use corral_protocol::rpc::ErrorCode;
use serde::{Deserialize, Serialize};

use super::{Command, Invoker, Outcome, local};

#[cfg(test)]
#[path = "route_tests.rs"]
mod tests;

/// `CORRAL_DAEMON_COMMANDS=0` keeps every command in this process.
pub const DISABLE_ENV: &str = "CORRAL_DAEMON_COMMANDS";
/// Last recorded local fallback, in the workspace runtime directory.
const FALLBACK_FILE: &str = "command-fallback.json";

/// Which path produced an `Outcome`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutePath {
    Daemon,
    Local(LocalReason),
}

/// Why a command ran in this process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalReason {
    /// `CORRAL_DAEMON_COMMANDS=0`.
    Disabled,
    OwnerAction,
    PinnedClock,
    /// Socket missing, connection refused or connect timeout.
    Absent,
    /// The welcome lacks `cmd.v1`, or hello failed.
    NoCapability,
    /// The serialized request exceeds one CONTROL frame.
    TooLarge,
    /// JSON-RPC refused/busy/expired/method_not_found/unavailable; the message is the code.
    Refused(String),
    /// The response was lost and `cmd.result` tombstoned the id.
    Abandoned,
}

impl LocalReason {
    /// The reason as `command-fallback.json` and `corral daemon status` show it.
    pub fn label(&self) -> &str {
        match self {
            Self::Disabled => "disabled",
            Self::OwnerAction => "owner_action",
            Self::PinnedClock => "pinned_clock",
            Self::Absent => "absent",
            Self::NoCapability => "no_capability",
            Self::TooLarge => "too_large",
            Self::Refused(code) => code,
            Self::Abandoned => "abandoned",
        }
    }

    /// A daemon was there but could not take the command: worth showing in `daemon status`.
    fn recorded(&self) -> bool {
        !matches!(
            self,
            Self::Disabled | Self::OwnerAction | Self::PinnedClock | Self::Absent
        )
    }
}

/// How long a caller may wait on the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Patience {
    /// The CLI: waits until its submit expires.
    Cli,
    /// The classic board: a short connect budget and at most about 5 s in total.
    Interactive,
}

/// Routes `command` for a CLI caller. Never panics, never fails: failures are outcomes.
pub fn submit(root: &Path, invoker: &Invoker, command: Command) -> Outcome {
    submit_with(root, invoker, command, Patience::Cli)
}

/// Routes `command` with the given patience. Never panics, never fails: failures are outcomes.
pub fn submit_with(
    root: &Path,
    invoker: &Invoker,
    command: Command,
    patience: Patience,
) -> Outcome {
    let _span = corral_core::timing::span("command.route");
    if let Some(reason) = before_connect(invoker) {
        return run_local(root, invoker, command, reason);
    }
    match daemon::route(root, invoker, command, patience) {
        Routed::Done(outcome) => outcome,
        Routed::Local(reason, command) => {
            if reason.recorded() {
                record_fallback(root, &command.name(), &reason);
            }
            run_local(root, invoker, command, reason)
        }
    }
}

/// The reasons decided without touching the socket.
fn before_connect(invoker: &Invoker) -> Option<LocalReason> {
    if std::env::var_os(DISABLE_ENV).is_some_and(|value| value == "0") {
        Some(LocalReason::Disabled)
    } else if invoker.owner_action.is_some() {
        Some(LocalReason::OwnerAction)
    } else if corral_core::clock::is_pinned() {
        Some(LocalReason::PinnedClock)
    } else {
        None
    }
}

fn run_local(root: &Path, invoker: &Invoker, command: Command, reason: LocalReason) -> Outcome {
    let mut outcome = local::run(root, invoker, command);
    outcome.path = RoutePath::Local(reason);
    outcome
}

/// Where the daemon path ended.
// One value per routed command; boxing the command would buy nothing.
#[allow(clippy::large_enum_variant)]
enum Routed {
    Done(Outcome),
    /// Not accepted: the command comes back unexecuted, to run in this process.
    Local(LocalReason, Command),
}

/// One daemon answer, reduced to what the fallback rule looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) enum Reply {
    /// Connect failed, or connect and hello did not finish within the budget.
    Absent,
    /// Hello failed or the welcome lacks `cmd.v1`.
    NoCapability,
    /// The serialized request exceeds one CONTROL frame; nothing was written.
    TooLarge,
    /// `cmd.submit` answered this JSON-RPC error and message: the command was not accepted.
    Rejected(ErrorCode, String),
    /// The daemon reported this state for the command.
    State(CmdState),
    /// After the submit write started: write error, read timeout, EOF or malformed answer.
    Lost,
    /// The wait budget ran out before a final state.
    OutOfTime,
}

/// What the router does next.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) enum Decision {
    /// Provably not accepted: run in this process.
    Local(LocalReason),
    /// Accepted and still running: query again on the same connection.
    Requery,
    /// Possibly accepted with the answer lost: reconnect and query with `abandon_if_absent`.
    Recover,
    /// The daemon's answer, or an unknown result, is the outcome. Never runs locally.
    Finish,
}

/// The fallback rule (contract section 5), as a pure function of the latest answer.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn decide(reply: &Reply) -> Decision {
    match reply {
        Reply::Absent => Decision::Local(LocalReason::Absent),
        Reply::NoCapability => Decision::Local(LocalReason::NoCapability),
        Reply::TooLarge => Decision::Local(LocalReason::TooLarge),
        Reply::Rejected(
            code @ (ErrorCode::Refused
            | ErrorCode::Busy
            | ErrorCode::Expired
            | ErrorCode::MethodNotFound
            | ErrorCode::Unavailable),
            message,
        ) => Decision::Local(LocalReason::Refused(code_token(*code, message))),
        Reply::Rejected(..) | Reply::OutOfTime => Decision::Finish,
        Reply::State(CmdState::Running) => Decision::Requery,
        Reply::State(CmdState::Abandoned) => Decision::Local(LocalReason::Abandoned),
        Reply::State(_) => Decision::Finish,
        Reply::Lost => Decision::Recover,
    }
}

/// The error's message when it is a bare `snake_case` code (`client_mismatch`), else the
/// JSON-RPC code's name (`method_not_found`).
#[cfg_attr(not(unix), allow(dead_code))]
fn code_token(code: ErrorCode, message: &str) -> String {
    let mut bytes = message.bytes();
    let token = bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    if token {
        return message.to_owned();
    }
    match serde_json::to_value(code) {
        Ok(serde_json::Value::String(name)) => name,
        _ => "refused".to_owned(),
    }
}

/// An answer that is final but carries no executor result.
#[cfg_attr(not(unix), allow(dead_code))]
fn unexecuted(state: CmdState, code: String, message: String) -> Outcome {
    Outcome {
        state,
        exit: 1,
        stdout: String::new(),
        stderr: format!("error: {message}\n"),
        error: Some(CmdError { code, message }),
        path: RoutePath::Daemon,
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
fn unknown(cmd_id: &str) -> Outcome {
    unexecuted(
        CmdState::Unknown,
        "unknown".to_owned(),
        format!(
            "corrald accepted command {cmd_id} but its result is unknown; inspect the task before retrying"
        ),
    )
}

#[cfg_attr(not(unix), allow(dead_code))]
fn still_running(cmd_id: &str) -> Outcome {
    unexecuted(
        CmdState::Running,
        "running".to_owned(),
        format!("corrald is still running command {cmd_id}; check the task before repeating"),
    )
}

#[cfg_attr(not(unix), allow(dead_code))]
fn rejected(code: ErrorCode, message: &str) -> Outcome {
    unexecuted(
        CmdState::Failed,
        code_token(code, message),
        format!("daemon rejected command: {message}"),
    )
}

/// `command-fallback.json`: the last fallback a reachable daemon could not take.
#[derive(Serialize, Deserialize)]
struct Fallback {
    at: String,
    command: String,
    reason: String,
    pid: u32,
}

/// Best effort: a failed write never affects the command.
fn record_fallback(root: &Path, command: &str, reason: &LocalReason) {
    let dir = crate::daemon_client::runtime_dir_of(&root.join(".corral"));
    let record = Fallback {
        at: corral_core::clock::now().to_string(),
        command: command.to_owned(),
        reason: reason.label().to_owned(),
        pid: std::process::id(),
    };
    let Ok(bytes) = serde_json::to_vec(&record) else {
        return;
    };
    let temp = dir.join(format!("{FALLBACK_FILE}.{}.tmp", record.pid));
    if std::fs::write(&temp, bytes).is_ok()
        && std::fs::rename(&temp, dir.join(FALLBACK_FILE)).is_err()
    {
        let _ = std::fs::remove_file(&temp);
    }
}

/// `commands: last local fallback {command} ({reason}) at {at}` while the workspace has a
/// recorded fallback; `store_dir` is the `.corral` directory.
pub fn fallback_line(store_dir: &Path) -> Option<String> {
    let path = crate::daemon_client::runtime_dir_of(store_dir).join(FALLBACK_FILE);
    let bytes = std::fs::read(path).ok()?;
    let record: Fallback = serde_json::from_slice(&bytes).ok()?;
    Some(format!(
        "commands: last local fallback {} ({}) at {}",
        record.command, record.reason, record.at
    ))
}

#[cfg(not(unix))]
mod daemon {
    use std::path::Path;

    use super::{Command, Invoker, LocalReason, Patience, Routed};

    pub(super) fn route(_: &Path, _: &Invoker, command: Command, _: Patience) -> Routed {
        Routed::Local(LocalReason::Absent, command)
    }
}

/// The socket round trip: submit, then re-query until a final state, a tombstone or the deadline.
#[cfg(unix)]
mod daemon {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use corral_protocol::cmd::{ClientBuild, CmdOutput, CmdResultQuery, CmdState, CmdSubmit};

    use super::{
        Command, Decision, Invoker, LocalReason, Outcome, Patience, Reply, RoutePath, Routed,
        decide, rejected, still_running, unknown,
    };
    use crate::daemon_client::{Client, CmdReply, Connected, SubmitReply};

    /// The daemon answers a submit within its 25 s queue hand-off window.
    const SUBMIT_REPLY: Duration = Duration::from_secs(30);
    /// The longest `cmd.result` wait the daemon honours.
    const MAX_WAIT: Duration = Duration::from_millis(25_000);
    /// Transfer allowance on top of a `cmd.result` wait.
    const REPLY_SLACK: Duration = Duration::from_secs(5);
    /// Kept between a query's wait and the caller's deadline so the answer can still arrive.
    const GRACE: Duration = Duration::from_millis(500);
    /// After a lost answer, consecutive connect failures are retried this long.
    const RECONNECT_WINDOW: Duration = Duration::from_secs(30);
    /// One reconnect attempt: connect plus hello.
    const RECONNECT: Duration = Duration::from_secs(2);
    const RETRY_PAUSE: Duration = Duration::from_millis(200);

    struct Budget {
        /// Connect plus hello.
        connect: Duration,
        /// Total wait for a result, counted from the start of routing.
        wait: Duration,
        /// `expires_at - issued_at`, whole seconds.
        expiry: i64,
    }

    fn budget(patience: Patience) -> Budget {
        match patience {
            // As other daemon clients: 2 s to connect and 2 s for hello.
            Patience::Cli => Budget {
                connect: Duration::from_secs(4),
                wait: Duration::from_secs(120),
                expiry: 120,
            },
            Patience::Interactive => Budget {
                connect: Duration::from_secs(2),
                wait: Duration::from_secs(5),
                expiry: 10,
            },
        }
    }

    pub(super) fn route(
        root: &Path,
        invoker: &Invoker,
        command: Command,
        patience: Patience,
    ) -> Routed {
        let started = Instant::now();
        let budget = budget(patience);
        let wait_until = started + budget.wait;
        let socket = crate::daemon_client::socket_path_of(&root.join(".corral"));
        let mut client = match Client::connect_commands(&socket, started + budget.connect) {
            Connected::Absent => return unaccepted(&Reply::Absent, command),
            Connected::Unusable => return unaccepted(&Reply::NoCapability, command),
            Connected::Ready(client) => client,
        };
        // Only a real socket round trip is timed: connect + hello succeeded with `cmd.v1`.
        let _span = corral_core::timing::span("command.daemon");
        // Commands are served only to the daemon's own executable and build, as reads are.
        let Ok((exe, build)) = crate::pm_read::client_identity() else {
            return Routed::Local(LocalReason::Refused("client_mismatch".to_owned()), command);
        };
        let issued = corral_core::clock::now();
        let Ok(expires) = issued.checked_add(jiff::SignedDuration::from_secs(budget.expiry)) else {
            return Routed::Local(LocalReason::Refused("expired".to_owned()), command);
        };
        let submit = CmdSubmit {
            cmd_id: cmd_id(issued),
            caller: invoker.caller.clone(),
            client_build: Some(ClientBuild { exe, build }),
            expect: None,
            issued_at: issued.to_string(),
            expires_at: expires.to_string(),
            command,
        };
        let submitted = client.submit(&submit, (Instant::now() + SUBMIT_REPLY).min(wait_until));
        let (mut reply, mut answer) = match submitted {
            Ok(SubmitReply::Unsupported) => (Reply::NoCapability, None),
            Ok(SubmitReply::TooLarge) => (Reply::TooLarge, None),
            Ok(SubmitReply::Rejected(error)) => (Reply::Rejected(error.code, error.message), None),
            Ok(SubmitReply::Result(answer)) => (Reply::State(answer.result.state), Some(answer)),
            Err(_) => (Reply::Lost, None),
        };
        let mut client = Some(client);
        let mut reconnected = false;
        let mut lost_since: Option<Instant> = None;
        loop {
            match decide(&reply) {
                Decision::Local(reason) => return Routed::Local(reason, submit.command),
                Decision::Finish => {
                    return Routed::Done(finish(&submit.cmd_id, reply, answer, patience));
                }
                Decision::Requery => lost_since = None,
                Decision::Recover => {
                    client = None;
                    if lost_since.is_some() {
                        // A repeated failure: pause before the next connect attempt.
                        let left = wait_until.saturating_duration_since(Instant::now());
                        std::thread::sleep(RETRY_PAUSE.min(left));
                    } else {
                        lost_since = Some(Instant::now());
                    }
                }
            }
            let now = Instant::now();
            let remaining = wait_until.saturating_duration_since(now);
            if remaining <= GRACE || lost_since.is_some_and(|since| now >= since + RECONNECT_WINDOW)
            {
                (reply, answer) = (Reply::OutOfTime, None);
                continue;
            }
            if client.is_none() {
                match Client::connect_commands(&socket, (now + RECONNECT).min(wait_until)) {
                    Connected::Ready(fresh) => {
                        client = Some(fresh);
                        // A fresh connection tombstones an id the daemon never received.
                        reconnected = true;
                    }
                    Connected::Absent | Connected::Unusable => {
                        (reply, answer) = (Reply::Lost, None);
                        continue;
                    }
                }
            }
            let Some(connection) = client.as_mut() else {
                (reply, answer) = (Reply::Lost, None);
                continue;
            };
            let wait = remaining.saturating_sub(GRACE).min(MAX_WAIT);
            let query = CmdResultQuery {
                cmd_id: submit.cmd_id.clone(),
                wait_ms: u32::try_from(wait.as_millis()).unwrap_or(25_000),
                abandon_if_absent: reconnected,
            };
            let deadline = (Instant::now() + wait + REPLY_SLACK).min(wait_until);
            (reply, answer) = match connection.result(&query, deadline) {
                Ok(answer) => (Reply::State(answer.result.state), Some(answer)),
                Err(_) => (Reply::Lost, None),
            };
        }
    }

    /// A reply given before anything was written: nothing ran, so the command stays local.
    fn unaccepted(reply: &Reply, command: Command) -> Routed {
        match decide(reply) {
            Decision::Local(reason) => Routed::Local(reason, command),
            // Not reached for pre-write replies; running locally is safe either way.
            Decision::Requery | Decision::Recover | Decision::Finish => {
                Routed::Local(LocalReason::NoCapability, command)
            }
        }
    }

    fn finish(cmd_id: &str, reply: Reply, answer: Option<CmdReply>, patience: Patience) -> Outcome {
        match (reply, answer) {
            (Reply::Rejected(code, message), _) => rejected(code, &message),
            (Reply::OutOfTime, _) if patience == Patience::Interactive => still_running(cmd_id),
            (Reply::State(CmdState::Unknown) | Reply::OutOfTime, _) | (_, None) => unknown(cmd_id),
            (_, Some(answer)) => answered(answer),
        }
    }

    /// The daemon's settled result, printed exactly as the executor buffered it.
    fn answered(answer: CmdReply) -> Outcome {
        let CmdReply { result, output } = answer;
        let output = output.unwrap_or(CmdOutput {
            stdout: String::new(),
            stderr: String::new(),
        });
        Outcome {
            state: result.state,
            exit: result.exit,
            stdout: output.stdout,
            stderr: output.stderr,
            error: result.error,
            path: RoutePath::Daemon,
        }
    }

    /// `cli-<unix nanos hex>-<pid hex>`; the process clock never repeats a stamp.
    fn cmd_id(issued: jiff::Timestamp) -> String {
        format!(
            "cli-{:x}-{:x}",
            issued.as_nanosecond().max(0),
            std::process::id()
        )
    }
}
