//! Work counters around one board path: store work, subprocesses and (Linux) thread I/O.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use corral_core::work::Work;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Sample {
    pub(crate) work: [u64; Work::ALL.len()],
    pub(crate) herdr: u64,
    pub(crate) git: u64,
    /// Informational: bytes and read syscalls of this thread, from /proc/thread-self/io.
    pub(crate) rchar: u64,
    pub(crate) syscr: u64,
}

impl Sample {
    pub(crate) fn now() -> Sample {
        let (rchar, syscr) = thread_io();
        Sample {
            work: Work::ALL.map(corral_core::work::count),
            herdr: corral_herdr::work::invocations(),
            git: corral_git::work::invocations(),
            rchar,
            syscr,
        }
    }

    pub(crate) fn since(self, before: Sample) -> Sample {
        let mut work = [0; Work::ALL.len()];
        for (i, slot) in work.iter_mut().enumerate() {
            *slot = self.work[i] - before.work[i];
        }
        Sample {
            work,
            herdr: self.herdr - before.herdr,
            git: self.git - before.git,
            rchar: self.rchar.saturating_sub(before.rchar),
            syscr: self.syscr.saturating_sub(before.syscr),
        }
    }

    pub(crate) fn get(&self, work: Work) -> u64 {
        self.work[work as usize]
    }

    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        for work in Work::ALL {
            let _ = write!(out, "{}={} ", work.name(), self.get(work));
        }
        let _ = write!(
            out,
            "herdr_calls={} git_calls={} io_read_bytes={} io_read_syscalls={}",
            self.herdr, self.git, self.rchar, self.syscr
        );
        out
    }
}

pub(crate) fn measure(action: impl FnOnce()) -> (Sample, Duration) {
    let before = Sample::now();
    let start = Instant::now();
    action();
    let elapsed = start.elapsed();
    (Sample::now().since(before), elapsed)
}

#[cfg(target_os = "linux")]
fn thread_io() -> (u64, u64) {
    let Ok(text) = std::fs::read_to_string("/proc/thread-self/io") else {
        return (0, 0);
    };
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0)
    };
    (field("rchar:"), field("syscr:"))
}

#[cfg(not(target_os = "linux"))]
fn thread_io() -> (u64, u64) {
    (0, 0)
}
