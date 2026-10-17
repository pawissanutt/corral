use std::io::{self, ErrorKind, Read};

use super::retry_interrupted;

#[test]
fn interrupted_reads_retry_until_eof_without_losing_bytes() {
    // Given the actual read boundary alternating EINTR, data, EINTR and EOF.
    let mut responses = [
        Err(ErrorKind::Interrupted.into()),
        Ok(3),
        Err(ErrorKind::Interrupted.into()),
        Ok(0),
    ]
    .into_iter();
    let mut source = &b"abc"[..];
    let mut buffer = [0; 3];
    let mut actual = Vec::new();
    // When the hook runner's retry boundary drains the reader.
    loop {
        let size = retry_interrupted(|| {
            let result: io::Result<usize> = responses.next().unwrap();
            result.and_then(|size| source.read(&mut buffer[..size]))
        })
        .unwrap();
        if size == 0 {
            break;
        }
        actual.extend_from_slice(&buffer[..size]);
    }
    // Then neither an interruption nor EOF drops or duplicates captured bytes.
    assert_eq!(actual, b"abc");
    assert!(responses.next().is_none());
}

#[test]
fn interrupted_wait_retries_without_swallowing_terminal_errors() {
    // Given a wait interrupted twice, followed by a non-retryable error.
    let mut responses = [
        Err(ErrorKind::Interrupted.into()),
        Err(ErrorKind::Interrupted.into()),
        Err(ErrorKind::PermissionDenied.into()),
    ]
    .into_iter();
    // When the same retry boundary used for reaping a hook child waits.
    let result: io::Result<()> = retry_interrupted(|| responses.next().unwrap());
    // Then only EINTR is retried; the actual failure is propagated.
    assert_eq!(result.unwrap_err().kind(), ErrorKind::PermissionDenied);
    assert!(responses.next().is_none());
}

#[cfg(unix)]
mod readonly {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use crate::{Ctx, HookState, PostLandOperation};

    const PRIOR: &[u8] = b"#!/bin/sh\nprintf 'prior\\n'\n";

    fn fixture() -> (tempfile::TempDir, Ctx, PostLandOperation) {
        let root = tempfile::tempdir().unwrap();
        let ctx = Ctx::from_store(corral_core::Store::init(root.path()).unwrap());
        let binary = root.path().join("tool");
        let backup = root.path().join("prior");
        fs::write(&binary, PRIOR).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o555)).unwrap();
        fs::copy(&binary, &backup).unwrap();
        let op = serde_json::from_value(serde_json::json!({
            "version":1, "key":"readonly", "project":"app", "repo":root.path(),
            "task":"T-001", "run":"R1", "base":"main", "base_before":"before",
            "landed_sha":"after", "policy_digest":"readonly",
            "created":"2026-10-17T00:00:00Z", "landed":true,
            "abandoned":false, "notified":false, "hooks":[{
                "hook":{
                    "name":"install", "argv":["/bin/sh", "-c", "exit 0"], "cwd":".", "env":{},
                    "timeout_secs":30, "max_attempts":2,
                    "identity":{"binary":"tool", "args":[], "expected_stdout":"prior"}
                },
                "state":"failed", "reason":"prior attempt failed", "attempts":[{
                    "number":1, "started":"2026-10-17T00:00:00Z",
                    "finished":"2026-10-17T00:00:00Z",
                    "token":root.path().to_string_lossy(), "pid":null,
                    "process_start":null, "cwd":root.path(), "env":{},
                    "log":root.path().join("prior.log"), "exit_code":1,
                    "signal":null, "timed_out":false, "error":null,
                    "backup":backup, "installed_sha256":null, "identity_stdout":null
                }]
            }]
        }))
        .unwrap();
        (root, ctx, op)
    }

    #[test]
    fn native_readonly_executable_is_backed_up_before_hook_execution() {
        // Given a readable, executable mode-0555 installation in a writable directory.
        let (root, ctx, mut op) = fixture();
        // When the real hook backs it up, executes, and probes the installed identity.
        super::super::execute(&ctx, &mut op, 0).unwrap();
        // Then the hook succeeds without changing installed or backup permissions.
        assert_eq!(op.hooks[0].state, HookState::Succeeded);
        let backup = op.hooks[0]
            .attempts
            .last()
            .unwrap()
            .backup
            .as_ref()
            .unwrap();
        assert_eq!(fs::read(backup).unwrap(), PRIOR);
        assert_eq!(
            fs::metadata(backup).unwrap().permissions().mode() & 0o777,
            0o555
        );
        assert_eq!(
            fs::metadata(root.path().join("tool"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o555
        );
    }

    #[test]
    fn native_readonly_backup_restores_a_replaced_executable() {
        // Given a mode-0555 backup and a failed installer that replaced the old inode.
        let (root, _ctx, op) = fixture();
        let binary = root.path().join("tool");
        let replacement = root.path().join("replacement");
        fs::write(&replacement, b"broken executable").unwrap();
        fs::rename(&replacement, &binary).unwrap();
        // When restoration atomically replaces the failed installation from its backup.
        super::super::restore(&op, 0).unwrap();
        // Then the original executable bytes and mode are restored without chmod.
        assert_eq!(fs::read(&binary).unwrap(), PRIOR);
        assert_eq!(
            fs::metadata(&binary).unwrap().permissions().mode() & 0o777,
            0o555
        );
    }
}
