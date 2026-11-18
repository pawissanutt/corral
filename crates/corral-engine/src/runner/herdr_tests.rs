use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use corral_herdr::Herdr;

use super::tests::spec;
use super::{HerdrRunner, Origin, Runner, SessionRef, SessionStatus};
use crate::as_refusal;

const FAKE_HERDR: &str = r#"#!/bin/sh
set -eu
[ "$1" = --session ] || exit 64
mode=$2
shift 2
dir=${0%/*}
[ "$#" -ge 2 ] || exit 64
printf '%s %s\n' "$1" "$2" >> "$dir/calls"
ok() { printf '{"result":{}}\n'; }
fail() { printf '{"error":{"code":"%s","message":"fixture failure"}}\n' "$1"; exit 1; }
case "$1:$2" in
  workspace:create)
    [ "$#" -eq 7 ] && [ "$3" = --cwd ] && [ "$4" = "$dir" ] &&
      [ "$5" = --label ] && [ "$6" = 'T-042 runner test' ] && [ "$7" = --no-focus ] || exit 65
    printf '%s\n' '{"result":{"root_pane":{"workspace_id":"w1","pane_id":"w1:p1","tab_id":"w1:t1"}}}'
    ;;
  pane:report-metadata)
    [ "$#" -eq 11 ] && [ "$3" = w1:p1 ] && [ "$4" = --source ] && [ "$5" = custom:corral ] &&
      [ "$6" = --title ] && [ "$7" = 'T-042 runner test' ] && [ "$8" = --token ] &&
      [ "$9" = task=T-042 ] && [ "${10}" = --token ] && [ "${11}" = run=R7 ] || exit 65
    [ "$mode" != metadata-error ] || fail metadata_failure
    ok
    ;;
  pane:run)
    [ "$#" -eq 4 ] && [ "$3" = w1:p1 ] || exit 65
    [ "$mode" != run-error ] || fail run_failure
    (cd "$dir" && sh -c "$4")
    ok
    ;;
  pane:get)
    [ "$3" = w1:p1 ] || exit 65
    [ ! -e "$dir/closed" ] || fail pane_not_found
    status=working
    if [ -e "$dir/state" ]; then status=$(cat "$dir/state"); fi
    printf '{"result":{"pane":{"workspace_id":"w1","pane_id":"w1:p1","agent_status":"%s"}}}\n' "$status"
    ;;
  pane:read)
    [ "$3" = w1:p1 ] || exit 65
    printf '\nWhich branch should I use?\n'
    ;;
  agent:prompt)
    [ "$#" -eq 4 ] && [ "$3" = w1:p1 ] || exit 65
    [ "$4" != 'reject this prompt' ] || fail transport_error
    printf '%s' "$4" > "$dir/prompt"
    ok
    ;;
  workspace:focus) [ "$3" = w1 ] || exit 65; ok ;;
  agent:focus) [ "$3" = w1:p1 ] || exit 65; ok ;;
  pane:close)
    [ "$3" = w1:p1 ] || exit 65
    : > "$dir/closed"
    ok
    ;;
  *) exit 64 ;;
esac
"#;

fn runner(root: &Path, scenario: &str) -> HerdrRunner {
    let bin = root.join("fake-herdr");
    fs::write(&bin, FAKE_HERDR).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
    HerdrRunner::new(Herdr::with_bin(bin, Some(scenario.to_owned())))
}

#[test]
fn herdr_launches_with_quoted_exports_and_routes_session_calls() {
    let temp = crate::tests::tempdir();
    let root = temp.path();
    let runner = runner(root, "normal");
    let value = "literal 'quote'; $HOME\nsecond line";
    let mut launch = spec(root, r#"printf '%s' "$SAMPLE" > received"#);
    launch.env.push(("SAMPLE".into(), value.into()));
    let session = runner.start(&launch).unwrap();
    assert_eq!(
        session,
        SessionRef::Herdr {
            workspace: "w1".into(),
            pane: "w1:p1".into(),
        }
    );
    assert_eq!(fs::read_to_string(root.join("received")).unwrap(), value);
    assert_eq!(runner.status(&session).unwrap(), SessionStatus::Working);
    fs::write(root.join("state"), "done").unwrap();
    assert_eq!(runner.status(&session).unwrap(), SessionStatus::Idle);
    fs::write(root.join("state"), "blocked").unwrap();
    assert_eq!(
        runner.status(&session).unwrap(),
        SessionStatus::Blocked {
            excerpt: "Which branch should I use?".into(),
        }
    );

    let text = "Owner's reply;\nsecond line";
    runner.send(&session, text, Origin::Owner).unwrap();
    assert_eq!(fs::read_to_string(root.join("prompt")).unwrap(), text);
    let error = runner
        .send(&session, "reject this prompt", Origin::Auto)
        .unwrap_err();
    assert!(error.to_string().contains("transport_error"));
    assert!(as_refusal(&error).is_none());

    runner.focus(&session).unwrap();
    runner.stop(&session).unwrap();
    assert_eq!(runner.status(&session).unwrap(), SessionStatus::Gone);
    let calls = fs::read_to_string(root.join("calls")).unwrap();
    assert!(calls.starts_with("workspace create\npane report-metadata\npane run\n"));
    assert!(calls.contains("workspace focus\nagent focus\npane close\n"));
}

#[test]
fn herdr_closes_new_pane_after_metadata_or_run_failure() {
    for (scenario, code, calls) in [
        (
            "metadata-error",
            "metadata_failure",
            "workspace create\npane report-metadata\npane close\n",
        ),
        (
            "run-error",
            "run_failure",
            "workspace create\npane report-metadata\npane run\npane close\n",
        ),
    ] {
        let temp = crate::tests::tempdir();
        let root = temp.path();
        let runner = runner(root, scenario);
        let error = runner.start(&spec(root, "true")).unwrap_err();
        assert!(format!("{error:#}").contains(code));
        assert_eq!(fs::read_to_string(root.join("calls")).unwrap(), calls);
        assert!(root.join("closed").exists());
    }
}

#[test]
fn herdr_rejects_invalid_export_before_creating_a_workspace() {
    let temp = crate::tests::tempdir();
    let root = temp.path();
    let runner = runner(root, "normal");
    let mut launch = spec(root, "true");
    launch.env.push(("invalid; key".into(), "value".into()));
    assert!(runner.start(&launch).is_err());
    assert!(!root.join("calls").exists());
}
