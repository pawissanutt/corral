//! Bounded hook lifetime, including descendants after the direct command exits.
use std::process::{Command, ExitStatus};
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;

use anyhow::{Context, Result, bail};

#[cfg(unix)]
use rustix::process::{Pid, Signal, kill_process_group};

pub(crate) struct HookChild {
    #[cfg(unix)]
    child: std::process::Child,
    #[cfg(windows)]
    child: Box<dyn process_wrap::std::ChildWrapper>,
    armed: bool,
}

impl HookChild {
    pub(crate) fn spawn(command: Command) -> Result<Self> {
        #[cfg(unix)]
        let child = {
            use std::os::unix::process::CommandExt;
            let mut command = command;
            command
                .process_group(0)
                .spawn()
                .context("spawning hook process group")?
        };
        #[cfg(windows)]
        let child = {
            use process_wrap::std::{CommandWrap, JobObject};
            CommandWrap::from(command)
                .wrap(JobObject)
                .spawn()
                .context("spawning suspended hook in kill-on-close Job Object")?
        };
        Ok(Self { child, armed: true })
    }

    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }

    fn stop(&mut self) -> Result<()> {
        #[cfg(unix)]
        {
            let pid = Pid::from_raw(i32::try_from(self.id())?).context("invalid hook PID")?;
            match kill_process_group(pid, Signal::KILL) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => {}
                Err(error) => return Err(error).context("stopping hook process group"),
            }
        }
        #[cfg(windows)]
        self.child
            .start_kill()
            .context("terminating hook Job Object")?;
        Ok(())
    }

    pub(crate) fn wait(mut self, timeout: u32) -> Result<(ExitStatus, bool)> {
        let timeout = Duration::from_secs(u64::from(timeout));
        #[cfg(unix)]
        {
            // Observe exit without reaping: retaining the group leader prevents PGID reuse
            // before the final group signal, even for an immediately exiting command.
            let event = ExitEvent::new(self.id())?;
            let timed_out = !event.wait(timeout)?;
            self.stop()?;
            if timed_out && !event.wait(Duration::from_secs(3))? {
                bail!("hook process has not exited after SIGKILL");
            }
            let status = self.child.wait().context("reaping hook process")?;
            self.armed = false;
            Ok((status, timed_out))
        }
        #[cfg(windows)]
        {
            use winsafe::{HPROCESS, co};
            let handle = HPROCESS::OpenProcess(co::PROCESS::SYNCHRONIZE, false, self.id())
                .context("retaining hook process exit handle")?;
            let exited = handle
                .WaitForSingleObject(Some(u32::try_from(timeout.as_millis())?))
                .context("waiting for hook process exit")?;
            let timed_out = match exited {
                co::WAIT::OBJECT_0 => false,
                co::WAIT::TIMEOUT => true,
                other => bail!("unexpected native hook wait result: {other:?}"),
            };
            self.stop()?;
            // The wrapper waits for JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO, not just the
            // direct child. Bound that wait without dropping its containment handle early.
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            self.armed = false;
            std::thread::spawn(move || {
                let mut owner = self;
                let result = owner.child.wait();
                if tx.send(result).is_err() {
                    eprintln!("hook job completion arrived after its exit deadline");
                }
            });
            let status = rx
                .recv_timeout(Duration::from_secs(3))
                .context("hook job has not drained after native termination")?
                .context("waiting for hook Job Object completion")?;
            Ok((status, timed_out))
        }
    }
}

impl Drop for HookChild {
    fn drop(&mut self) {
        if self.armed
            && let Err(error) = self.stop()
        {
            eprintln!("hook process cleanup failed: {error:#}");
        }
    }
}

#[cfg(target_os = "linux")]
struct ExitEvent(rustix::fd::OwnedFd);

#[cfg(target_os = "linux")]
impl ExitEvent {
    fn new(pid: u32) -> Result<Self> {
        use rustix::process::{PidfdFlags, pidfd_open};
        let pid = Pid::from_raw(i32::try_from(pid)?).context("invalid hook PID")?;
        Ok(Self(
            pidfd_open(pid, PidfdFlags::empty()).context("opening hook pidfd")?,
        ))
    }

    fn wait(&self, timeout: Duration) -> Result<bool> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let timeout = Timespec {
                tv_sec: i64::try_from(remaining.as_secs())?,
                tv_nsec: remaining.subsec_nanos().into(),
            };
            match poll(&mut [PollFd::new(&self.0, PollFlags::IN)], Some(&timeout)) {
                Ok(count) => return Ok(count != 0),
                Err(rustix::io::Errno::INTR) => {}
                Err(error) => return Err(error).context("waiting for hook exit"),
            }
        }
    }
}

#[cfg(target_os = "macos")]
struct ExitEvent(Option<kqueue::Watcher>);

#[cfg(target_os = "macos")]
impl ExitEvent {
    fn new(pid: u32) -> Result<Self> {
        match crate::native_process::platform::exit_watch(pid) {
            Ok(watcher) => Ok(Self(Some(watcher))),
            // An unreaped child cannot have its PID/PGID reused. Darwin may
            // refuse NOTE_EXIT registration for an already exited child.
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.raw_os_error() == Some(libc::ESRCH)) =>
            {
                Ok(Self(None))
            }
            Err(error) => Err(error),
        }
    }

    fn wait(&self, timeout: Duration) -> Result<bool> {
        match &self.0 {
            Some(watcher) => crate::native_process::platform::wait_exit(watcher, timeout),
            None => Ok(true),
        }
    }
}
