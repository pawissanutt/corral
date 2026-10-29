use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::sync::mpsc;

use corral_core::{ExecutionSnapshot, OperatingActionKind, Status};
use serde_json::{Value, json};

use crate::agent::bridge::fixture::{Fx, HostProc};
use crate::agent::hooks::metered_tests::{
    MODEL, assert_notice_before_pane_run, assert_one_notice, fixture, isolated, write,
};
use crate::agent::launch::{Secrets, check, consume_bootstrap, host_command, write_ctl_cap};

#[test]
fn worker_dispatch_emits_one_metered_notice_despite_repeated_readiness() {
    isolated("dispatch::metered_tests::worker_dispatch_child");
}

#[test]
#[ignore = "driven with private HOME/XDG by the parent test"]
fn worker_dispatch_child() {
    let (mut fx, resolved) = fixture();
    fx.ctx.store.config.dispatch.paused = false;
    fx.ctx.store.config.defaults.profile = Some("bedrock".into());
    fx.ctx.store.save_config().unwrap();
    let task = fx
        .ctx
        .store
        .create("Metered worker", Some("corral"), Status::Ready, "Fixture")
        .unwrap();
    let (send, receive) = mpsc::channel();
    let _capture = corral_core::messages::capture_messages(send);
    // Given repeated route readiness, which must have no presentation side effects.
    for _ in 0..2 {
        crate::agent::hooks::preflight::check(
            &fx.ctx,
            corral_core::agent::AgentRole::Worker,
            &resolved,
        )
        .unwrap();
    }
    assert!(receive.try_recv().is_err());
    assert!(fx.events("metered_notice").is_empty());
    assert_notice_before_pane_run(&fx, "worker");
    // When dispatch performs its own repeated preflight and submits the one host command.
    let outcome = crate::dispatch(&fx.ctx, &task.id, &crate::DispatchOpts::default()).unwrap();
    assert!(matches!(outcome, crate::DispatchOutcome::Started { .. }));
    // Then the launch, rather than any readiness probe, presented exactly one notice.
    assert_one_notice(&fx, "worker", &receive);
    assert_eq!(fx.calls("pane run").len(), 1);
}

#[test]
fn planner_launch_emits_one_metered_notice_before_submission() {
    isolated("dispatch::metered_tests::planner_launch_child");
}

