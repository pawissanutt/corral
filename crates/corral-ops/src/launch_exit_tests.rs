use super::*;

#[test]
fn missing_receipt_never_overwrites_real_exit_status() {
    let tmp = tempfile::tempdir().unwrap();
    let receipt = tmp.path().join("attempt.exited");
    std::fs::write(&receipt, "17\n").unwrap();
    record_missing(&receipt).unwrap();
    assert_eq!(std::fs::read_to_string(receipt).unwrap(), "17\n");
}

#[test]
#[cfg(unix)]
fn launch_exports_stable_attempt_marker_only_to_child() {
    let tmp = tempfile::tempdir().unwrap();
    let receipt = tmp.path().join("R1.exited");
    let output = tmp.path().join("marker");
    let command = format!(
        "printf '%s' \"$CORRAL_PROCESS_OWNER\" > '{}'",
        output.display()
    );
    let launch = arm(&command, &receipt).unwrap();
    let status = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(launch)
        .stdin(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let marker = std::fs::read_to_string(output).unwrap();
    assert_eq!(marker, ownership_marker(&receipt).unwrap());
    assert_eq!(marker.len(), 64);
    let other = ownership_marker(&tmp.path().join("R2.exited")).unwrap();
    assert_ne!(marker, other);
}
