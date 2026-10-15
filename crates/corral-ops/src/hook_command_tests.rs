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
