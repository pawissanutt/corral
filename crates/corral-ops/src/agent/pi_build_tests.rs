use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::agent::bridge::assets::{HOST_BINARY, asset_digest};
use crate::agent::hooks::pm::tests::fixture;
use crate::agent::install::{InstallRecord, RECORD_FILE, RECORD_VERSION};

fn code(error: &anyhow::Error) -> Option<&'static str> {
    error
        .downcast_ref::<super::super::pi::PiRefusal>()
        .map(|error| error.code)
}

fn fake_bun(dir: &Path) -> PathBuf {
    let path = dir.join("bun");
    crate::agent::test_support::write_executable(&path, "#!/bin/sh\necho 1.4.2\n");
    path
}

fn auth(root: &Path) {
    let dir = root.join("auth");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.join("openai-codex.json");
    std::fs::write(&path, "{}").unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn source_assets(assets: &Path) {
    std::fs::create_dir_all(assets.join("src/host")).unwrap();
    std::fs::write(assets.join(BUILD_ENTRY), "export {}\n").unwrap();
    std::fs::write(
        assets.join("compat.json"),
        json!({
            "verdict":"GO", "bun":{"range":">=1.4.0 <2.0.0"},
            "capabilities":["todo", "web.enabled"],
        })
        .to_string(),
    )
    .unwrap();
}

fn managed_assets(root: &Path, compiled: bool) -> PathBuf {
    let staging = root.join("staging");
    source_assets(&staging);
    if compiled {
        std::fs::remove_dir_all(staging.join("src")).unwrap();
        crate::agent::test_support::write_executable(
            &staging.join(HOST_BINARY),
            "#!/bin/sh\nexit 0\n",
        );
    }
    let digest = asset_digest(&staging).unwrap();
    let assets = root.join("assets").join(&digest);
    std::fs::create_dir_all(assets.parent().unwrap()).unwrap();
    std::fs::rename(staging, &assets).unwrap();
    let record = InstallRecord {
        version: RECORD_VERSION,
        digest,
        path: assets.clone(),
        bun: "1.4.2".into(),
        pi: "1.3.0".into(),
        installed_at: jiff::Timestamp::now(),
    };
    std::fs::write(root.join(RECORD_FILE), serde_json::to_vec(&record).unwrap()).unwrap();
    assets
}

fn message(mut reader: std::io::PipeReader) -> Value {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn listing(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                dirs.push(path.clone());
            }
            paths.push(path.strip_prefix(root).unwrap().to_path_buf());
        }
    }
    paths.sort();
    paths
}

#[test]
fn workspace_launch_carries_project_policy_profiles_and_capabilities_without_writes() {
    let mut fx = fixture();
    let data = tempfile::tempdir().unwrap();
    auth(data.path());
    let assets = fx.ctx.store.config.agent.assets_dir.clone().unwrap();
    source_assets(&assets);
    let config = &mut fx.ctx.store.config;
    config.projects.clear();
    config.projects.insert(
        "app".into(),
        serde_json::from_value(json!({"path":"app", "build_exit":"gate"})).unwrap(),
    );
    config.assistant.agent = Some("assistant-pi".into());
    let mut profile = config.profiles["pm-pi"].clone();
    profile.model = Some("gpt-6.1-sol".into());
    profile.effort = Some("high".into());
    config.profiles.insert("assistant-pi".into(), profile);
    config.build.max_parallel = 4;
    config.build.profiles.insert(
        "fast".into(),
        corral_core::BuildProfileCfg {
            model: "anthropic/claude-opus-4-8".into(),
            effort: Some("medium".into()),
            description: Some("Small independent changes".into()),
        },
    );
    fx.ctx.store.save_config().unwrap();
    let before = listing(&fx.root().join(".corral"));
    let (command, reader) = build_command(
        data.path(),
        &BuildArgs::default(),
        &fx.root().join("app"),
        Some(fake_bun(data.path())),
        Some(fx.root()),
    )
    .unwrap();
    let launch = message(reader);
    assert_eq!(listing(&fx.root().join(".corral")), before);
    assert_eq!(launch["workspace_root"], json!(fx.root()));
    assert_eq!(launch["project"], "app");
    assert_eq!(launch["settings"]["exit_policy"], "gate");
    assert_eq!(launch["settings"]["max_parallel"], 4);
    assert_eq!(
        launch["settings"]["profiles"],
        json!([{
            "name":"fast", "model":"anthropic/claude-opus-4-8", "effort":"medium",
            "description":"Small independent changes",
        }])
    );
    assert!(launch["capabilities"]["enabled"].is_array());
    assert_eq!(launch["model"], "gpt-6.1-sol");
    assert_eq!(launch["effort"], "high");
    assert!(launch.get("guidance").is_none());
    assert_eq!(command.get_current_dir(), Some(assets.as_path()));
    let argv: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(argv[0], "--no-env-file");
    assert_eq!(argv[1], assets.join(BUILD_ENTRY).to_string_lossy());
    assert_eq!(argv[2], "--launch-fd");
}

