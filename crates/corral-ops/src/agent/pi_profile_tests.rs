//! Named profile boundary regressions: disposable engines, no provider requests or logins.
use super::*;
use crate::agent::bridge::fixture::{self as f, Fx};
use crate::agent::hooks::pm::tests::fixture;
use crate::agent::launch;
use crate::agent::ops::assistant as ops;
use crate::availability::continuation;
use corral_core::agent::{BudgetRequest, GenerationBound, HostHealth, pm_state};
use corral_core::connection::ConnectionId;
use serde_json::json;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;

const PANE: &str = "w1:p9";

fn configured() -> Fx {
    let mut fx = fixture();
    std::fs::write(fx.root().join(PANE), "").unwrap();
    let assets = fx.root().join("assets");
    std::fs::create_dir_all(assets.join("src/roles/assistant")).unwrap();
    std::fs::write(assets.join("src/roles/assistant/index.ts"), "export {}").unwrap();
    std::fs::write(
        assets.join("compat.json"),
        json!({
            "verdict":"GO", "memory_projection":1, "providers":["openai"],
            "connections":["openai-subscription","openai-api"],
        })
        .to_string(),
    )
    .unwrap();
    let config = &mut fx.ctx.store.config;
    config.profiles.insert(
        "selected-sub".into(),
        serde_json::from_value(json!({
            "harness":"corral-pi", "vendor":"openai", "model":"gpt-fixture", "effort":"low",
            "connection":"openai-subscription",
        }))
        .unwrap(),
    );
    config.profiles.insert("selected-api".into(), serde_json::from_value(json!({
        "harness":"corral-pi", "vendor":"openai", "model":"gpt-fixture", "effort":"high",
        "connection":"openai-api",
        "metadata": {
            "display_name":"Selected API", "purpose":"fixture", "instructions":"fixture",
            "guidance":{}, "revision":"fixture", "evidence":null,
            "eligibility":{"enabled":true,"automatic":false,"roles":["implementation","pm"]},
            "candidates":[{"id":"api","harness":"corral-pi","connection":"openai-api",
                "provider":"openai","model":"gpt-fixture","effort":"high","args":[],
                "evidence":"fixture", "cost":{"kind":"metered","currency":"USD","maximum_micros":50000000}}],
            "fallback":null,
            "api_start":{"roles":["assistant","pm"],"spending":{"currency":"USD","maximum_micros":50000000},"approval":"fixture-owner"}
        }
    })).unwrap());
    fx.ctx.store.save_config().unwrap();
    fx
}

fn status(code: i32) -> std::process::ExitStatus {
    std::process::ExitStatus::from_raw(code << 8)
}

fn bootstrap(prepared: &Prepared) -> launch::Bootstrap {
    serde_json::from_slice(&std::fs::read(&prepared.bootstrap).unwrap()).unwrap()
}

fn bind(ctx: &Ctx, reference: &ContainerRef, file: &Path) {
    continuation::with_state(ctx, reference, |state| {
        let generation = state.generations.last_mut().unwrap();
        generation.bound = Some(GenerationBound {
            host_cap_sha256: "host".into(),
            ctl_cap_sha256: "ctl".into(),
            session_id: "fixture-session".into(),
            session_file: file.into(),
            pid: u32::MAX,
            start_time: "gone".into(),
            request_id: format!("hello-{}", generation.n),
            at: corral_core::clock::now(),
            herdr_seq_epoch: None,
        });
        state.binding.health = HostHealth::Healthy;
        Ok(())
    })
    .unwrap();
}

