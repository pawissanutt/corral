//! Read committed canonical state in a separate test process at the fake pane boundary.
use corral_core::{
    Store,
    agent::{ReceiptState, pm_state},
    inbox::{Inbox, Recipient},
};
use std::path::Path;

pub(crate) fn pane_check(root: &Path, mode: &str, role: &str) -> String {
    let quote = |value: &str| format!("'{}'", crate::prompt::sh_escape(value));
    let root = quote(&root.display().to_string());
    let exe = quote(&std::env::current_exe().unwrap().display().to_string());
    let mode = quote(mode);
    let role = quote(role);
    format!(
        r#"if [ "$1 $2" = "pane run" ]; then
rm -f {root}/before-pane-guard.ok
CORRAL_FIXTURE_GUARD_ROOT={root} CORRAL_FIXTURE_GUARD_MODE={mode} CORRAL_FIXTURE_GUARD_ROLE={role} {exe} --exact agent::fixture_guard_tests::durable_before_pane_run --ignored --nocapture >/dev/null || exit 1
test -f {root}/before-pane-guard.ok || exit 1
fi
case "$1 $2" in"#
    )
}

#[test]
#[ignore = "entrypoint invoked by private metered/PM fake engines before pane_run returns"]
fn durable_before_pane_run() {
    let Some(root) = std::env::var_os("CORRAL_FIXTURE_GUARD_ROOT") else {
        println!("fixture guard entrypoint needs its private root");
        return;
    };
    let root = std::path::PathBuf::from(root);
    let store = Store::open(&root).unwrap();
    match std::env::var("CORRAL_FIXTURE_GUARD_MODE").unwrap().as_str() {
        "metered" => {
            let role = std::env::var("CORRAL_FIXTURE_GUARD_ROLE").unwrap();
            let notices: Vec<_> = store
                .events()
                .unwrap()
                .into_iter()
                .filter(|event| {
                    event.kind == "metered_notice"
                        && event.detail["role"].as_str() == Some(role.as_str())
                })
                .collect();
            assert_eq!(notices.len(), 1, "{notices:?}");
        }
        "pm-recovery" => {
            let pm = pm_state::load(&store).unwrap().unwrap();
            assert_eq!(pm.state.binding.generation, 2);
            assert!(pm.state.generations[0].fenced.is_some());
            assert!(
                pm.state
                    .receipts
                    .iter()
                    .all(|receipt| receipt.state != ReceiptState::Reserved)
            );
            assert!(
                Inbox::new(&store, Recipient::Pm)
                    .unwrap()
                    .messages()
                    .unwrap()
                    .iter()
                    .any(|message| message.kind == "recovery_notice")
            );
        }
        mode => panic!("unknown fixture guard mode {mode}"),
    }
    // Parent requires this marker, so a skipped guard or a failed child cannot pass silently.
    std::fs::write(
        root.join("before-pane-guard.ok"),
        "validated canonical committed state\n",
    )
    .unwrap();
}
