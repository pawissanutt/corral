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
