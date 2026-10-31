use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn only_matching_run_sandboxes_are_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let shim = tmp.path().join("podman");
    let log = tmp.path().join("removed");
    let marker = "a".repeat(64);
    std::fs::write(&shim, format!(r##"#!/bin/sh
if [ "$1" = ps ]; then
  case "$*" in *"label=corral.load-sandbox=true"*"label=corral.run-owner={marker}"*) echo 123456789abc;; esac
elif [ "$1" = rm ]; then
  echo "$5" >> '{}'
fi
"##, log.display())).unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    reap_with(shim.to_str().unwrap(), &marker).unwrap();
    reap_with(shim.to_str().unwrap(), &"b".repeat(64)).unwrap();
    assert_eq!(std::fs::read_to_string(log).unwrap(), "123456789abc\n");
}