#[test]
fn unbound_launch_uses_none_and_bare_defaults_with_source_or_compiled_assets() {
    let fx = fixture();
    for compiled in [false, true] {
        let data = tempfile::tempdir().unwrap();
        auth(data.path());
        let assets = managed_assets(data.path(), compiled);
        let (command, reader) = build_command(
            data.path(),
            &BuildArgs::default(),
            &fx.root().join("app"),
            (!compiled).then(|| fake_bun(data.path())),
            None,
        )
        .unwrap();
        let launch = message(reader);
        assert_eq!(launch["settings"]["exit_policy"], "none");
        assert_eq!(launch["settings"]["max_parallel"], 3);
        assert!(launch.get("workspace_root").is_none());
        assert!(launch.get("project").is_none());
        assert_eq!(launch["provider"], "openai-codex");
        assert_eq!(launch["model"], "gpt-6.1-sol");
        if compiled {
            assert_eq!(Path::new(command.get_program()), assets.join(HOST_BINARY));
            assert_eq!(command.get_args().next().unwrap(), "build");
        }
    }
}

#[test]
fn missing_project_disables_workspace_exit_and_unset_project_policy_is_review() {
    let mut fx = fixture();
    let data = tempfile::tempdir().unwrap();
    auth(data.path());
    source_assets(fx.ctx.store.config.agent.assets_dir.as_ref().unwrap());
    fx.ctx.store.config.projects.clear();
    fx.ctx.store.save_config().unwrap();
    let bun = fake_bun(data.path());
    let launch = message(
        build_command(
            data.path(),
            &BuildArgs::default(),
            &fx.root().join("app"),
            Some(bun.clone()),
            Some(fx.root()),
        )
        .unwrap()
        .1,
    );
    assert_eq!(launch["settings"]["exit_policy"], "none");
    fx.ctx.store.config.projects.insert(
        "app".into(),
        serde_json::from_value(json!({"path":"app"})).unwrap(),
    );
    fx.ctx.store.save_config().unwrap();
    let launch = message(
        build_command(
            data.path(),
            &BuildArgs::default(),
            &fx.root().join("app"),
            Some(bun),
            Some(fx.root()),
        )
        .unwrap()
        .1,
    );
    assert_eq!(launch["settings"]["exit_policy"], "review");
}

#[test]
fn non_git_and_bad_identity_flags_refuse_before_assets_or_state() {
    let data = tempfile::tempdir().unwrap();
    let args = BuildArgs::default();
    let error = build_command(data.path(), &args, data.path(), None, None).unwrap_err();
    assert_eq!(code(&error), Some("not_a_repository"));
    for args in [
        BuildArgs {
            name: Some("Invalid_Name".into()),
            ..Default::default()
        },
        BuildArgs {
            resume: Some("B-abcdef".into()),
            here: true,
            ..Default::default()
        },
        BuildArgs {
            resume: Some("B-abcdef".into()),
            name: Some("valid".into()),
            ..Default::default()
        },
        BuildArgs {
            resume: Some("A-42".into()),
            ..Default::default()
        },
    ] {
        let error = build_command(data.path(), &args, data.path(), None, None).unwrap_err();
        assert_eq!(code(&error), Some("usage_error"));
    }
    assert!(!data.path().join(".corral").exists());
}