fn request(
    prepared: &Prepared,
    session: Option<&str>,
    generation: u32,
    profile: &str,
    confirmed: bool,
) {
    let mut body =
        json!({"version":1,"generation":generation,"profile":profile,"new_session":confirmed});
    let catalog: serde_json::Value = serde_json::from_slice(
        &std::fs::read(prepared.bootstrap.parent().unwrap().join(CATALOG_FILE)).unwrap(),
    )
    .unwrap();
    let row = catalog["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == profile)
        .unwrap();
    body["binding_digest"] = row["binding_digest"].clone();
    body["policy_digest"] = row["policy_digest"].clone();
    if let Some(session) = session {
        body["session"] = json!(session);
    }
    std::fs::write(
        prepared.bootstrap.parent().unwrap().join(REQUEST_FILE),
        body.to_string(),
    )
    .unwrap();
}

fn open(
    ctx: &Ctx,
    profile: &str,
) -> (
    corral_core::assistant::AssistantSession,
    Prepared,
    ops::Claim,
) {
    let (session, prepared, _, claim) = ops::open_claimed(
        ctx,
        ctx.store.root(),
        PANE,
        &Overrides {
            profile: Some(profile.into()),
            ..Default::default()
        },
        false,
        None,
    )
    .unwrap();
    (session, prepared, claim)
}

#[test]
fn confirmed_profile_change_opens_fresh_paid_session_and_paid_resume_never_refreshes_allowance() {
    let fx = configured();
    let ctx = f::ctx_at(fx.root());
    let default = ctx.store.config.pm.agent.clone();
    let (source, prepared, claim) = open(&ctx, "selected-sub");
    let source_file = fx.root().join("source.jsonl");
    let paid_file = fx.root().join("paid.jsonl");
    std::fs::write(&source_file, "{\"type\":\"session\"}\n").unwrap();
    std::fs::write(&paid_file, "{\"type\":\"session\"}\n").unwrap();
    let mut runs = 0;
    let mut episode_id = None;
    let mut paid_id = None;
    super::super::supervise_with(
        &ctx,
        PANE,
        claim,
        prepared,
        &Overrides {
            profile: Some("selected-sub".into()),
            ..Default::default()
        },
        false,
        |prepared| {
            runs += 1;
            let boot = bootstrap(prepared);
            let id = boot.assistant.as_deref().unwrap();
            let reference = ContainerRef::Assistant { session: id.into() };
            if runs == 1 {
                assert_eq!(
                    boot.binding.as_ref().unwrap().connection,
                    ConnectionId::OpenaiSubscription
                );
                bind(&ctx, &reference, &source_file);
                request(prepared, Some(id), boot.generation, "selected-api", true);
                return Ok(status(PROFILE_EXIT));
            }
            assert_ne!(id, source.id);
            paid_id = Some(id.to_owned());
            assert_eq!(
                boot.binding.as_ref().unwrap().connection,
                ConnectionId::OpenaiApi
            );
            assert_eq!(boot.effort.as_deref(), Some("high"));
            if runs == 2 {
                assert!(boot.resume_session.is_none());
            } else {
                assert_eq!(boot.resume_session.as_deref(), Some(paid_file.as_path()));
            }
            bind(&ctx, &reference, &paid_file);
            continuation::activate(&ctx, &reference, boot.generation).unwrap();
            let intent = continuation::intent_for(&ctx.store, &reference, boot.generation)
                .unwrap()
                .unwrap();
            let mut episode = ctx.store.paid_episode(&intent.link.episode).unwrap();
            assert_eq!(episode.approval.allowance_nanos, 50_000_000_000);
            if runs == 2 {
                episode_id = Some(episode.id.clone());
                assert!(
                    episode
                        .reserve(
                            &intent.link,
                            &intent.successor,
                            BudgetRequest {
                                connection: Some(ConnectionId::OpenaiApi),
                                request: "fixture-spend".into(),
                                generation: boot.generation,
                                tokens: 1,
                                usd_nanos: 12345,
                                priced: true,
                                unbounded: false,
                                response: None,
                            }
                        )
                        .unwrap()
                );
                episode
                    .settle(
                        &intent.link,
                        &intent.successor,
                        "fixture-spend",
                        "0".repeat(64),
                        1,
                        12345,
                    )
                    .unwrap();
                continuation::with_state(&ctx, &reference, |state| {
                    state.budget = episode.ledger.clone();
                    ctx.store.save_paid_episode(&episode)
                })
                .unwrap();
                request(prepared, Some(id), boot.generation, "selected-api", false);
            } else {
                assert_eq!(Some(&episode.id), episode_id.as_ref());
                assert_eq!(episode.ledger.totals().1, 12345);
                if runs == 3 {
                    request(prepared, Some(id), boot.generation, "selected-sub", true);
                } else {
                    let notice = std::fs::read_to_string(
                        prepared
                            .bootstrap
                            .parent()
                            .unwrap()
                            .join(ops::SWITCH_NOTICE_FILE),
                    )
                    .unwrap();
                    assert!(notice.contains("paid route is sticky"), "{notice}");
                    return Ok(status(0));
                }
            }
            Ok(status(PROFILE_EXIT))
        },
    )
    .unwrap();
    assert_eq!(runs, 4);
    assert_eq!(
        ctx.store
            .config_layers()
            .unwrap()
            .config()
            .unwrap()
            .pm
            .agent,
        default
    );
    assert_eq!(
        ctx.store.get_assistant(&source.id).unwrap().profile,
        "selected-sub"
    );
    assert_eq!(
        std::fs::read_to_string(&source_file).unwrap(),
        "{\"type\":\"session\"}\n"
    );
    let paid_id = paid_id.unwrap();
    let before = ctx.store.get_assistant_container(&paid_id).unwrap();
    let episode = ctx.store.paid_episode(&episode_id.unwrap()).unwrap();
    let error = ops::resume(
        &ctx,
        &paid_id,
        PANE,
        &Overrides {
            profile: Some("selected-sub".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("paid route is sticky"),
        "{error:#}"
    );
    assert_eq!(ctx.store.get_assistant_container(&paid_id).unwrap(), before);
    assert_eq!(ctx.store.paid_episode(&episode.id).unwrap(), episode);
    assert_eq!(
        ctx.store
            .record_rows(corral_core::paid_episode::AREA)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn refused_unconfirmed_stale_and_busy_requests_keep_the_original_profile_and_transcript() {
    for case in ["unconfirmed", "stale", "busy", "policy", "binding"] {
        let mut fx = configured();
        let ctx = f::ctx_at(fx.root());
        let (source, prepared, claim) = open(&ctx, "selected-sub");
        let file = fx.root().join("source.jsonl");
        std::fs::write(&file, "{\"type\":\"session\"}\n").unwrap();
        let mut runs = 0;
        super::super::supervise_with(
            &ctx,
            PANE,
            claim,
            prepared,
            &Overrides {
                profile: Some("selected-sub".into()),
                ..Default::default()
            },
            false,
            |prepared| {
                runs += 1;
                let boot = bootstrap(prepared);
                assert_eq!(boot.assistant.as_deref(), Some(source.id.as_str()));
                bind(
                    &ctx,
                    &ContainerRef::Assistant {
                        session: source.id.clone(),
                    },
                    &file,
                );
                if runs == 1 {
                    if case == "busy" {
                        ctx.store
                            .update_assistant_container(&source.id, |container| {
                                container.state.turns.push(serde_json::from_value(json!({
                                    "generation":boot.generation,"seq":1,"origin":{"kind":"bridge"},
                                    "started":corral_core::clock::now()
                                }))?);
                                Ok(())
                            })
                            .unwrap();
                    }
                    request(
                        prepared,
                        Some(&source.id),
                        if case == "stale" { 0 } else { boot.generation },
                        "selected-api",
                        case != "unconfirmed",
                    );
                    if case == "policy" {
                        fx.ctx
                            .store
                            .config
                            .profiles
                            .get_mut("selected-api")
                            .unwrap()
                            .metadata
                            .as_mut()
                            .unwrap()
                            .api_start
                            .as_mut()
                            .unwrap()
                            .spending
                            .maximum_micros *= 2;
                        fx.ctx.store.save_config().unwrap();
                    }
                    if case == "binding" {
                        let path = prepared.bootstrap.parent().unwrap().join(REQUEST_FILE);
                        let mut body: serde_json::Value =
                            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                        body["binding_digest"] = json!("stale-binding");
                        std::fs::write(path, body.to_string()).unwrap();
                    }
                    Ok(status(PROFILE_EXIT))
                } else {
                    assert_eq!(boot.resume_session.as_deref(), Some(file.as_path()));
                    let notice = std::fs::read_to_string(
                        prepared
                            .bootstrap
                            .parent()
                            .unwrap()
                            .join(ops::SWITCH_NOTICE_FILE),
                    )
                    .unwrap();
                    assert!(notice.contains("did not switch"), "{notice}");
                    Ok(status(0))
                }
            },
        )
        .unwrap();
        assert_eq!(runs, 2);
        assert_eq!(
            ctx.store.get_assistant(&source.id).unwrap().profile,
            "selected-sub"
        );
        assert!(!ctx.store.assistant_exists("A-2").unwrap());
        assert!(
            ctx.store
                .record_rows(corral_core::paid_episode::AREA)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn named_pm_launch_and_fresh_profile_switch_ignore_automation_pause_without_relabeling_transcript()
{
    let mut fx = configured();
    fx.ctx.store.config.dispatch.paused = true;
    fx.ctx.store.save_config().unwrap();
    let ctx = f::ctx_at(fx.root());
    let prepared = super::super::adopt(
        &ctx,
        PANE,
        &Overrides {
            profile: Some("selected-sub".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let file = fx.root().join("pm-source.jsonl");
    std::fs::write(&file, "{\"type\":\"session\"}\n").unwrap();
    let mut runs = 0;
    supervise_pm(&ctx, PANE, prepared, |prepared| {
        runs += 1;
        let boot = bootstrap(prepared);
        assert_eq!(boot.role, AgentRole::Pm);
        if runs == 1 {
            assert_eq!(boot.effort.as_deref(), Some("low"));
            bind(&ctx, &ContainerRef::Pm, &file);
            request(prepared, None, boot.generation, "selected-api", true);
            Ok(status(PROFILE_EXIT))
        } else {
            assert!(boot.resume_session.is_none());
            assert_eq!(
                boot.binding.as_ref().unwrap().connection,
                ConnectionId::OpenaiApi
            );
            assert_eq!(boot.effort.as_deref(), Some("high"));
            assert_eq!(pm_current(&ctx).unwrap(), "selected-api");
            bind(&ctx, &ContainerRef::Pm, &fx.root().join("pm-paid.jsonl"));
            continuation::activate(&ctx, &ContainerRef::Pm, boot.generation).unwrap();
            Ok(status(0))
        }
    })
    .unwrap();
    assert_eq!(runs, 2);
    assert_eq!(
        std::fs::read_to_string(file).unwrap(),
        "{\"type\":\"session\"}\n"
    );
    let pm = pm_state::load_current(&ctx.store).unwrap().unwrap();
    assert!(pm.adopted());
    assert!(
        ctx.store
            .config_layers()
            .unwrap()
            .config()
            .unwrap()
            .dispatch
            .paused
    );
    let resolved = crate::pm::continuation::recovery_profile(&ctx, &pm.state).unwrap();
    assert_eq!(resolved.profile, "selected-api");
}

#[test]
fn failed_paid_profile_start_stays_resumable_and_never_falls_back_to_source() {
    let fx = configured();
    let ctx = f::ctx_at(fx.root());
    let (source, prepared, claim) = open(&ctx, "selected-sub");
    let file = fx.root().join("source.jsonl");
    std::fs::write(&file, "{\"type\":\"session\"}\n").unwrap();
    let mut runs = 0;
    let error = super::super::supervise_with(
        &ctx,
        PANE,
        claim,
        prepared,
        &Overrides {
            profile: Some("selected-sub".into()),
            ..Default::default()
        },
        false,
        |prepared| {
            runs += 1;
            let boot = bootstrap(prepared);
            if runs == 1 {
                bind(
                    &ctx,
                    &ContainerRef::Assistant {
                        session: source.id.clone(),
                    },
                    &file,
                );
                request(
                    prepared,
                    Some(&source.id),
                    boot.generation,
                    "selected-api",
                    true,
                );
                Ok(status(PROFILE_EXIT))
            } else {
                assert_ne!(boot.assistant.as_deref(), Some(source.id.as_str()));
                anyhow::bail!("fixture host could not start")
            }
        },
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("fixture host could not start"),
        "{error:#}"
    );
    assert_eq!(runs, 2);
    let paid = ctx.store.get_assistant("A-2").unwrap();
    assert_eq!(paid.profile, "selected-api");
    assert!(paid.ended.is_none());
    assert!(ctx.store.get_assistant(&source.id).unwrap().ended.is_none());
    let reference = ContainerRef::Assistant { session: paid.id };
    let intent = continuation::intent_for(&ctx.store, &reference, 1)
        .unwrap()
        .unwrap();
    assert_eq!(intent.phase, continuation::IntentPhase::Paused);
    assert_eq!(
        ctx.store
            .record_rows(corral_core::paid_episode::AREA)
            .unwrap()
            .len(),
        1
    );
}
