use std::path::Path;

pub fn herdr(root: &Path) -> corral_herdr::Herdr {
    let bin = root.join("live-herdr");
    std::os::unix::fs::symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/live-herdr.sh"),
        &bin,
    )
    .unwrap();
    corral_herdr::Herdr::with_bin(bin, Some("corral-test".into()))
}
