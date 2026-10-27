//! PM launch seam for a pi-native PM profile. Owner: T-252 child pm-role.
use anyhow::{Context, Result, ensure};
use corral_core::agent::AgentRole;
use corral_core::{ProfileChoice, ResolvedProfile};

use super::{consulted, is_pi_native};
use crate::Ctx;

/// pm open's preflight, before any policy delivery, workspace or pane: the bootstrap
/// connection's profile, else the configured `pm.agent` profile when its harness is pi-native.
pub fn preflight(ctx: &Ctx, bootstrap: Option<&ResolvedProfile>) -> Result<()> {
    let resolved = match bootstrap {
        Some(resolved) => resolved.clone(),
        None => {
            let config = &ctx.store.config;
            let name = &config.pm.agent;
            let harness = config
                .profiles
                .get(name)
                .map_or(name.as_str(), |p| p.harness.as_str());
            if !is_pi_native(harness) {
                return Ok(());
            }
            let choice = ProfileChoice {
                profile: Some(name.clone()),
                ..ProfileChoice::default()
            };
            config.resolve_profile(&choice, None, None)?
        }
    };
    super::preflight::check(ctx, AgentRole::Pm, &resolved)?;
    if is_pi_native(&resolved.harness) {
        ensure!(resolved.model.is_some(), "a Pi-native PM needs a model");
        project(ctx)?;
    }
    Ok(())
}

/// Only an approved, unambiguous operating scope selects the PM's project.
pub fn project(ctx: &Ctx) -> Result<String> {
    let config = ctx.store.config_layers()?.config()?;
    let effective = ctx.store.effective_operating_policy()?;
    ensure!(
        effective.authorization.is_some(),
        "pm_scope_unapproved: a Pi-native PM requires an approved operating scope"
    );
    let scope = &config.automation.scope;
    let projects: std::collections::BTreeSet<_> = scope
        .projects
        .keys()
        .chain(scope.tasks.values().map(|task| &task.project))
        .collect();
    ensure!(
        projects.len() == 1,
        "pm_scope_ambiguous: a Pi-native PM requires exactly one approved project, found {}",
        projects.len()
    );
    projects
        .into_iter()
        .next()
        .cloned()
        .context("no approved PM project")
}

pub fn profile(ctx: &Ctx) -> Result<ResolvedProfile> {
    let config = ctx.store.config_layers()?.config()?;
    config.check_profile_eligibility(
        &config.pm.agent,
        corral_core::ProfileRole::Pm,
        None,
        false,
    )?;
    let resolved = config.resolve_profile(
        &ProfileChoice {
            profile: Some(config.pm.agent.clone()),
            ..ProfileChoice::default()
        },
        None,
        None,
    )?;
    ensure!(
        is_pi_native(&resolved.harness),
        "the PM resume profile must be pi-native"
    );
    preflight(ctx, Some(&resolved))?;
    Ok(resolved)
}

