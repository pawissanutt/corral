//! Owner/agent commands executed into an output buffer: by `corrald` workers and by the local
//! fallback through the same `execute_in`. Family behaviour lives in sibling files.
pub mod caller;
pub mod local;
pub mod render;
pub mod route;

mod assistant;
mod attention;
mod inbox;
mod knowledge;
mod meta;
pub mod mode;
mod plan;
mod pm;
mod question;
mod queue;
mod repair;
mod run;
mod setup;
mod task;

use std::fmt::Write as _;

pub use caller::Invoker;
pub use corral_protocol::cmd::{CmdError, CmdState, Command};

use corral_core::backend::ApplyError;
use corral_protocol::cmd::{CmdSubmit, Principal};

/// What a command printed, in order per stream.
#[derive(Debug, Default)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// `println!`.
    pub fn line(&mut self, text: std::fmt::Arguments<'_>) {
        let _ = writeln!(self.stdout, "{text}");
    }

    /// `print!`.
    pub fn text(&mut self, text: std::fmt::Arguments<'_>) {
        let _ = write!(self.stdout, "{text}");
    }

    /// `eprintln!`.
    pub fn err_line(&mut self, text: std::fmt::Arguments<'_>) {
        let _ = writeln!(self.stderr, "{text}");
    }

    /// `println!("{}", serde_json::to_string_pretty(value)?)`.
    pub fn json_pretty(&mut self, value: &impl serde::Serialize) -> anyhow::Result<()> {
        let text = serde_json::to_string_pretty(value)?;
        self.line(format_args!("{text}"));
        Ok(())
    }

    /// `println!("{}", serde_json::to_string(value)?)`.
    pub fn json(&mut self, value: &impl serde::Serialize) -> anyhow::Result<()> {
        let text = serde_json::to_string(value)?;
        self.line(format_args!("{text}"));
        Ok(())
    }
}

/// One command's execution context.
pub struct Exec<'a> {
    pub ctx: crate::Ctx,
    pub invoker: &'a Invoker,
    pub out: Output,
}

/// How the local path opens the store for a command.
#[derive(Debug, Clone, Copy)]
pub struct OpenSpec {
    pub observe: bool,
    pub attribute: bool,
}

impl OpenSpec {
    /// `Entry::open`.
    pub const ENTRY: Self = Self {
        observe: true,
        attribute: true,
    };
    pub const UNOBSERVED_APPLIED: Self = Self {
        observe: false,
        attribute: true,
    };
    /// Locally an owner action's attribution still applies, as unobserved CLI opens do.
    pub const UNOBSERVED: Self = Self {
        observe: false,
        attribute: false,
    };
}

/// Per-family behaviour; each family file implements it for its protocol enum.
pub(crate) trait Verb {
    fn execute(self, cx: &mut Exec<'_>) -> anyhow::Result<()>;
    fn open(&self) -> OpenSpec {
        OpenSpec::ENTRY
    }
    /// True when the ops path can reach `ctx.herdr`, launches or cancels processes.
    fn touches_runtime(&self) -> bool {
        false
    }
    fn error_style(&self) -> render::ErrorStyle {
        render::ErrorStyle::Plain
    }
}

/// Applies `$body` to the family value of every known family arm; `$other` for `Command::Other`.
macro_rules! by_family {
    ($command:expr, $family:ident => $body:expr, $other:expr) => {
        match $command {
            Command::Assistant($family) => $body,
            Command::Attention($family) => $body,
            Command::Inbox($family) => $body,
            Command::Knowledge($family) => $body,
            Command::Meta($family) => $body,
            Command::Mode($family) => $body,
            Command::Plan($family) => $body,
            Command::Pm($family) => $body,
            Command::Question($family) => $body,
            Command::Queue($family) => $body,
            Command::Repair($family) => $body,
            Command::Run($family) => $body,
            Command::Setup($family) => $body,
            Command::Task($family) => $body,
            Command::Other => $other,
        }
    };
}

pub fn open_spec(command: &Command) -> OpenSpec {
    by_family!(command, family => family.open(), OpenSpec::ENTRY)
}

pub fn touches_runtime(command: &Command) -> bool {
    by_family!(command, family => family.touches_runtime(), false)
}

pub fn error_style(command: &Command) -> render::ErrorStyle {
    by_family!(command, family => family.error_style(), render::ErrorStyle::Plain)
}

/// A finished (or not provably finished) command.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub state: CmdState,
    pub exit: u8,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<CmdError>,
    /// Stamped by `route`; executor outcomes carry `Local(Disabled)` until then.
    pub path: route::RoutePath,
}

