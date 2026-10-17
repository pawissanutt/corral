//! Small native helper for shell-runner identity and cross-process monotonic time.
use anyhow::{Context, Result};

/// Used by generated gate scripts, before any workspace or engine discovery.
#[doc(hidden)]
pub fn runtime_probe(pid: Option<u32>) -> Result<String> {
    if let Some(pid) = pid {
        use crate::ProcessControl;
        return crate::SystemProcesses
            .inspect(pid)?
            .map(|process| process.started)
            .context("gate shell exited before identity capture");
    }
    #[cfg(windows)]
    let millis = winsafe::GetTickCount64();
    #[cfg(unix)]
    let millis = {
        let time = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
        u64::try_from(time.tv_sec)?
            .checked_mul(1000)
            .and_then(|ms| ms.checked_add(u64::try_from(time.tv_nsec).ok()? / 1_000_000))
            .context("monotonic clock overflow")?
    };
    Ok(millis.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_gate_clock_is_monotonic_and_identity_is_live() {
        // Given the current native process.
        let pid = std::process::id();
        let before: u64 = super::runtime_probe(None).unwrap().parse().unwrap();
        // When the gate helper captures its identity between monotonic readings.
        let identity = super::runtime_probe(Some(pid)).unwrap();
        let after: u64 = super::runtime_probe(None).unwrap().parse().unwrap();
        // Then sidecar identity is stable and durations cannot move backwards.
        assert_eq!(identity, super::runtime_probe(Some(pid)).unwrap());
        assert!(after >= before);
    }
}