#[test]
fn old_source_installation_refuses_build_and_unknown_profile_provider_refuses_usage() {
    let mut fx = fixture();
    let data = tempfile::tempdir().unwrap();
    let args = BuildArgs::default();
    let cwd = fx.root().join("app");
    let error = build_command(data.path(), &args, &cwd, None, Some(fx.root())).unwrap_err();
    assert_eq!(code(&error), Some("build_unavailable"));
    source_assets(fx.ctx.store.config.agent.assets_dir.as_ref().unwrap());
    fx.ctx.store.config.build.profiles.insert(
        "bad".into(),
        corral_core::BuildProfileCfg {
            model: "unknown/model".into(),
            effort: None,
            description: None,
        },
    );
    fx.ctx.store.save_config().unwrap();
    let error = build_command(data.path(), &args, &cwd, None, Some(fx.root())).unwrap_err();
    assert_eq!(code(&error), Some("usage_error"));
}

fn session_file(data: &Path, id: &str, body: &str) {
    let dir = data.join("build").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("session.json"), body).unwrap();
}

fn session_record(id: &str, updated: &str) -> Value {
    json!({
        "version": 1, "id": id, "name": "checkout-copy", "created": "2026-11-01T09:00:00.000Z",
        "updated": updated, "repoRoot": "/repo", "worktree": "/repo/.worktrees/build-checkout-copy",
        "branch": "build/checkout-copy", "base": "abc123", "here": false,
        "sessionFile": format!("/data/build/{id}/parent.jsonl"),
    })
}

#[test]
fn list_sessions_reads_host_records_newest_first_and_skips_unresumable_ones() {
    let data = tempfile::tempdir().unwrap();
    assert!(list_sessions(data.path()).is_empty());
    let older = session_record("B-older1", "2026-11-02T10:00:00.000Z");
    session_file(data.path(), "B-older1", &older.to_string());
    let mut newer = session_record("B-newer1", "2026-11-03T10:00:00.000Z");
    newer["here"] = json!(true);
    newer["worktree"] = json!("/repo");
    newer["project"] = json!("app");
    newer["workspaceRoot"] = json!("/ws");
    newer["exit"] = json!({"choice": "ship", "at": "2026-11-03T10:00:00.000Z", "task": "T-9"});
    session_file(data.path(), "B-newer1", &newer.to_string());
    let mut untranscribed = session_record("B-nofile1", "2026-11-04T10:00:00.000Z");
    untranscribed.as_object_mut().unwrap().remove("sessionFile");
    session_file(data.path(), "B-nofile1", &untranscribed.to_string());
    session_file(data.path(), "B-broken1", "{\"id\": \"B-broken1\"");
    let sessions = list_sessions(data.path());
    let ids: Vec<_> = sessions.iter().map(|session| session.id.as_str()).collect();
    assert_eq!(ids, ["B-newer1", "B-older1"]);
    assert_eq!(
        sessions[0],
        BuildSessionSummary {
            id: "B-newer1".into(),
            name: "checkout-copy".into(),
            repo_root: "/repo".into(),
            worktree: "/repo".into(),
            here: true,
            project: Some("app".into()),
            updated: "2026-11-03T10:00:00.000Z".into(),
            exited: Some("ship".into()),
        }
    );
    assert_eq!(
        sessions[1].worktree,
        PathBuf::from("/repo/.worktrees/build-checkout-copy")
    );
    assert!(!sessions[1].here);
    assert_eq!(sessions[1].project, None);
    assert_eq!(sessions[1].exited, None);
}

