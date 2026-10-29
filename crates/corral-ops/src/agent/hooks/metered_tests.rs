//! Offline launch checks run in a subprocess with private HOME/XDG paths. The fake herdr checks
//! the durable notice at pane-run time; it never starts an agent or contacts AWS.
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;

use corral_core::{ProfileChoice, ResolvedProfile, Status};
use serde_json::json;

use crate::agent::bridge::fixture::{self as f, Fx};
use crate::agent::{credentials, install};

pub(crate) const MODEL: &str = "us.meta.llama3-3-70b-instruct-v1:0";

pub(crate) fn isolated(test: &str) {
    let tmp = tempfile::tempdir().unwrap();
    for dir in ["home", "data", "config", "state", "cache", "runtime"] {
        let path = tmp.path().join(dir);
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--ignored", "--nocapture"])
        .env("HOME", tmp.path().join("home"))
        .env("XDG_DATA_HOME", tmp.path().join("data"))
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("XDG_STATE_HOME", tmp.path().join("state"))
        .env("XDG_CACHE_HOME", tmp.path().join("cache"))
        .env("XDG_RUNTIME_DIR", tmp.path().join("runtime"))
        .env("CORRAL_METERED_FIXTURE", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{test}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(crate) fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

pub(crate) fn fixture() -> (Fx, ResolvedProfile) {
    assert_eq!(std::env::var("CORRAL_METERED_FIXTURE").unwrap(), "1");
    let mut fx = f::fixture(false, 1);
    fx.ctx.store.config.profiles.insert(
        "bedrock".into(),
        serde_json::from_value(json!({
            "harness":"pi-native", "vendor":"amazon-bedrock", "model":MODEL,
        }))
        .unwrap(),
    );
    // A high limit keeps the fixture's existing worker from blocking a second test launch.
    fx.ctx.store.config.limits.running = 8;
    fx.ctx.store.config.review.require_cross_family = Some(false);
    let assets = fx.ctx.store.config.agent.assets_dir.as_ref().unwrap();
    write(
        &assets.join("compat.json"),
        r#"{"verdict":"GO","providers":["openai","anthropic","amazon-bedrock"]}"#,
    );
    for module in [
        "roles/worker",
        "roles/reviewer",
        "roles/pm",
        "auth",
        "delivery",
    ] {
        write(&assets.join(format!("src/{module}/index.ts")), "export {};");
    }
    fx.ctx.store.save_config().unwrap();
    let root = install::data_root().unwrap();
    let auth = credentials::auth_dir(&root);
    credentials::ensure_auth_dir(&auth).unwrap();
    let binding = auth.join("amazon-bedrock.json");
    write(
        &binding,
        r#"{"type":"aws_profile","profile":"dev","region":"us-west-2"}"#,
    );
    std::fs::set_permissions(&binding, std::fs::Permissions::from_mode(0o600)).unwrap();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    write(
        &home.join(".aws/credentials"),
        "[dev]\naws_access_key_id = FIXTUREKEY\naws_secret_access_key = FIXTURESECRET\n",
    );
    let resolved = fx
        .ctx
        .store
        .config
        .resolve_profile(
            &ProfileChoice {
                profile: Some("bedrock".into()),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
    (fx, resolved)
}

/// Require the notice to be durable when a host launch is submitted to the fake pane.
pub(crate) fn assert_notice_before_pane_run(fx: &Fx, role: &str) {
    let bin = fx.root().join("herdr");
    let source = std::fs::read_to_string(&bin).unwrap();
    let check = format!(
        r#"if [ "$1 $2" = "pane run" ]; then
python3 -c '
import json, pathlib
events = [json.loads(line) for line in pathlib.Path(".corral/events.jsonl").read_text().splitlines()]
notices = [e for e in events if e["kind"] == "metered_notice" and e["detail"]["role"] == "{role}"]
assert len(notices) == 1, notices
' || exit 1
fi
case "$1 $2" in"#
    );
    write(&bin, &source.replace("case \"$1 $2\" in", &check));
}

pub(crate) fn assert_one_notice(
    fx: &Fx,
    role: &str,
    receiver: &mpsc::Receiver<corral_core::messages::Message>,
) {
    let messages: Vec<_> = receiver
        .try_iter()
        .filter(|m| m.source == "metered")
        .collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    let events = fx.events("metered_notice");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        events[0].detail,
        json!({
            "role":role, "profile":"bedrock", "model":MODEL,
            "aws_profile":"dev", "region":"us-west-2",
        })
    );
}

#[test]
fn reviewer_launch_emits_one_metered_notice_before_submission() {
    isolated("agent::hooks::metered_tests::reviewer_launch_child");
}

#[test]
#[ignore = "driven with private HOME/XDG by the parent test"]
fn reviewer_launch_child() {
    let (fx, resolved) = fixture();
    let (send, receive) = mpsc::channel();
    let _capture = corral_core::messages::capture_messages(send);
    // Given repeated readiness checks for the same reviewer.
    super::review::preflight(&fx.ctx, &resolved).unwrap();
    super::review::preflight(&fx.ctx, &resolved).unwrap();
    assert!(receive.try_recv().is_err());
    assert!(fx.events("metered_notice").is_empty());
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut().unwrap().vendor = Some("anthropic".into());
            task.current_run_mut().unwrap().outcome = Some(corral_core::RunOutcome::Done);
            task.status = Status::Review;
            Ok(())
        })
        .unwrap();
    assert_notice_before_pane_run(&fx, "reviewer");
    // When review-run builds and submits its child launch.
    crate::review_run(&fx.ctx, &fx.id, Some("bedrock")).unwrap();
    // Then one message and event were presented before the fake pane saw the command.
    assert_one_notice(&fx, "reviewer", &receive);
    assert_eq!(fx.calls("pane run").len(), 1);
}

#[test]
fn pm_open_emits_one_metered_notice_despite_repeated_preflight() {
    isolated("agent::hooks::metered_tests::pm_launch_child");
}

#[test]
#[ignore = "driven with private HOME/XDG by the parent test"]
fn pm_launch_child() {
    let (mut fx, resolved) = fixture();
    fx.ctx.store.config.pm.agent = "bedrock".into();
    fx.ctx.store.save_config().unwrap();
    let selection = corral_core::OperatingSelection {
        tasks: [fx.id.clone()].into(),
        ..Default::default()
    };
    let preview = fx.ctx.store.preview_operating_policy(&selection).unwrap();
    fx.ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", jiff::Timestamp::now())
        .unwrap();
    let (send, receive) = mpsc::channel();
    let _capture = corral_core::messages::capture_messages(send);
    super::pm::preflight(&fx.ctx, Some(&resolved)).unwrap();
    super::pm::preflight(&fx.ctx, Some(&resolved)).unwrap();
    assert!(receive.try_recv().is_err());
    assert!(fx.events("metered_notice").is_empty());
    assert_notice_before_pane_run(&fx, "pm");
    // When PM open builds and submits its command (and internally preflights again).
    crate::open_pm(&fx.ctx).unwrap();
    assert_one_notice(&fx, "pm", &receive);
    assert_eq!(fx.calls("pane run").len(), 1);
}

#[test]
fn bedrock_doctor_and_preflight_inspect_fixture_files_without_writing_or_secrets() {
    isolated("agent::hooks::metered_tests::bedrock_readiness_child");
}

#[test]
#[ignore = "driven with private HOME/XDG by the parent test"]
fn bedrock_readiness_child() {
    let (fx, resolved) = fixture();
    let root = install::data_root().unwrap();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let keys = home.join(".aws/credentials");
    let original = std::fs::read(&keys).unwrap();
    let modified = std::fs::metadata(&keys).unwrap().modified().unwrap();
    let (send, receive) = mpsc::channel();
    let _capture = corral_core::messages::capture_messages(send);
    let ready = super::doctor::bedrock_check(Some(&root), Some(&home));
    assert!(ready.ok, "{ready:?}");
    assert!(ready.message.contains("static") && ready.message.contains("metered"));
    for role in [
        corral_core::agent::AgentRole::Worker,
        corral_core::agent::AgentRole::Reviewer,
        corral_core::agent::AgentRole::Pm,
    ] {
        super::preflight::check(&fx.ctx, role, &resolved).unwrap();
    }
    assert!(receive.try_recv().is_err());
    assert!(fx.events("metered_notice").is_empty());
    assert_eq!(std::fs::read(&keys).unwrap(), original);
    assert_eq!(
        std::fs::metadata(&keys).unwrap().modified().unwrap(),
        modified
    );
    assert!(!ready.message.contains("FIXTUREKEY"));
    assert!(!ready.message.contains("FIXTURESECRET"));
    // Missing profile and unsupported mechanisms refuse with actionable, redacted diagnostics.
    for (content, hint) in [
        (
            "[other]\naws_access_key_id = FIXTUREKEY\naws_secret_access_key = FIXTURESECRET\n",
            "aws configure --profile dev",
        ),
        (
            "[dev]\ncredential_process = secret-process\n",
            "credential_process",
        ),
    ] {
        write(&keys, content);
        let check = super::doctor::bedrock_check(Some(&root), Some(&home));
        assert!(!check.ok);
        let error = super::preflight::bedrock(&root, &home).unwrap_err();
        assert_eq!(error.code, "credentials_unavailable");
        assert!(error.detail.contains(hint), "{error}");
        assert!(!format!("{check:?} {error}").contains("secret-process"));
    }
    // No binding is a login refusal, not an inferred ambient AWS login.
    std::fs::remove_file(credentials::auth_dir(&root).join("amazon-bedrock.json")).unwrap();
    let missing = super::preflight::bedrock(&root, &home).unwrap_err();
    assert_eq!(missing.code, "credentials_unavailable");
    assert!(
        missing
            .detail
            .contains("corral agent login --provider amazon-bedrock")
    );
}

#[test]
fn sso_doctor_and_preflight_report_valid_expired_and_missing_tokens_read_only() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("data");
    credentials::ensure_auth_dir(&credentials::auth_dir(&root)).unwrap();
    let binding = credentials::auth_dir(&root).join("amazon-bedrock.json");
    write(
        &binding,
        r#"{"type":"aws_profile","profile":"dev","region":"us-west-2"}"#,
    );
    std::fs::set_permissions(&binding, std::fs::Permissions::from_mode(0o600)).unwrap();
    write(
        &home.join(".aws/config"),
        "[profile dev]\nsso_session=test\nsso_account_id=123456789012\nsso_role_name=Developer\n\
         [sso-session test]\nsso_start_url=https://example.awsapps.com/start\nsso_region=us-east-1\n",
    );
    let cache = home.join(".aws/sso/cache/a94a8fe5ccb19ba61c4c0873d391e987982fbbd3.json");
    for (expiry, expected) in [
        ("2099-01-01T00:00:00Z", true),
        ("2000-01-01T00:00:00Z", false),
    ] {
        write(
            &cache,
            &format!(r#"{{"expiresAt":"{expiry}","accessToken":"SECRET-TOKEN"}}"#),
        );
        let before = std::fs::read(&cache).unwrap();
        let modified = std::fs::metadata(&cache).unwrap().modified().unwrap();
        let check = super::doctor::bedrock_check(Some(&root), Some(&home));
        assert_eq!(check.ok, expected, "{check:?}");
        assert!(!format!("{check:?}").contains("SECRET-TOKEN"));
        if expected {
            assert!(check.message.contains(expiry), "{check:?}");
            super::preflight::bedrock(&root, &home).unwrap();
        } else {
            let error = super::preflight::bedrock(&root, &home).unwrap_err();
            assert_eq!(error.code, "credentials_unavailable");
            assert!(error.detail.contains("aws sso login --profile dev"));
        }
        assert_eq!(std::fs::read(&cache).unwrap(), before);
        assert_eq!(
            std::fs::metadata(&cache).unwrap().modified().unwrap(),
            modified
        );
        assert_eq!(
            std::fs::read_dir(cache.parent().unwrap()).unwrap().count(),
            1
        );
    }
    std::fs::remove_file(&cache).unwrap();
    let missing = super::doctor::bedrock_check(Some(&root), Some(&home));
    assert!(!missing.ok);
    assert!(missing.fix.unwrap().contains("aws sso login --profile dev"));
    assert!(!cache.exists());
}

#[test]
fn non_bedrock_profiles_do_not_get_a_metered_notice() {
    let fx = crate::agent::test_support::fx();
    let resolved: ResolvedProfile = serde_json::from_value(json!({
        "profile":"codex", "harness":"pi-native", "command":"unused",
        "model":"gpt-fixture", "vendor":"openai", "effort":null, "model_args":[],
    }))
    .unwrap();
    let binding = crate::agent::aws_profile::AwsBinding {
        profile: "dev".into(),
        region: "us-west-2".into(),
    };
    let (send, receive) = mpsc::channel();
    let _capture = corral_core::messages::capture_messages(send);
    crate::dispatch::metered_notice(&fx.ctx, "T-001", "worker", &resolved).unwrap();
    assert!(super::preflight::metered_notice("reviewer", &resolved, &binding).is_none());
    assert!(receive.try_recv().is_err());
    assert!(fx.ctx.store.events().unwrap().is_empty());
}

#[test]
fn inferred_bedrock_preflight_refuses_before_assets_credentials_or_launch() {
    let mut fx = crate::agent::test_support::fx();
    fx.ctx.store.config.profiles.insert(
        "inferred".into(),
        serde_json::from_value(json!({
            "harness":"pi-native", "model":format!("amazon-bedrock/{MODEL}"),
        }))
        .unwrap(),
    );
    let resolved = fx
        .ctx
        .store
        .config
        .resolve_profile(
            &ProfileChoice {
                profile: Some("inferred".into()),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
    let error = super::preflight::check(&fx.ctx, corral_core::agent::AgentRole::Worker, &resolved)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("metered_vendor_not_explicit:")
    );
    assert!(fx.ctx.store.events().unwrap().is_empty());
}
