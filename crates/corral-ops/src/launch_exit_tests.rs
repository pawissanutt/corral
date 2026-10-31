use super::*;
use corral_core::Store;

fn fixture() -> (tempfile::TempDir, Ctx) {
    let root = tempfile::tempdir().unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(root.path()).unwrap());
    (root, ctx)
}

#[test]
fn shell_receipts_are_scratch_notifications_that_survive_reopen() {
    let (root, ctx) = fixture();
    let path = receipt(&ctx, "T-001", "R1").unwrap();
    assert_eq!(
        path,
        ctx.store
            .scratch_path("tmp")
            .unwrap()
            .join("launch-exits/T-001-R1.exited")
    );
    let command = arm(&ctx, "exit 23", &path).unwrap();
    let status = std::process::Command::new("/bin/sh")
        .args(["-c", &command])
        .status()
        .unwrap();
    assert!(status.success());
    let mut reopened = ctx.clone();
    reopened.store = Store::open(root.path()).unwrap();
    assert!(exited_for(&reopened, "T-001", "R1"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "23\n");
    assert!(
        !ctx.store
            .dir()
            .join("state")
            .join("launch-exits/T-001-R1.exited")
            .exists()
    );
}

#[test]
fn process_marker_lookup_is_one_record_despite_historical_launches() {
    use corral_core::work::{self, Counter, HotScope};
    let mut work = Vec::new();
    for scale in [1, 10] {
        let (_root, ctx) = fixture();
        let path = receipt(&ctx, "T-001", "R1").unwrap();
        arm(&ctx, "true", &path).unwrap();
        let marker = ownership_marker(&path).unwrap();
        for n in 2..=scale {
            arm(
                &ctx,
                "true",
                &receipt(&ctx, "T-001", &format!("R{n}")).unwrap(),
            )
            .unwrap();
        }
        let _hot = HotScope::enter();
        let before = work::snapshot();
        assert_eq!(
            process_receipt(&ctx, marker.as_bytes()).unwrap(),
            Some(path)
        );
        let after = work::snapshot();
        work.push((
            after.total(Counter::RecordDecodes) - before.total(Counter::RecordDecodes),
            after.total(Counter::DecodedDocBytes) - before.total(Counter::DecodedDocBytes),
            after.full_task_hydrations() - before.full_task_hydrations(),
        ));
    }
    assert_eq!(work[0], work[1]);
    assert_eq!(work[0].0, 1);
    assert_eq!(work[0].2, 0);
}

#[test]
fn process_marker_lookup_refuses_unknown_invalid_and_escaping_receipts() {
    let (_root, ctx) = fixture();
    assert!(process_receipt(&ctx, b"not-a-marker").unwrap().is_none());
    let marker = "a".repeat(64);
    assert!(process_receipt(&ctx, marker.as_bytes()).unwrap().is_none());
    PROCESS_OWNERS
        .save(&ctx.store, &marker, &"../escape.exited".to_owned())
        .unwrap();
    assert!(process_receipt(&ctx, marker.as_bytes()).is_err());
}

#[test]
fn rearming_clears_the_previous_scratch_receipt_only() {
    let (_root, ctx) = fixture();
    let path = receipt(&ctx, "T-001", "R1").unwrap();
    arm(&ctx, "exit 0", &path).unwrap();
    std::fs::write(&path, "0\n").unwrap();
    let other = receipt(&ctx, "T-001", "R2").unwrap();
    std::fs::write(&other, "1\n").unwrap();
    arm(&ctx, "exit 1", &path).unwrap();
    assert!(!exited(&path));
    assert!(exited(&other));
    let display = "cat '/prompt.md'";
    assert_eq!(arm(&ctx, display, &other).unwrap(), display);
    assert!(exited(&other), "display never arms or clears a launch");
}

#[test]
fn legacy_receipts_are_not_read_or_mirrored() {
    let (_root, ctx) = fixture();
    let legacy = ctx
        .store
        .dir()
        .join("state")
        .join("launch-exits/T-001-R1.exited");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, "1\n").unwrap();
    let path = receipt(&ctx, "T-001", "R1").unwrap();
    assert!(!exited(&path));
    arm(&ctx, "exit 0", &path).unwrap();
    assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "1\n");
}

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
    let (_root, ctx) = fixture();
    let launch = arm(&ctx, &command, &receipt).unwrap();
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
