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
