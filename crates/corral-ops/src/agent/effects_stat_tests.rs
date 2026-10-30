//! `effects::process_start` treats an executor that exits while its `/proc/<pid>/stat` is read
//! as gone (ESRCH), not as an error that aborts the caller.
#![cfg(target_os = "linux")]

use corral_core::agent::Executor;

use rustix::io::Errno;

use super::effects::{self, STAT_FAULT};

fn fail_next_read(errno: Errno) {
    STAT_FAULT.with(|fault| fault.set(Some(errno.raw_os_error())));
}

#[test]
fn esrch_during_the_stat_read_is_a_gone_process() {
    let pid = std::process::id();
    let started = effects::process_start(pid).unwrap().unwrap();

    fail_next_read(Errno::SRCH);
    assert_eq!(effects::process_start(pid).unwrap(), None);

    fail_next_read(Errno::SRCH);
    let executor = Executor {
        pid,
        start_time: started,
    };
    assert!(effects::executor_gone(&executor).unwrap());
}

#[test]
fn other_stat_read_errors_still_propagate() {
    fail_next_read(Errno::ACCESS);
    assert!(effects::process_start(std::process::id()).is_err());
}