#[test]
fn resume_resolves_the_recorded_repository_instead_of_the_callers_directory() {
    let mut recorded = fixture();
    let mut caller = fixture();
    let data = tempfile::tempdir().unwrap();
    let unrelated = tempfile::tempdir().unwrap();
    auth(data.path());
    for (fx, project, policy, parallel) in [
        (&mut recorded, "recorded", "gate", 5),
        (&mut caller, "caller", "none", 2),
    ] {
        let config = &mut fx.ctx.store.config;
        source_assets(config.agent.assets_dir.as_ref().unwrap());
        config.projects.clear();
        config.projects.insert(
            project.into(),
            serde_json::from_value(json!({"path":"app", "build_exit":policy})).unwrap(),
        );
        config.profiles.get_mut("pm-pi").unwrap().model = Some("gpt-6.1-sol".into());
        config.build.max_parallel = parallel;
        config
            .agent
            .capabilities
            .set(AgentRole::Assistant, "todo", project == "caller")
            .unwrap();
        fx.ctx.store.save_config().unwrap();
    }
    let repo = recorded.root().join("app");
    let id = "B-resume1";
    let mut record = session_record(id, "2026-11-03T10:00:00.000Z");
    record["repoRoot"] = json!(repo);
    record["project"] = json!("recorded");
    record["workspaceRoot"] = json!(recorded.root());
    session_file(data.path(), id, &record.to_string());
    let before = listing(&recorded.root().join(".corral"));
    let caller_before = listing(&caller.root().join(".corral"));
    let bun = fake_bun(data.path());
    let args = BuildArgs {
        resume: Some(id.into()),
        ..Default::default()
    };
    for (cwd, workspace) in [
        (caller.root().join("app"), Some(caller.root())),
        (unrelated.path().to_path_buf(), None),
    ] {
        let launch = message(
            build_command(data.path(), &args, &cwd, Some(bun.clone()), workspace)
                .unwrap()
                .1,
        );
        assert_eq!(launch["repo_root"], json!(repo));
        assert_eq!(launch["workspace_root"], json!(recorded.root()));
        assert_eq!(launch["project"], "recorded");
        assert_eq!(launch["settings"]["exit_policy"], "gate");
        assert_eq!(launch["settings"]["max_parallel"], 5);
        assert_eq!(launch["model"], "gpt-6.1-sol");
        assert!(
            !launch["capabilities"]["enabled"]
                .as_array()
                .unwrap()
                .contains(&json!("todo"))
        );
    }
    let launch = message(
        build_command(
            data.path(),
            &BuildArgs::default(),
            &caller.root().join("app"),
            Some(bun.clone()),
            Some(caller.root()),
        )
        .unwrap()
        .1,
    );
    assert_eq!(launch["repo_root"], json!(caller.root().join("app")));
    assert_eq!(launch["project"], "caller");
    assert_eq!(launch["settings"]["exit_policy"], "none");
    assert_eq!(launch["settings"]["max_parallel"], 2);
    let overrides = BuildArgs {
        model: Some("gpt-override".into()),
        effort: Some("medium".into()),
        provider: Some("openai-codex".into()),
        ..args
    };
    let launch = message(
        build_command(data.path(), &overrides, unrelated.path(), Some(bun), None)
            .unwrap()
            .1,
    );
    assert_eq!(launch["provider"], "openai-codex");
    assert_eq!(launch["model"], "gpt-override");
    assert_eq!(launch["effort"], "medium");
    assert_eq!(listing(&recorded.root().join(".corral")), before);
    assert_eq!(listing(&caller.root().join(".corral")), caller_before);
}

#[test]
fn resume_refuses_unknown_unreadable_and_missing_repository_records() {
    let data = tempfile::tempdir().unwrap();
    let id = "B-missing1";
    let args = BuildArgs {
        resume: Some(id.into()),
        ..Default::default()
    };
    let error = build_command(data.path(), &args, data.path(), None, None).unwrap_err();
    assert_eq!(code(&error), Some("build_session_unknown"));
    assert!(error.to_string().contains(id));
    session_file(data.path(), id, "{malformed");
    let error = build_command(data.path(), &args, data.path(), None, None).unwrap_err();
    assert_eq!(code(&error), Some("build_session_unknown"));
    assert!(error.to_string().contains(id));
    let mut record = session_record(id, "2026-11-03T10:00:00.000Z");
    let missing = data.path().join("missing-repository");
    record["repoRoot"] = json!(missing);
    session_file(data.path(), id, &record.to_string());
    let error = build_command(data.path(), &args, data.path(), None, None).unwrap_err();
    assert_eq!(code(&error), Some("build_session_unknown"));
    assert!(error.to_string().contains(id));
    assert!(error.to_string().contains(missing.to_str().unwrap()));
    record["padding"] = json!("x".repeat(SESSION_MAX_BYTES as usize));
    session_file(data.path(), id, &record.to_string());
    let error = build_command(data.path(), &args, data.path(), None, None).unwrap_err();
    assert_eq!(code(&error), Some("build_session_unknown"));
}