#[test]
#[ignore = "driven with private HOME/XDG by the parent test"]
fn planner_launch_child() {
    let (fx, _foreground) = planner_fixture();
    let task = fx
        .ctx
        .store
        .create("Metered planner", Some("corral"), Status::Ready, "Fixture")
        .unwrap();
    let (send, receive) = mpsc::channel();
    let _capture = corral_core::messages::capture_messages(send);
    assert_notice_before_pane_run(&fx, "planner");
    // When real plan dispatch submits its command and its bootstrap reaches a fake host.
    let outcome = crate::dispatch(
        &fx.ctx,
        &task.id,
        &crate::DispatchOpts {
            plan: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(outcome, crate::DispatchOutcome::Started { .. }));
    assert_planner_host(&fx, &task.id);
    // Then the actual host received the selected inputs after exactly one notice.
    assert_one_notice(&fx, "planner", &receive);
    assert_eq!(fx.calls("pane run").len(), 1);
}

#[test]
fn prepared_planner_launch_carries_bedrock_inputs_and_one_metered_notice() {
    isolated("dispatch::metered_tests::prepared_planner_launch_child");
}

#[test]
#[ignore = "driven with private HOME/XDG by the parent test"]
fn prepared_planner_launch_child() {
    // Given an authorized D1 claim persisted before any pane submission.
    let (mut fx, _foreground) = planner_fixture();
    let task = fx
        .ctx
        .store
        .create(
            "Prepared planner",
            Some("corral"),
            Status::Running,
            "Fixture",
        )
        .unwrap();
    let resolved = fx
        .ctx
        .store
        .config
        .resolve_profile(
            &corral_core::ProfileChoice {
                profile: Some("bedrock".into()),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
    let output = fx
        .ctx
        .store
        .state_dir()
        .join("plans")
        .join(&task.id)
        .join("D1");
    let run = serde_json::from_value(json!({
        "id":"D1", "profile":"bedrock", "harness":"pi-native", "model":MODEL, "delivery":"inbox",
        "vendor":"amazon-bedrock", "effort":null,
        "execution":ExecutionSnapshot::new(&fx.ctx.store.config, resolved),
        "route":corral_core::RouteSource::Override,
        "spec_revision":crate::plan_spec_revision(&task).unwrap(),
        "started":jiff::Timestamp::now(), "output_dir":output,
        "rounds":0, "state":"planning",
    }))
    .unwrap();
    let task = fx
        .ctx
        .store
        .update(&task.id, |task| {
            task.plan_runs.push(run);
            Ok(())
        })
        .unwrap();
    // A changed current model must not replace the prepared execution snapshot.
    fx.ctx
        .store
        .config
        .profiles
        .get_mut("bedrock")
        .unwrap()
        .model = Some("us.amazon.nova-micro-v1:0".into());
    fx.ctx.store.save_config().unwrap();
    let mut action =
        crate::operating::identity(&task.id, "D1", OperatingActionKind::Plan, "launch");
    action.round = Some(0);
    crate::operating::authorize(&fx.ctx, action, crate::operating::Origin::Owner).unwrap();
    write(
        &output.join("launch.json"),
        &json!({
            "run":"D1", "attempt":"launch", "round":0, "previous_round":0,
            "plan":null, "review":null, "notes":null, "force_resources":false,
            "command":null, "herdr":null,
        })
        .to_string(),
    );
    let (send, receive) = mpsc::channel();
    let _capture = corral_core::messages::capture_messages(send);
    assert_notice_before_pane_run(&fx, "planner");
    // When resume_prepared reuses the durable identity and submits its native command.
    assert!(crate::plan_dispatch::resume_prepared(&fx.ctx, &task).unwrap());
    assert_planner_host(&fx, &task.id);
    // Then one D1 host receives the original model, prompt and binding, after one notice.
    assert_eq!(fx.ctx.store.get(&task.id).unwrap().plan_runs.len(), 1);
    assert_one_notice(&fx, "planner", &receive);
    assert_eq!(fx.calls("pane run").len(), 1);
}

fn planner_fixture() -> (Fx, HostProc) {
    let (mut fx, _) = fixture();
    fx.ctx.store.config.dispatch.paused = false;
    fx.ctx.store.config.defaults.profile = Some("bedrock".into());
    fx.ctx.store.save_config().unwrap();
    let assets = fx.ctx.store.config.agent.assets_dir.as_ref().unwrap();
    write(
        &assets.join("compat.json"),
        r#"{"verdict":"GO","bun":{"range":"1.4.2"},"providers":["openai","anthropic","amazon-bedrock"]}"#,
    );
    let host = assets.join("corral-pi-host");
    write(
        &host,
        r#"#!/usr/bin/env python3
import json, os, pathlib, sys
assert sys.argv[1] == "--launch-fd" and len(sys.argv) == 3, sys.argv
with os.fdopen(int(sys.argv[2])) as pipe:
    message = json.load(pipe)
events = pathlib.Path(message["root"], ".corral/events.jsonl").read_text().splitlines()
notices = [json.loads(line) for line in events if json.loads(line)["kind"] == "metered_notice"]
assert len(notices) == 1 and notices[0]["detail"]["role"] == "planner", notices
assert not any(key.startswith("AWS_") for key in os.environ)
fields = ("role", "task", "run", "model", "provider", "aws", "prompt_file", "cwd")
captured = {key: message[key] for key in fields}
captured["prompt"] = pathlib.Path(message["prompt_file"]).read_text()
print(json.dumps(captured))
"#,
    );
    std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Smoke observation has a real, fixture-owned foreground process, not a fabricated PID.
    let foreground = HostProc::spawn(true);
    let engine = fx.root().join("herdr");
    let source = std::fs::read_to_string(&engine).unwrap().replace(
        r#""foreground_processes":[]"#,
        &format!(
            r#""foreground_processes":[{{"pid":{},"argv":["/fixture/src/host/main.ts","--launch-fd","3"]}}]"#,
            foreground.pid()
        ),
    );
    let capture = r#"if [ "$1 $2" = "pane run" ]; then
python3 -c '
import pathlib, shlex, sys
outer = shlex.split(sys.argv[1])
assert outer[:2] == ["/bin/sh", "-c"], outer
script = shlex.shlex(outer[2], posix=True, punctuation_chars=True)
script.whitespace_split = True
wrapped = list(script)
index = wrapped.index("--")
assert wrapped[index+1:index+3] == ["/bin/sh", "-c"], wrapped
argv = shlex.split(wrapped[index+3])
assert argv[1:3] == ["agent", "launch"] and "--bootstrap" in argv, argv
path = pathlib.Path(argv[argv.index("--bootstrap")+1])
assert path.is_file(), path
pathlib.Path("planner-bootstrap").write_text(str(path))
' "$4" || exit 1
fi
case "$1 $2" in"#;
    write(&engine, &source.replace("case \"$1 $2\" in", capture));
    (fx, foreground)
}

fn assert_planner_host(fx: &Fx, task: &str) {
    // Consume exactly the bootstrap named by the real pane command, not a searched-for file.
    let path = std::path::PathBuf::from(
        std::fs::read_to_string(fx.root().join("planner-bootstrap")).unwrap(),
    );
    let bootstrap = consume_bootstrap(&path).unwrap();
    assert_eq!(
        bootstrap.recipient().unwrap(),
        corral_core::inbox::Recipient::PlanRun {
            task: task.into(),
            plan_run: "D1".into(),
        }
    );
    let ready = check(&bootstrap, path.parent().unwrap(), None).unwrap();
    let secrets = Secrets::generate().unwrap();
    write_ctl_cap(&ready.generation_dir, &secrets.ctl_cap).unwrap();
    let (mut command, pipe) = host_command(&bootstrap, &ready, &secrets).unwrap();
    // The real Rust host command passes its launch message over the inherited descriptor.
    let output = command.stdin(Stdio::null()).output().unwrap();
    drop(pipe);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let captured: Value = serde_json::from_slice(&output.stdout).unwrap();
    let prompt = fx
        .ctx
        .store
        .state_dir()
        .join("plans")
        .join(task)
        .join("D1/prompt.md");
    assert_eq!(
        captured,
        json!({
            "role":"planner", "task":task, "run":"D1", "model":MODEL,
            "provider":"amazon-bedrock", "aws":{"profile":"dev","region":"us-west-2"},
            "prompt_file":prompt, "cwd":fx.app(),
            "prompt":std::fs::read_to_string(&prompt).unwrap(),
        })
    );
}
