//! Bootstrap integration uses a private fake engine and never launches a provider.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;

use corral_core::{ProfileSetupState, Store};
use corral_herdr::Herdr;
use corral_ops::{
    Ctx, MODEL_SELECTION_PROCEDURE, SetupConnection, open_setup_pm, setup_answers,
    setup_connection, setup_status, skill_markdown,
};
use serde_json::Value;

const ENGINE: &str = r#"#!/usr/bin/python3
import json, pathlib, shlex, sys
root = pathlib.Path(__file__).parent
args = sys.argv[1:]
if args[:1] == ['--session']:
    args = args[2:]
with (root / 'calls.jsonl').open('a') as output:
    output.write(json.dumps(args) + '\n')
pane = {'pane_id': 'w1:p1', 'workspace_id': 'w1', 'tab_id': 'w1:t1'}
if args[:2] == ['workspace', 'create']:
    result = {'workspace': {'workspace_id': 'w1'}, 'root_pane': pane}
elif args[:2] == ['pane', 'get']:
    result = {'pane': pane}
elif args[:2] == ['pane', 'run']:
    argv = shlex.split(args[3])
    # The agent command is the argument after corral's launch wrapper marker.
    if 'corral-launch' in argv:
        argv = shlex.split(argv[argv.index('corral-launch') + 1])
    (root / 'launch-argv.json').write_text(json.dumps(argv))
    result = {}
else:
    result = {}
print(json.dumps({'id': 'fixture', 'result': result}))
"#;

fn fixture() -> (tempfile::TempDir, Ctx, SetupConnection) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(dir.path()).unwrap();
    store.config.projects.clear();
    store.config.profiles.clear();
    store.config.agents = BTreeMap::from([(
        "existing".into(),
        serde_json::from_value(serde_json::json!({
            "command": "fixture-agent {model_args} {prompt_file}"
        }))
        .unwrap(),
    )]);
    store.config.harnesses = BTreeMap::from([(
        "existing".into(),
        serde_json::from_value(serde_json::json!({
            "command": "fixture-agent {model_args} {prompt_file}",
            "flags": "omo"
        }))
        .unwrap(),
    )]);
    store.config.pm.agent = "existing".into();
    store.save_config().unwrap();
    let engine = dir.path().join("engine");
    fs::write(&engine, ENGINE).unwrap();
    fs::set_permissions(&engine, fs::Permissions::from_mode(0o755)).unwrap();
    let mut ctx = Ctx::from_store(store);
    ctx.herdr = Herdr::with_bin(engine, Some("corral-test".into()));
    let connection = SetupConnection {
        harness: "existing".into(),
        model: Some("local/only-model".into()),
        effort: Some("high".into()),
    };
    (dir, ctx, connection)
}

fn calls(dir: &std::path::Path) -> Vec<Vec<String>> {
    fs::read_to_string(dir.join("calls.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn one_connection_bootstraps_without_completed_profile_catalog() {
    // Given only one legacy connection, with no configured profile catalog.
    let (dir, ctx, connection) = fixture();
    let before = fs::read(ctx.store.config_path()).unwrap();

    // When starting the setup PM through the real launch service and fake engine.
    let pane = open_setup_pm(&ctx, &connection).unwrap();

    // Then the selected binding launches without changing mappings or claiming access.
    assert_eq!(pane, "w1:p1");
    assert_eq!(fs::read(ctx.store.config_path()).unwrap(), before);
    let state = setup_status(&ctx.store).unwrap();
    assert_eq!(state.record.state, ProfileSetupState::Gathering);
    assert!(!state.record.validation.access_checked);
    assert!(!state.record.validation.execution_tested);
    let state: Value =
        serde_json::from_slice(&fs::read(ctx.store.state_dir().join("pm.json")).unwrap()).unwrap();
    assert_eq!(
        state["bootstrap"],
        serde_json::to_value(&connection).unwrap()
    );
    let commands = calls(dir.path());
    let argv: Vec<String> =
        serde_json::from_slice(&fs::read(dir.path().join("launch-argv.json")).unwrap()).unwrap();
    assert!(
        argv.windows(2)
            .any(|args| args == ["--model", "local/only-model"])
    );
    assert!(argv.windows(2).any(|args| args == ["--thinking", "high"]));
    assert_eq!(
        commands
            .iter()
            .filter(|args| args[..2] == ["workspace", "create"])
            .count(),
        1
    );
    assert!(ctx.store.list().unwrap().is_empty());
    let prompt = fs::read_to_string(ctx.store.prompts_dir().join("pm.md")).unwrap();
    let procedure = MODEL_SELECTION_PROCEDURE.replace(
        "corral setup ",
        &format!("{} setup ", std::env::current_exe().unwrap().display()),
    );
    assert_eq!(prompt.matches(&procedure).count(), 1);
}

#[test]
fn reopening_setup_reuses_pane_and_reads_saved_answers_without_regenerating_prompt() {
    // Given a running PM and answers saved after its startup prompt was generated.
    let (dir, ctx, connection) = fixture();
    open_setup_pm(&ctx, &connection).unwrap();
    let prompt = fs::read(ctx.store.prompts_dir().join("pm.md")).unwrap();
    let answers = BTreeMap::from([("access".into(), "One local model".into())]);
    let saved = setup_answers(&ctx.store, &answers, 1).unwrap();

    // When the PM is reopened and reads current setup status with its original context.
    assert_eq!(open_setup_pm(&ctx, &connection).unwrap(), "w1:p1");
    let current = setup_status(&ctx.store).unwrap();

    // Then saved answers are discoverable without another launch or startup rewrite.
    assert_eq!(current.record, saved);
    assert_eq!(current.record.answers, answers);
    assert_eq!(
        fs::read(ctx.store.prompts_dir().join("pm.md")).unwrap(),
        prompt
    );
    assert_eq!(
        calls(dir.path())
            .iter()
            .filter(|args| args[..2] == ["workspace", "create"])
            .count(),
        1
    );
}

#[test]
fn unsupported_bootstrap_settings_fail_before_setup_or_engine_writes() {
    // Given an existing harness and an unsupported effort.
    let (dir, ctx, mut connection) = fixture();
    connection.effort = Some("unsupported".into());

    // When confirming that invalid connection.
    let result = open_setup_pm(&ctx, &connection);

    // Then setup stays unstarted and no engine is contacted.
    assert!(result.is_err());
    assert_eq!(
        setup_status(&ctx.store).unwrap().record.state,
        ProfileSetupState::NotStarted
    );
    assert!(!dir.path().join("calls.jsonl").exists());
}

#[test]
fn default_connection_does_not_invent_model_or_provider() {
    // Given a connection that uses its existing harness defaults.
    let (_dir, ctx, mut connection) = fixture();
    connection.model = None;
    connection.effort = None;

    // When resolving the bootstrap binding.
    let binding = setup_connection(&ctx.store.config, &connection).unwrap();

    // Then no model, provider, effort, or unrelated profile is guessed.
    assert!(binding.model.is_none());
    assert!(binding.vendor.is_none());
    assert!(binding.effort.is_none());
    assert!(binding.model_args.is_empty());
}

#[test]
fn installed_skill_ships_the_authoritative_procedure_bytes() {
    // Given the authoritative embedded runtime procedure.
    let procedure = MODEL_SELECTION_PROCEDURE;
    // When generating the installable skill outside a source checkout.
    let skill = skill_markdown();
    // Then it carries the exact shipped copy, rather than a drifting second procedure.
    assert_eq!(skill.matches(procedure).count(), 1);
}