pub fn command(ctx: &Ctx, pane: &str, resolved: &ResolvedProfile) -> Result<String> {
    consulted("pm");
    preflight(ctx, Some(resolved))?;
    let target = super::super::admission::BindTarget::Pm {
        project: project(ctx)?,
    };
    let spec = super::super::launch::HostSpec {
        cwd: ctx.store.root().to_path_buf(),
        model: resolved
            .model
            .clone()
            .context("a Pi-native PM needs a model")?,
        provider: resolved.model.as_deref().and_then(|model| {
            super::super::launch::host_provider(resolved.vendor.as_deref(), model)
        }),
        effort: resolved.effort.clone(),
        prompt_file: Some(ctx.store.prompts_dir().join("pm.md")),
    };
    super::super::launch::prepare(ctx, &target, pane, &spec)
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod tests {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use corral_core::{OperatingSelection, Profile};
    use serde_json::json;

    use crate::agent::bridge::fixture::{self as f, Fx};
    use crate::agent::test_support as ts;

    pub(crate) fn fixture() -> Fx {
        let mut fx = f::fixture(false, 1);
        let config = &mut fx.ctx.store.config;
        config.pm.agent = "pm-pi".into();
        let profile: Profile = serde_json::from_value(json!({
            "harness": "pi-native", "model": "gpt-fixture", "vendor": "openai"
        }))
        .unwrap();
        config.profiles.insert("pm-pi".into(), profile);
        let assets = config.agent.assets_dir.as_ref().unwrap();
        for module in ["roles/pm", "delivery", "auth"] {
            let dir = assets.join("src").join(module);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("index.ts"), "export {}").unwrap();
        }
        std::fs::write(assets.join("compat.json"), r#"{"verdict":"GO"}"#).unwrap();
        fx.ctx.store.save_config().unwrap();
        let selection = OperatingSelection {
            tasks: [fx.id.clone()].into(),
            ..Default::default()
        };
        let preview = fx.ctx.store.preview_operating_policy(&selection).unwrap();
        fx.ctx
            .store
            .confirm_operating_policy(&preview, "fixture owner", jiff::Timestamp::now())
            .unwrap();
        std::fs::write(fx.root().join("pm-pane"), "").unwrap();
        // Observe the durable ordering at the real adapter boundary, before pane_run returns.
        let bin = fx.root().join("herdr");
        let script = std::fs::read_to_string(&bin).unwrap();
        let check = r#"if [ "$1 $2" = "pane run" ]; then
python3 -c '
import json, pathlib
root = pathlib.Path(".corral")
pm = json.loads((root / "state/agent-pm.json").read_text())["state"]
assert pm["binding"]["generation"] == 2
assert pm["generations"][0]["fenced"] is not None
assert all(r["state"] != "reserved" for r in pm["receipts"])
assert any("kind: recovery_notice" in p.read_text().splitlines() for p in (root / "inbox/pm").glob("*.md"))
' || exit 1
fi
case "$1 $2" in"#;
        std::fs::write(&bin, script.replace("case \"$1 $2\" in", check)).unwrap();
        fx
    }

    /// A real authenticated executor stopped at the operation's durable effect boundary.
    pub(crate) fn kill_at(fx: &Fx, host: &ts::Host, op: &str, point: &str) {
        let payload = match op {
            "capture" => json!({"project":"corral","title":"captured","body":"fixture"}),
            "reply" => json!({"task":fx.id,"text":"fixture reply"}),
            _ => unreachable!(),
        };
        let request = crate::agent::tool_request_id(&host.identity.session_id, "pm-call");
        let envelope = ts::envelope(host, op, &request, payload, false);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "agent::hooks::pm::tests::pm_executor_child",
                "--ignored",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env("CORRAL_PM_TEST_ROOT", fx.root())
            .env("CORRAL_PM_TEST_OP", op)
            .env("CORRAL_PM_TEST_INPUT", String::from_utf8(envelope).unwrap())
            .env("CORRAL_AGENT_FAILPOINT", point)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(120);
        let marker = format!("FAILPOINT {point}");
        let reached = loop {
            match receive.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(line) if line.contains(&marker) => break true,
                Ok(_) => {}
                Err(_) => break false,
            }
        };
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(reached, "PM executor did not reach {point}");
    }

    #[test]
    #[ignore = "executor subprocess, invoked with an authenticated fixture envelope"]
    fn pm_executor_child() {
        let root = std::env::var("CORRAL_PM_TEST_ROOT").unwrap();
        let op = std::env::var("CORRAL_PM_TEST_OP").unwrap();
        let input = std::env::var("CORRAL_PM_TEST_INPUT").unwrap();
        let response = crate::agent::call(
            &f::ctx_at(std::path::Path::new(&root)),
            &op,
            input.as_bytes(),
        )
        .unwrap();
        panic!("executor returned before its armed failpoint: {response:?}");
    }

    #[test]
    fn resume_preflight_refuses_before_replacing_or_fencing_a_live_pm() {
        use crate::agent::bridge::fixture::HostProc;
        use corral_core::agent::{ContainerRef, pm_state};
        // Given a live PM whose required role module is no longer available.
        let fx = fixture();
        let host = HostProc::spawn(true);
        fx.bind(
            &crate::agent::BindTarget::Pm {
                project: "corral".into(),
            },
            "pm-pane",
            &host,
        );
        std::fs::remove_file(fx.root().join("assets/src/roles/pm/index.ts")).unwrap();
        // When resume --replace preflights the replacement.
        let error = crate::agent::recovery::resume_container(
            &f::ctx_at(fx.root()),
            &ContainerRef::Pm,
            true,
        )
        .unwrap_err();
        // Then the recorded process and generation remain untouched.
        assert!(error.to_string().contains("role_unavailable"), "{error}");
        assert!(
            !crate::agent::bridge::liveness::gone(&corral_core::agent::Executor {
                pid: host.pid(),
                start_time: host.start_time()
            })
            .unwrap()
        );
        let pm = pm_state::load(&fx.ctx.store).unwrap().unwrap();
        assert!(pm.state.generation(1).unwrap().fenced.is_none());
        assert_eq!(pm.state.binding.generation, 1);
        assert!(fx.calls("pane run").is_empty());
    }

    #[test]
    fn resume_refuses_a_different_approved_project_without_changing_the_binding() {
        use crate::agent::bridge::fixture::HostProc;
        use corral_core::agent::{ContainerRef, pm_state};
        // Given a dead PM originally bound to corral and a newly approved other scope.
        let mut fx = fixture();
        let mut host = HostProc::spawn(true);
        fx.bind(
            &crate::agent::BindTarget::Pm {
                project: "corral".into(),
            },
            "pm-pane",
            &host,
        );
        host.kill();
        fx.ctx.store.config.projects.insert(
            "other".into(),
            serde_json::from_value(json!({"path":"app"})).unwrap(),
        );
        fx.ctx.store.save_config().unwrap();
        let task = fx
            .ctx
            .store
            .create(
                "other",
                Some("other"),
                corral_core::Status::Ready,
                "fixture",
            )
            .unwrap();
        let selection = OperatingSelection {
            tasks: [task.id].into(),
            ..Default::default()
        };
        let preview = fx.ctx.store.preview_operating_policy(&selection).unwrap();
        fx.ctx
            .store
            .confirm_operating_policy(&preview, "fixture owner", jiff::Timestamp::now())
            .unwrap();
        // When resume attempts to use the changed scope.
        let error = crate::agent::recovery::resume_container(
            &f::ctx_at(fx.root()),
            &ContainerRef::Pm,
            false,
        )
        .unwrap_err();
        // Then only an explicit new launch may change project; recovery leaves generation 1 intact.
        assert!(error.to_string().starts_with("out_of_scope"), "{error}");
        let pm = pm_state::load(&fx.ctx.store).unwrap().unwrap();
        assert_eq!(pm.state.binding.project, "corral");
        assert_eq!(pm.state.binding.generation, 1);
        assert!(pm.state.generation(1).unwrap().fenced.is_none());
        assert!(fx.calls("pane run").is_empty());
    }

    #[test]
    fn pm_request_namespace_rejects_cross_target_and_cross_operation_collisions() {
        use crate::agent::{self, receipts::FAILPOINT};
        use corral_core::{Status, agent::ContainerRef};
        for (race, winning_op) in [(false, "comment"), (true, "comment"), (true, "capture")] {
            let mut fx = ts::fx();
            fx.ctx.store.config.projects.insert(
                "corral".into(),
                serde_json::from_value(json!({"path":fx.ctx.store.root()})).unwrap(),
            );
            fx.ctx.store.save_config().unwrap();
            let host = ts::bind(
                &fx,
                &agent::BindTarget::Pm {
                    project: "corral".into(),
                },
                "pm-pane",
                "pm-session",
            );
            ts::committed(&ts::hello(&fx, &host));
            let first = fx
                .ctx
                .store
                .create("first", Some("corral"), Status::Backlog, "")
                .unwrap();
            let second = fx
                .ctx
                .store
                .create("second", Some("corral"), Status::Backlog, "")
                .unwrap();
            let id = agent::tool_request_id("pm-session", "one-id");
            let payload = |task: &str| json!({"task":task,"text":"context"});
            let first_input = ts::envelope(&host, "comment", &id, payload(&first.id), false);
            let second_payload = if winning_op == "capture" {
                json!({"project":"corral","title":"winner","body":""})
            } else {
                payload(&second.id)
            };
            let second_input = ts::envelope(&host, winning_op, &id, second_payload, false);
            let (winner, winning_input, stored) = if race {
                // Admit one comment, commit its competitor, then release the first transaction.
                let ctx = crate::Ctx::from_store(fx.ctx.store.clone());
                let (arrived, arrival) = mpsc::channel();
                let (release, released) = mpsc::channel();
                let pending = std::thread::spawn(move || {
                    FAILPOINT.with(|slot| {
                        *slot.borrow_mut() = Some(Box::new(move |point| {
                            if point == "pm_admitted_before_commit" {
                                arrived.send(()).unwrap();
                                released.recv_timeout(Duration::from_secs(5)).unwrap();
                            }
                        }))
                    });
                    agent::call(&ctx, "comment", &first_input).unwrap()
                });
                arrival.recv_timeout(Duration::from_secs(5)).unwrap();
                let stored = ts::committed(&ts::send(&fx, &second_input, winning_op));
                release.send(()).unwrap();
                assert_eq!(ts::code(&pending.join().unwrap()), Some("payload_mismatch"));
                (
                    (winning_op == "comment").then_some(second.id.clone()),
                    second_input,
                    stored,
                )
            } else {
                let stored = ts::committed(&ts::send(&fx, &first_input, "comment"));
                assert_eq!(
                    ts::code(&ts::send(&fx, &second_input, "comment")),
                    Some("payload_mismatch")
                );
                (Some(first.id.clone()), first_input, stored)
            };
            for (op, payload) in [
                (
                    "capture",
                    json!({"project":"corral","title":"collision","body":""}),
                ),
                ("reply", payload(&second.id)),
                ("read_next", json!({})),
            ] {
                let response = ts::send(&fx, &ts::envelope(&host, op, &id, payload, false), op);
                assert_eq!(ts::code(&response), Some("payload_mismatch"), "{op}");
            }
            agent::fence_generation(&fx.ctx.store, &ContainerRef::Pm, 1, "fixture").unwrap();
            let replay = ts::send(&fx, &winning_input, winning_op);
            assert_eq!(replay.receipt(), Some(&stored));
            assert!(matches!(
                replay,
                agent::CallResponse::Receipt { replayed: true, .. }
            ));
            let tasks = fx.ctx.store.list().unwrap();
            assert_eq!(tasks.len(), if winning_op == "capture" { 3 } else { 2 });
            for task in tasks {
                assert_eq!(
                    task.comments.len(),
                    usize::from(Some(&task.id) == winner.as_ref())
                );
            }
        }
    }

    #[test]
    #[ignore = "driven by the offline PM cross-lane fixture in a private herdr server"]
    fn pm_cross_lane_setup() {
        use corral_core::agent::paths;
        use corral_core::{Status, Store};
        let spec: serde_json::Value =
            serde_json::from_str(&std::env::var("CORRAL_PM_LANE").unwrap()).unwrap();
        let text = |key: &str| spec[key].as_str().unwrap().to_owned();
        let root = std::path::PathBuf::from(text("root"));
        std::fs::create_dir_all(root.join("app")).unwrap();
        let mut store = Store::init(&root).unwrap();
        store.config.dispatch.paused = true;
        for project in ["inside", "outside"] {
            store.config.projects.insert(
                project.into(),
                serde_json::from_value(json!({"path":"app"})).unwrap(),
            );
        }
        store.save_config().unwrap();
        let ctx = crate::Ctx::from_store(Store::discover(&root).unwrap());
        let inside = ctx
            .store
            .create("PM lane", Some("inside"), Status::Running, "fixture")
            .unwrap();
        let outside = ctx
            .store
            .create("Outside", Some("outside"), Status::Ready, "fixture")
            .unwrap();
        let run: corral_core::Run = serde_json::from_value(json!({
            "id":"R1", "agent":"fixture", "worktree":root.join("app"),
            "branch":"main", "base":"main", "base_sha":"fixture",
            "started":jiff::Timestamp::now(), "delivery":"legacy",
            "herdr":{"workspace_id":text("workspace"),"pane_id":text("pane"),
                "socket":ctx.herdr.socket_path().unwrap()},
        }))
        .unwrap();
        ctx.store
            .update(&inside.id, |task| {
                task.runs.push(run);
                Ok(())
            })
            .unwrap();
        let target = crate::agent::BindTarget::Pm {
            project: "inside".into(),
        };
        let grant = crate::agent::open_generation(&ctx.store, &target, &text("pane")).unwrap();
        let line = json!({
            "store":paths::store_id(&ctx.store), "project":"inside", "task":inside.id,
            "outside_task":outside.id, "generation":grant.generation, "nonce":grant.bind_nonce,
            "sessions":paths::container_sessions_dir(&ctx.store, &grant.container),
        });
        println!("PM_LANE {line}");
    }
}
