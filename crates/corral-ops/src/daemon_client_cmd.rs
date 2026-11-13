//! `cmd.v1`: submits a command to `corrald` and re-queries its result. The routing rule (when a
//! command may still run in this process) lives in `command::route`; this module only speaks the
//! wire and reports exactly what came back.
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use corral_protocol::cmd::{
    CMD_RESULT, CMD_SUBMIT, CMD_V1, CmdOutput, CmdResult, CmdResultQuery, CmdSubmit,
};
use corral_protocol::frame::CONTROL_MAX;
use corral_protocol::rpc::{self, ResponseOutcome};

use super::{Client, connect_socket};

/// One deadline for all DATA frames of a command output, as for served documents.
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(5);

/// How connecting for commands came out.
pub enum Connected {
    /// Nothing usable answered in time: the socket is missing, refused or failed the connection,
    /// or connect and hello did not finish by the deadline.
    Absent,
    /// A daemon answered but the peer check or hello failed, or its welcome lacks `cmd.v1`.
    Unusable,
    /// A daemon that negotiated `cmd.v1`.
    Ready(Client),
}

/// What `cmd.submit` answered.
#[derive(Debug)]
pub enum SubmitReply {
    /// The connection did not negotiate `cmd.v1`; nothing was written.
    Unsupported,
    /// The serialized request exceeds one CONTROL frame; nothing was written.
    TooLarge,
    /// A JSON-RPC error: the daemon did not accept the command.
    Rejected(rpc::Error),
    /// The command's state and its output.
    Result(CmdReply),
}

/// A `cmd.submit` or `cmd.result` answer with the output its DATA frames carried.
#[derive(Debug)]
pub struct CmdReply {
    pub result: CmdResult,
    /// None when the result announced no output body.
    pub output: Option<CmdOutput>,
}

impl Client {
    /// Connects and says hello, both by `deadline`. A failure after the deadline counts as absent.
    pub fn connect_commands(socket: &Path, deadline: Instant) -> Connected {
        let Ok(stream) = connect_socket(socket, deadline) else {
            return Connected::Absent;
        };
        let Some(hello) = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
        else {
            return Connected::Absent;
        };
        match Self::handshake(stream, hello) {
            Ok(client) if client.cmd_capable => Connected::Ready(client),
            Ok(_) => Connected::Unusable,
            Err(_) if Instant::now() >= deadline => Connected::Absent,
            Err(_) => Connected::Unusable,
        }
    }

    /// Sends one `cmd.submit` and reads its response and output by `deadline` (output: 5 s more).
    /// `Err` means the frame write may have started and no complete answer arrived.
    pub fn submit(&mut self, submit: &CmdSubmit, deadline: Instant) -> Result<SubmitReply> {
        if !self.cmd_capable {
            return Ok(SubmitReply::Unsupported);
        }
        let (id, payload) = self.encode(CMD_SUBMIT, submit)?;
        if payload.len() > CONTROL_MAX {
            return Ok(SubmitReply::TooLarge);
        }
        match self.exchange::<CmdResult>(id, payload, deadline)? {
            ResponseOutcome::Error { error } => Ok(SubmitReply::Rejected(error)),
            ResponseOutcome::Result { result } => {
                self.reply(&submit.cmd_id, result).map(SubmitReply::Result)
            }
        }
    }

    /// Sends one `cmd.result` and reads its response and output by `deadline` (output: 5 s more).
    /// A JSON-RPC error is an `Err`, like a lost answer.
    pub fn result(&mut self, query: &CmdResultQuery, deadline: Instant) -> Result<CmdReply> {
        if !self.cmd_capable {
            bail!("daemon does not offer {CMD_V1}");
        }
        let (id, payload) = self.encode(CMD_RESULT, query)?;
        match self.exchange::<CmdResult>(id, payload, deadline)? {
            ResponseOutcome::Error { error } => {
                bail!("daemon {:?}: {}", error.code, error.message)
            }
            ResponseOutcome::Result { result } => self.reply(&query.cmd_id, result),
        }
    }

    /// Checks the answer is for `cmd_id` and reads the announced output body.
    fn reply(&mut self, cmd_id: &str, result: CmdResult) -> Result<CmdReply> {
        if result.cmd_id != cmd_id {
            bail!(
                "daemon answered for command {} instead of {cmd_id}",
                result.cmd_id
            );
        }
        let output = match &result.output {
            None => None,
            Some(body) => {
                let bytes = self.read_body(
                    body.stream,
                    body.bytes,
                    body.frames,
                    Instant::now() + OUTPUT_TIMEOUT,
                    "command output",
                )?;
                Some(serde_json::from_slice(&bytes).context("decoding command output")?)
            }
        };
        Ok(CmdReply { result, output })
    }
}
