//! The dedicated sandboxed shim. This process has no provider or host-poll
//! capability. Its only broker data endpoint arrives after launch on fd 0/1.
use anyhow::{Context, Result, ensure};
use rustix::{
    fs::{self, FileType, Mode, OFlags},
    net::SocketType,
};
use std::{
    io::{stderr, stdin, stdout},
    os::fd::{AsFd, AsRawFd, BorrowedFd},
    time::{Duration, Instant},
};

use super::super::wire;
use super::{
    CALL_TIMEOUT, MAX_REPLY,
    io::{self, Lines, Scope},
    protocol::{self, Arm, BindingKind, BindingRecord, Call, Probe},
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const SESSION_TIMEOUT: Duration = Duration::from_secs(120);

pub(super) fn run_inherited() -> Result<()> {
    ensure!(
        // Bubblewrap 0.9 supplies PWD even under --clearenv.
        std::env::args_os().count() == 1
            && std::env::vars_os().all(|(key, value)| key == "PWD" && value == "/scratch"),
        "unexpected fixture shim argv or environment"
    );
    let input = stdin();
    let output = stdout();
    let diagnostics = stderr();
    let fd = input.as_fd();
    let identity = io::socket_identity(fd, SocketType::STREAM)?;
    ensure!(
        io::socket_identity(output.as_fd(), SocketType::STREAM)? == identity,
        "fixture shim stdin and stdout must be the same socket"
    );
    let null = fs::fstat(diagnostics.as_fd())?;
    ensure!(
        FileType::from_raw_mode(null.st_mode) == FileType::CharacterDevice
            && fs::major(null.st_rdev) == 1
            && fs::minor(null.st_rdev) == 3,
        "fixture shim stderr must be null"
    );
    check_inventory()?;
    run(fd)
}

fn check_inventory() -> Result<()> {
    let dir = fs::open(
        "/proc/self/fd",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .context("open fixture shim descriptor inventory")?;
    ensure!(
        fs::fstatfs(&dir)?.f_type == fs::PROC_SUPER_MAGIC,
        "fixture shim inventory is not procfs"
    );
    let scan = dir.as_raw_fd();
    let mut seen = [false; 3];
    for entry in fs::Dir::new(dir)? {
        let entry = entry?;
        let raw = entry.file_name().to_bytes();
        if matches!(raw, b"." | b"..") {
            continue;
        }
        let number = std::str::from_utf8(raw)?.parse::<i32>()?;
        if number == scan {
            continue;
        }
        ensure!(
            (0..3).contains(&number),
            "unexpected inherited fixture shim descriptor"
        );
        seen[number as usize] = true;
    }
    ensure!(
        seen.into_iter().all(|x| x),
        "missing inherited fixture shim descriptor"
    );
    Ok(())
}

pub(super) fn run(stdio: BorrowedFd<'_>) -> Result<()> {
    let mut none = || Ok(());
    let launched = Instant::now();
    let session_deadline = launched + SESSION_TIMEOUT;
    let mut startup = Scope {
        deadline: launched + STARTUP_TIMEOUT,
        pump: &mut none,
    };
    let [evidence_fd, control_fd] = startup.recv_handoff(stdio)?;
    let evidence = evidence_fd.as_fd();
    let control = control_fd.as_fd();
    ensure!(
        io::socket_identity(evidence, SocketType::SEQPACKET)?
            != io::socket_identity(control, SocketType::SEQPACKET)?,
        "aliased fixture shim descriptors"
    );

    let bound = BindingRecord::parse(
        startup.recv_private(control, &[stdio, evidence])?,
        BindingKind::ShimBind,
    )?;
    startup.send_private(
        control,
        &bound.record(BindingKind::ShimBound)?,
        &[stdio, evidence],
    )?;
    let mut lines = Lines::default();
    startup_stdio(&mut startup, stdio, &mut lines, &[control, evidence])?;

    let mut last_id = 2;
    loop {
        let mut idle = Scope {
            deadline: session_deadline,
            pump: &mut none,
        };
        // The lack of an installed arm is a strict state: ANY stdio traffic
        // here is unarmed and must not reach the ledger (even a valid call).
        let record = idle.recv_private(control, &[stdio, evidence])?;
        if record["type"] == "shim_probe" {
            let probe = Probe::parse(record)?;
            idle.send_private(control, &probe.ack()?, &[stdio, evidence])?;
            continue;
        }
        let arm = Arm::parse(record, last_id)?;
        last_id = arm.id;
        let mut active = Scope {
            deadline: session_deadline.min(Instant::now() + CALL_TIMEOUT),
            pump: &mut none,
        };
        // Installation and ACK are separate from accepting a stdio call. An
        // early LF call already queued prevents even writing the ACK.
        active.send_private(control, &arm.ack()?, &[stdio, evidence])?;
        let call = Call::parse(lines.read(&mut active, stdio, &[control, evidence])?, false)?;
        lines.unbuffered()?;
        let request = arm.request(&bound, &call)?;
        // No reentry or second call before the reply. This arm is single-use;
        // the next arm must carry a strictly fresh correlation ID.
        active.send_packet(evidence, &request, &[], &[stdio, control])?;
        let reply = active.recv_packet(evidence, &[stdio, control])?;
        let (response, payload) = arm.reply(&bound, &call, reply)?;
        ensure!(
            wire::canonical_signed(&payload)?.len() as u64 <= MAX_REPLY,
            "fixture shim response exceeds pinned byte limit"
        );
        active.send_line(stdio, &response, &[evidence, control])?;
    }
}

fn startup_stdio(
    scope: &mut Scope<'_>,
    stdio: BorrowedFd<'_>,
    lines: &mut Lines,
    idle: &[BorrowedFd<'_>],
) -> Result<()> {
    ensure!(
        lines.read(scope, stdio, idle)? == protocol::initialize(),
        "fixture shim requires pinned MCP initialization"
    );
    lines.unbuffered()?;
    scope.send_line(stdio, &protocol::server_info(protocol::SHIM_SERVER), idle)?;
    ensure!(
        lines.read(scope, stdio, idle)? == protocol::initialized(),
        "fixture shim requires initialized notification"
    );
    ensure!(
        lines.read(scope, stdio, idle)? == protocol::list(),
        "fixture shim requires pinned MCP list request"
    );
    lines.unbuffered()?;
    scope.send_line(stdio, &protocol::listing(false)?, idle)
}
