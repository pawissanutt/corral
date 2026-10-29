use std::path::Path;

use anyhow::{Context, Result, bail};
use corral_core::{Run, StuckProcess, Task};
use corral_herdr::ProcessInfo;
use serde_json::json;

use crate::Ctx;

/// Local process identity and lifetime, independent of herdr's semantic agent state.
#[derive(Debug, Clone)]
pub struct ProcessSnapshot {
    pub parent_pid: u32,
    pub started: String,
    pub elapsed_secs: u64,
    pub argv: Vec<String>,
}

/// Injectable boundary for process inspection and single-process termination.
pub trait ProcessControl {
    fn inspect(&self, pid: u32) -> Result<Option<ProcessSnapshot>>;
    /// Terminate only this kernel identity, with bounded waits.
    /// Unix uses SIGTERM then SIGKILL; Windows uses its retained process handle.
    fn terminate(&self, process: &StuckProcess) -> Result<()>;
}

pub struct SystemProcesses;

#[cfg(target_os = "linux")]
impl ProcessControl for SystemProcesses {
    fn inspect(&self, pid: u32) -> Result<Option<ProcessSnapshot>> {
        let path = format!("/proc/{pid}");
        let stat = match std::fs::read_to_string(format!("{path}/stat")) {
            Ok(stat) => stat,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err).with_context(|| format!("reading {path}/stat")),
        };
        // comm (field 2) may contain spaces and parentheses; fields after its final ')' start at 3.
        let fields = stat
            .rsplit_once(')')
            .context("process stat missing comm")?
            .1;
        let parent_pid = fields
            .split_whitespace()
            .nth(1)
            .context("process stat missing parent PID")?
            .parse()
            .context("parsing process parent PID")?;
        let started = fields
            .split_whitespace()
            .nth(19)
            .context("process stat missing starttime")?
            .to_string();
        let ticks: u64 = started.parse().context("parsing process start ticks")?;
        let uptime = std::fs::read_to_string("/proc/uptime").context("reading process uptime")?;
        let secs: u64 = uptime
            .split('.')
            .next()
            .context("missing uptime")?
            .parse()
            .context("parsing uptime seconds")?;
        let cmdline = std::fs::read(format!("{path}/cmdline"))
            .with_context(|| format!("reading {path}/cmdline"))?;
        let argv = cmdline
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        Ok(Some(ProcessSnapshot {
            parent_pid,
            started,
            elapsed_secs: secs.saturating_sub(ticks / rustix::param::clock_ticks_per_second()),
            argv,
        }))
    }

    fn terminate(&self, process: &StuckProcess) -> Result<()> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

        let pid = Pid::from_raw(i32::try_from(process.pid).context("PID out of range")?)
            .context("refusing to signal PID zero")?;
        // Pin the kernel identity before rechecking; neither signal can hit a recycled PID.
        let fd = pidfd_open(pid, PidfdFlags::empty()).context("opening stuck process pidfd")?;
        let current = self.inspect(process.pid)?.context("stuck process exited")?;
        if current.started != process.started || current.argv != process.argv {
            bail!("stuck process identity changed; sync and confirm again");
        }
        pidfd_send_signal(&fd, Signal::TERM).context("sending SIGTERM to stuck process")?;
        let mut fds = [PollFd::new(&fd, PollFlags::IN)];
        let timeout = Timespec {
            tv_sec: 3,
            tv_nsec: 0,
        };
        if poll(&mut fds, Some(&timeout)).context("waiting for stuck process exit")? == 0 {
            match pidfd_send_signal(&fd, Signal::KILL) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => {}
                Err(err) => return Err(err).context("sending SIGKILL to stuck process"),
            }
            poll(&mut fds, Some(&timeout)).context("waiting after SIGKILL")?;
            if fds[0].revents().is_empty() {
                bail!("stuck process has not exited after SIGKILL");
            }
        }
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
impl ProcessControl for SystemProcesses {
    fn inspect(&self, pid: u32) -> Result<Option<ProcessSnapshot>> {
        crate::native_process::inspect(pid)
    }

    fn terminate(&self, process: &StuckProcess) -> Result<()> {
        crate::native_process::terminate(process)
    }
}

fn basename(value: &str) -> &str {
    let name = Path::new(value)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(value);
    #[cfg(windows)]
    if name.len() >= 4
        && name
            .get(name.len() - 4..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".exe"))
    {
        return &name[..name.len() - 4];
    }
    name
}

pub(crate) fn protected(ctx: &Ctx, run: &Run, argv: &[String]) -> bool {
    let Some(exe) = argv.first() else { return true };
    let name = basename(exe);
    #[cfg(windows)]
    let normalized = name.to_ascii_lowercase();
    #[cfg(windows)]
    let name = normalized.as_str();
    let harness = run.harness.as_deref().unwrap_or(&run.agent);
    if name == harness
        || (cfg!(windows) && name.eq_ignore_ascii_case(harness))
        || ["omo", "omp", "claude", "codex", "pi", "senpi"]
            .iter()
            .any(|h| name == *h || (cfg!(windows) && name.eq_ignore_ascii_case(h)))
    {
        return true;
    }
    let command = ctx
        .store
        .config
        .harnesses
        .get(harness)
        .map(|h| h.command.as_str())
        .or_else(|| {
            ctx.store
                .config
                .agents
                .get(harness)
                .map(|a| a.command.as_str())
        });
    // Runtime executables alone are NOT exempt: bun/node/python can run a wedged user command.
    let interpreter = matches!(
        name,
        "bun" | "bun.exe" | "node" | "node.exe" | "python" | "python3"
    );
    if command
        .and_then(|c| c.split_whitespace().next())
        .is_some_and(|c| {
            basename(c) == name || (cfg!(windows) && basename(c).eq_ignore_ascii_case(name))
        })
    {
        if !interpreter {
            return true;
        }
        if let Some(script) = command.and_then(|c| c.split_whitespace().nth(1))
            && argv.get(1).is_some_and(|arg| arg == script)
        {
            return true;
        }
    }
    interpreter
        && argv
            .iter()
            .skip(1)
            .find(|arg| !arg.starts_with('-'))
            .is_some_and(|script| {
                #[cfg(windows)]
                let script = script.replace('\\', "/").to_ascii_lowercase();
                script.contains("/@code-yeongyu/senpi/")
                    || script.ends_with("/@oh-my-pi/pi-coding-agent/dist/cli.js")
                    || script.ends_with("/@mariozechner/pi-coding-agent/dist/cli.js")
                    || script.contains("/@code-yeongyu/senpi-codemode/src/kernels/")
                    || script.ends_with("/omo-ai/plugin/runtime/ast-grep-mcp/cli.js")
                    || script.contains("/omo-ai/plugin/runtime/lsp-daemon/dist/")
            })
}

