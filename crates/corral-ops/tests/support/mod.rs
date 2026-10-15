use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

pub fn command(program: impl AsRef<OsStr>, root: &Path) -> Command {
    let mut command = Command::new(program);
    command.env("CORRAL_ROOT", root);
    for name in [
        "CORRAL_GATE_ATTEMPT",
        "CORRAL_CANDIDATE_HEAD",
        "CORRAL_INTEGRATION_BASE",
        "HERDR_PANE_ID",
        "HERDR_WORKSPACE_ID",
        "HERDR_TAB_ID",
    ] {
        command.env_remove(name);
    }
    command
}
