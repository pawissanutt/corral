use anyhow::{Context, Result};
use corral_core::StuckProcess;
use winsafe as win;

pub(super) fn identity(pid: u32) -> Result<Option<String>> {
    let handle = match win::HPROCESS::OpenProcess(
        win::co::PROCESS::QUERY_LIMITED_INFORMATION | win::co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    ) {
        Ok(handle) => handle,
        Err(win::co::ERROR::INVALID_PARAMETER) => return Ok(None),
        Err(error) => return Err(error).context("opening process for identity"),
    };
    if handle
        .WaitForSingleObject(Some(0))
        .context("checking process exit")?
        == win::co::WAIT::OBJECT_0
    {
        return Ok(None);
    }
    let (created, _, _, _) = handle
        .GetProcessTimes()
        .context("reading process creation time")?;
    Ok(Some(u64::from(created).to_string()))
}

pub(crate) fn terminate(process: &StuckProcess) -> Result<()> {
    if process.pid <= 1 {
        anyhow::bail!("refusing to terminate a system PID");
    }
    let handle = win::HPROCESS::OpenProcess(
        win::co::PROCESS::QUERY_LIMITED_INFORMATION
            | win::co::PROCESS::SYNCHRONIZE
            | win::co::PROCESS::TERMINATE,
        false,
        process.pid,
    )
    .context("opening stuck process handle")?;
    let (created, _, _, _) = handle
        .GetProcessTimes()
        .context("reading retained process identity")?;
    let current = super::inspect(process.pid)?.context("stuck process exited")?;
    if u64::from(created).to_string() != process.started
        || current.started != process.started
        || current.argv != process.argv
    {
        anyhow::bail!("stuck process identity changed; sync and confirm again");
    }
    // Windows has no general SIGTERM equivalent. TerminateProcess acts on this retained
    // kernel handle only, including when the numeric PID is no longer in the inventory.
    handle
        .TerminateProcess(1)
        .context("terminating stuck process")?;
    if handle
        .WaitForSingleObject(Some(3000))
        .context("waiting for stuck process exit")?
        != win::co::WAIT::OBJECT_0
    {
        anyhow::bail!("stuck process has not exited after native termination");
    }
    Ok(())
}