pub(crate) fn foreign_process(
    ctx: &Ctx,
    run: &Run,
    info: &ProcessInfo,
    processes: &dyn ProcessControl,
) -> Result<Option<StuckProcess>> {
    for process in &info.foreground_processes {
        if process.pid <= 1 || process.pid == info.shell_pid || protected(ctx, run, &process.argv) {
            continue;
        }
        if let Some(local) = processes.inspect(process.pid)?
            && local.argv == process.argv
            // The pane shell's direct child is the launch process even when its executable
            // differs from the configured harness name (shebangs, symlinks, wrappers).
            && !launched(&local, info, processes)?
            && local.elapsed_secs > ctx.store.config.sync.stuck_after_secs
        {
            return Ok(Some(StuckProcess {
                pid: process.pid,
                argv: process.argv.clone(),
                started: local.started,
            }));
        }
    }
    Ok(None)
}

/// corral's launch shells: the wrapper and the template's own shell
/// (`/bin/sh -c <script> corral-launch ...`).
fn launch_shell(argv: &[String]) -> bool {
    argv.get(1).is_some_and(|a| a == "-c") && argv.get(3).is_some_and(|a| a == "corral-launch")
}

/// The launched agent itself: a direct child of the pane shell, or a launch shell or a direct
/// child of one, descending from the pane shell. The pane shell may first start a nested
/// interactive shell (bash running fish), so launch shells need not be its direct children.
fn launched(
    process: &ProcessSnapshot,
    info: &ProcessInfo,
    processes: &dyn ProcessControl,
) -> Result<bool> {
    let mut wrapped = launch_shell(&process.argv);
    let mut pid = process.parent_pid;
    // Two launch shells and a few nested shells; deeper ancestry is not a launch.
    for _ in 0..6 {
        if pid == info.shell_pid {
            return Ok(true);
        }
        let Some(parent) = processes.inspect(pid)? else {
            return Ok(false);
        };
        if launch_shell(&parent.argv) {
            wrapped = true;
        } else if !wrapped {
            return Ok(false);
        }
        pid = parent.parent_pid;
    }
    Ok(false)
}

/// Called only after the UI/CLI has confirmed the displayed process identity.
pub fn unstick(ctx: &Ctx, id: &str, expected: &StuckProcess) -> Result<Task> {
    unstick_with(ctx, id, expected, &SystemProcesses)
}

pub fn unstick_with(
    ctx: &Ctx,
    id: &str,
    expected: &StuckProcess,
    processes: &dyn ProcessControl,
) -> Result<Task> {
    let _attribution = ctx.store.attribute(corral_core::Actor::System);
    let task = ctx.store.get(id)?;
    let run = task
        .current_run()
        .filter(|r| r.outcome.is_none())
        .context("task has no live run")?;
    if !task.status.holds_slot() || run.stuck.as_ref() != Some(expected) {
        bail!("stuck run changed; sync and confirm again");
    }
    let pane = &run.herdr.as_ref().context("run has no pane")?.pane_id;
    let info = ctx.herdr.pane_process_info(pane)?;
    if expected.pid == info.shell_pid
        || expected.pid <= 1
        || protected(ctx, run, &expected.argv)
        || !info
            .foreground_processes
            .iter()
            .any(|p| p.pid == expected.pid && p.argv == expected.argv)
    {
        bail!("confirmed process is no longer a foreign foreground process; refusing to kill");
    }
    let current = processes
        .inspect(expected.pid)?
        .context("stuck process exited")?;
    if launched(&current, &info, processes)?
        || current.started != expected.started
        || current.argv != expected.argv
    {
        bail!("stuck process identity changed; sync and confirm again");
    }
    processes.terminate(expected)?;
    let recovered = ctx.store.update(&task.id, |t| {
        let r = t
            .current_run_mut()
            .context("run disappeared during recovery")?;
        if r.id != run.id || r.outcome.is_some() || r.stuck.as_ref() != Some(expected) {
            bail!("run changed during recovery; process terminated but state left unchanged");
        }
        r.stuck = None;
        r.herdr_idle_since = None;
        r.idle_since = None;
        r.idle_no_report = false;
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "unstuck",
        json!({"pid": expected.pid, "argv": expected.argv}),
    )?;
    if let Some(text) = recovered
        .current_run()
        .and_then(|r| r.pending_reply.as_deref())
    {
        return crate::reply(ctx, id, text);
    }
    Ok(recovered)
}

#[cfg(test)]
#[path = "watchdog_tests.rs"]
mod tests;