/// The one executor: daemon worker and local fallback both call this with a prepared Ctx.
/// Never sets attribution (a second `set_attribution` fails): `local::run` applies it in
/// `Store::open_with`'s prepare as `Entry::open` does; the daemon applies it to the
/// `reattributed()` clone.
pub fn execute_in(ctx: crate::Ctx, invoker: &Invoker, command: Command) -> Outcome {
    let _span = corral_core::timing::span("command.exec");
    if !command.is_known() {
        return unknown(&command, Output::default());
    }
    let style = error_style(&command);
    let mut cx = Exec {
        ctx,
        invoker,
        out: Output::default(),
    };
    let result =
        by_family!(command, family => family.execute(&mut cx), Err(unknown_command("other")));
    settle(&style, result, cx.out)
}

/// The error every `Other` arm returns.
pub(crate) fn unknown_command(name: &str) -> anyhow::Error {
    anyhow::anyhow!("unknown command {name}")
}

/// An unknown family or verb: `Failed`, stderr `error: unknown command <name>`.
pub(crate) fn unknown(command: &Command, out: Output) -> Outcome {
    settle(
        &render::ErrorStyle::Plain,
        Err(unknown_command(&command.name())),
        out,
    )
}

/// `ApplyError::Conflict | Precondition` → Conflict, `ApplyError::Busy` → Busy, else Failed;
/// stderr += `render::failure(style, &err)`; exit 1.
pub(crate) fn settle(
    style: &render::ErrorStyle,
    result: anyhow::Result<()>,
    mut out: Output,
) -> Outcome {
    let path = route::RoutePath::Local(route::LocalReason::Disabled);
    let error = match result {
        Ok(()) => {
            return Outcome {
                state: CmdState::Done,
                exit: 0,
                stdout: out.stdout,
                stderr: out.stderr,
                error: None,
                path,
            };
        }
        Err(error) => error,
    };
    let state = match error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ApplyError>())
    {
        Some(ApplyError::Conflict(_) | ApplyError::Precondition(_)) => CmdState::Conflict,
        Some(ApplyError::Busy { .. }) => CmdState::Busy,
        Some(ApplyError::Io(_)) | None => CmdState::Failed,
    };
    out.stderr.push_str(&render::failure(style, &error));
    let message = format!("{error:#}");
    let code = leading_code(&message).unwrap_or(match state {
        CmdState::Conflict => "conflict",
        CmdState::Busy => "busy",
        _ => "failed",
    });
    Outcome {
        state,
        exit: 1,
        stdout: out.stdout,
        stderr: out.stderr,
        error: Some(CmdError {
            code: code.to_owned(),
            message,
        }),
        path,
    }
}

/// The leading `snake_case:` token of an error message (`retry_refused: ...`), if any.
fn leading_code(message: &str) -> Option<&str> {
    let (token, _) = message.split_once(':')?;
    let mut bytes = token.bytes();
    (bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
    .then_some(token)
}

/// The idempotency digest of a submit: sha256 hex of the JSON of
/// `(command, expect, caller with principal = Unverified)`.
pub fn digest(submit: &CmdSubmit) -> String {
    let mut caller = submit.caller.clone();
    caller.principal = Principal::Unverified;
    // These protocol types always serialize; an empty input would still give a fixed digest.
    let bytes = serde_json::to_vec(&(&submit.command, &submit.expect, &caller)).unwrap_or_default();
    crate::verification::digest(&bytes)
}
