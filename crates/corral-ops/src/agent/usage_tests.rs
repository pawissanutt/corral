//! pi-usage: fixture totals on the run and review child, API-equivalent pricing, deduplication
//! across generations and collections, and PM sessions attributed to no task.
use std::path::{Path, PathBuf};

use corral_core::agent::{ContainerRef, paths};
use corral_core::{Run, RunOutcome, Status, Task, Usage};
use serde_json::Value;

use super::super::test_support::{self as ts, Fx, Host, fx};
use super::super::{BindTarget, CallResponse};
use super::*;

const FIXTURE: &str = include_str!("../../../../tools/pi-probe/fixtures/session-usage.jsonl");
const EXPECTED: &str =
    include_str!("../../../../tools/pi-probe/fixtures/session-usage.expected.json");

#[test]
fn transcript_pages_use_bound_worker_and_review_files() {
    let fx = fx();
    let (id, target) = worker(&fx);
    let host = bound_hello(&fx, &target, "p1", "sess-worker");
    write_session(
        &host.sessions.join("session.jsonl"),
        &[r#"{"type":"message","text":"worker only"}"#],
    );
    fx.ctx
        .store
        .update(&id, |t| {
            t.status = Status::Review;
            t.runs[0].outcome = Some(RunOutcome::Done);
            t.runs[0].ended = Some(jiff::Timestamp::now());
            t.runs[0].review_runs.push(ts::run("R1-review1", "p2"));
            Ok(())
        })
        .unwrap();
    let target = BindTarget::Reviewer {
        task: id.clone(),
        run: "R1".into(),
        review: "R1-review1".into(),
        callback: "g1:c1".into(),
    };
    let review = bound_hello(&fx, &target, "p2", "sess-review");
    write_session(
        &review.sessions.join("session.jsonl"),
        &[r#"{"type":"message","text":"review only"}"#],
    );
    let worker = crate::transcript_page(&fx.ctx.store, &id, "R1", None, 1, 200).unwrap();
    let child = crate::transcript_page(&fx.ctx.store, &id, "R1", Some(1), 1, 200).unwrap();
    assert!(worker.contains("worker only") && !worker.contains("review only"));
    assert!(child.contains("review only") && !child.contains("worker only"));
    std::fs::remove_file(review.sessions.join("session.jsonl")).unwrap();
    let error = crate::transcript_page(&fx.ctx.store, &id, "R1", Some(1), 1, 200).unwrap_err();
    assert!(format!("{error:#}").contains("transcript_unavailable"));
    assert!(format!("{error:#}").contains("session.jsonl"));
}

fn totals() -> [u64; 4] {
    let expected: Value = serde_json::from_str(EXPECTED).unwrap();
    let t = &expected["totals"];
    ["input", "output", "cacheRead", "cacheWrite"].map(|k| t[k].as_u64().unwrap())
}

fn tokens(usage: &Usage) -> [u64; 4] {
    [
        usage.input,
        usage.output,
        usage.cache_read,
        usage.cache_write,
    ]
}

/// The fixture's lines, and the indices of its assistant responses.
fn fixture_lines() -> (Vec<&'static str>, Vec<usize>) {
    let lines: Vec<&str> = FIXTURE.lines().collect();
    let responses = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("\"responseId\""))
        .map(|(i, _)| i)
        .collect();
    (lines, responses)
}

fn write_session(path: &Path, lines: &[&str]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
    )
    .unwrap();
}

fn bound_hello(fx: &Fx, target: &BindTarget, pane: &str, session: &str) -> Host {
    let host = ts::bind(fx, target, pane, session);
    assert!(matches!(ts::hello(fx, &host), CallResponse::Receipt { .. }));
    host
}

fn worker(fx: &Fx) -> (String, BindTarget) {
    let id = ts::worker_task(fx, "p1");
    let target = BindTarget::Worker {
        task: id.clone(),
        run: "R1".into(),
    };
    (id, target)
}

fn task(fx: &Fx, id: &str) -> Task {
    fx.ctx.store.get(id).unwrap()
}

#[test]
fn fixture_session_totals_reach_the_run_and_the_review_child() {
    // Given the bun-reproof fixture session bound to a worker run and to its review child:
    let fx = fx();
    let (id, target) = worker(&fx);
    let host = bound_hello(&fx, &target, "p1", "sess-worker");
    write_session(&host.sessions.join("session.jsonl"), &fixture_lines().0);
    fx.ctx
        .store
        .update(&id, |t| {
            t.status = Status::Review;
            t.runs[0].outcome = Some(RunOutcome::Done);
            t.runs[0].ended = Some(jiff::Timestamp::now());
            t.runs[0].review_runs.push(ts::run("R1-review1", "p2"));
            Ok(())
        })
        .unwrap();
    let review = BindTarget::Reviewer {
        task: id.clone(),
        run: "R1".into(),
        review: "R1-review1".into(),
        callback: "g1:c1".into(),
    };
    let reviewer = bound_hello(&fx, &review, "p2", "sess-review");
    write_session(&reviewer.sessions.join("session.jsonl"), &fixture_lines().0);
    // When the existing usage pipeline refreshes the task:
    crate::usage::refresh_usage(&fx.ctx.store, &id).unwrap();
    // Then the run and the child carry the fixture totals, as show --json renders them:
    let task = task(&fx, &id);
    let json = serde_json::to_value(&task).unwrap();
    for (run, shown) in [
        (&task.runs[0], &json["runs"][0]),
        (
            &task.runs[0].review_runs[0],
            &json["runs"][0]["review_runs"][0],
        ),
    ] {
        assert_eq!(run.usage_note, None, "{}", run.id);
        assert_eq!(tokens(run.usage.as_ref().unwrap()), totals(), "{}", run.id);
        let shown = ["input", "output", "cache_read", "cache_write"]
            .map(|k| shown["usage"][k].as_u64().unwrap());
        assert_eq!(shown, totals());
    }
    // and the existing readback includes both.
    let doubled = totals().map(|n| n * 2);
    assert_eq!(tokens(&task.total_usage().unwrap()), doubled);
}

#[test]
fn codex_models_price_at_api_equivalent_rates_and_unknown_models_stay_unpriced() {
    assert_eq!(pricing_key("openai-codex/gpt-6.1-sol"), "gpt-6.1-sol");
    assert_eq!(pricing_key("gpt-6.1-sol"), "gpt-6.1-sol");
    let fx = fx();
    let (id, target) = worker(&fx);
    let host = bound_hello(&fx, &target, "p1", "sess-price");
    let session = host.sessions.join("session.jsonl");
    std::fs::create_dir_all(&host.sessions).unwrap();
    let (lines, responses) = fixture_lines();
    let reported: f64 = responses
        .iter()
        .map(|&i| {
            let line: Value = serde_json::from_str(lines[i]).unwrap();
            line["message"]["usage"]["cost"]["total"].as_f64().unwrap()
        })
        .sum();
    let collected = |lines: &[String]| {
        std::fs::write(&session, lines.concat()).unwrap();
        let run = task(&fx, &id).runs[0].clone();
        collect(&run, &fx.ctx.store).unwrap()
    };
    // Given a response whose model carries the openai-codex provider prefix:
    let prefixed: Vec<String> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let l = if Some(&i) == responses.first() {
                l.replace(
                    "\"model\":\"gpt-6.1-sol\"",
                    "\"model\":\"openai-codex/gpt-6.1-sol\"",
                )
            } else {
                (*l).to_owned()
            };
            format!("{l}\n")
        })
        .collect();
    assert!(prefixed.concat().contains("openai-codex/gpt-6.1-sol\""));
    let usage = collected(&prefixed);
    // Then every response is priced from the catalog key with provenance; Pi's cost stays reported.
    let (price, source) = fx
        .ctx
        .store
        .config
        .price_with_source("gpt-6.1-sol")
        .unwrap();
    let [input, output, cache_read, cache_write] = totals().map(|n| n as f64);
    let api = (input * price.input
        + output * price.output
        + cache_read * price.cache_read()
        + cache_write * price.cache_write())
        / 1_000_000.0;
    assert!(
        (usage.estimated_usd.unwrap() - api).abs() < 1e-12,
        "{usage:?}"
    );
    assert!((usage.reported_usd.unwrap() - reported).abs() < 1e-12);
    assert_eq!(usage.unpriced_tokens, 0);
    for entry in &usage.entries {
        assert_eq!(entry.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(entry.price_source.as_deref(), Some(source));
    }
    // Given an unknown model: it adds to unpriced_tokens and estimated_usd is absent, not zero.
    let unknown: Vec<String> = lines
        .iter()
        .map(|l| format!("{}\n", l.replace("gpt-6.1-sol", "gpt-unpriced-9")))
        .collect();
    let usage = collected(&unknown);
    assert_eq!(usage.estimated_usd, None);
    assert_eq!(usage.unpriced_tokens, totals().iter().sum::<u64>());
    assert!((usage.reported_usd.unwrap() - reported).abs() < 1e-12);
}

#[test]
fn two_generations_collected_twice_and_after_resume_never_double_count() {
    let fx = fx();
    let (id, target) = worker(&fx);
    let (lines, responses) = fixture_lines();
    let after_second = responses[1] + 1;
    // Given generation 1 wrote the first two responses:
    let first = bound_hello(&fx, &target, "p1", "sess-resume");
    let file = first.sessions.join("session.jsonl");
    write_session(&file, &lines[..after_second]);
    // and generation 2 resumed the same session file and appended the third:
    let second = bound_hello(&fx, &target, "p1", "sess-resume");
    assert_eq!(second.identity.generation, first.identity.generation + 1);
    write_session(&file, &lines);
    // and a copy of the whole session (same session ID) is also bound, as a forked resume would:
    let copy: PathBuf = first.sessions.join("session-copy.jsonl");
    write_session(&copy, &lines);
    fx.ctx
        .store
        .update(&id, |t| {
            let state = t.runs[0].agent_host.as_mut().unwrap();
            let bound = state
                .generations
                .last_mut()
                .unwrap()
                .bound
                .as_mut()
                .unwrap();
            bound.session_file = copy.clone();
            Ok(())
        })
        .unwrap();
    let run: Run = task(&fx, &id).runs[0].clone();
    assert_eq!(run.agent_host.as_ref().unwrap().generations.len(), 2);
    // When it is collected twice and through the pipeline twice:
    let once = collect(&run, &fx.ctx.store).unwrap();
    let twice = collect(&run, &fx.ctx.store).unwrap();
    // Then each response counts once and repeated collection is identical.
    assert_eq!(once, twice);
    assert_eq!(once.entries.len(), responses.len());
    assert_eq!(tokens(&once), totals());
    crate::usage::refresh_usage(&fx.ctx.store, &id).unwrap();
    let stored = task(&fx, &id).runs[0].usage.clone();
    crate::usage::refresh_usage(&fx.ctx.store, &id).unwrap();
    assert_eq!(task(&fx, &id).runs[0].usage, stored);
    assert_eq!(tokens(stored.as_ref().unwrap()), totals());
}

#[test]
fn pm_sessions_are_attributed_to_no_task() {
    let fx = fx();
    // Given a PM generation whose session holds the fixture usage:
    let pm = bound_hello(
        &fx,
        &BindTarget::Pm {
            project: "corral".into(),
        },
        "pm1",
        "sess-pm",
    );
    let pm_dir = paths::container_sessions_dir(&fx.ctx.store, &ContainerRef::Pm);
    assert_eq!(pm.sessions, pm_dir);
    write_session(&pm_dir.join("session.jsonl"), &fixture_lines().0);
    // and a bound worker that has not answered yet:
    let (id, target) = worker(&fx);
    bound_hello(&fx, &target, "p1", "sess-idle");
    // When every task is refreshed:
    crate::usage::refresh_usage(&fx.ctx.store, &id).unwrap();
    // Then no task carries the PM's tokens.
    for task in fx.ctx.store.list().unwrap() {
        assert_eq!(task.total_usage(), None, "{}", task.id);
    }
    let run = &task(&fx, &id).runs[0];
    assert_eq!(
        run.usage_note.as_deref(),
        Some("no assistant usage found in the bound Pi sessions")
    );
}

#[test]
fn auxiliary_references_survive_serialization_and_share_response_deduplication() {
    use corral_core::agent::{UsageSession, UsageSessionKind};
    let fx = fx();
    let (id, target) = worker(&fx);
    let host = bound_hello(&fx, &target, "p1", "sess-parent");
    write_session(&host.sessions.join("session.jsonl"), &fixture_lines().0);
    let mut state = task(&fx, &id).runs[0].agent_host.clone().unwrap();
    let container = ContainerRef::Worker {
        task: id.clone(),
        run: "R1".into(),
    };
    for (key, kind, text) in [
        (
            "summary-copy",
            UsageSessionKind::Summary,
            FIXTURE.to_owned(),
        ),
        ("oracle", UsageSessionKind::Oracle, FIXTURE.to_owned()),
    ] {
        let file = paths::usage_session_file(&fx.ctx.store, &container, key);
        write_session(&file, &text.lines().collect::<Vec<_>>());
        state.usage_sessions.push(UsageSession {
            generation: 1,
            kind,
            session_id: key.into(),
            session_file: file,
            model: None,
            request: None,
            outcome: None,
        });
    }
    // Give the oracle a distinct canonical session identity, independent of fixture naming.
    let oracle = state.usage_sessions.last().unwrap();
    let mut lines: Vec<Value> = FIXTURE
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    lines[0]["id"] = "oracle-session".into();
    std::fs::write(
        &oracle.session_file,
        lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
    )
    .unwrap();
    let state: AgentRunState =
        serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
    let once = collect_state(&state, None, &fx.ctx.store).unwrap();
    assert_eq!(once, collect_state(&state, None, &fx.ctx.store).unwrap());
    assert_eq!(tokens(&once), totals().map(|n| n * 2));
    assert_eq!(state.usage_sessions[1].generation, 1);
    assert_eq!(state.usage_sessions[1].kind, UsageSessionKind::Oracle);
    // The references are canonical state, not a runtime directory scan.
    assert!(
        paths::usage_session_file(&fx.ctx.store, &container, "../../escape")
            .starts_with(paths::container_sessions_dir(&fx.ctx.store, &container))
    );
    let mut invalid = state;
    invalid.usage_sessions[0].generation = 99;
    assert!(collect_state(&invalid, None, &fx.ctx.store).is_err());
}

#[test]
fn container_readback_keeps_pm_and_planning_usage_separate() {
    use corral_core::agent::{AgentRole, pm_state};
    let fx = fx();
    let (id, target) = worker(&fx);
    let host = bound_hello(&fx, &target, "p1", "sess-worker");
    write_session(&host.sessions.join("session.jsonl"), &fixture_lines().0);
    let pm = bound_hello(
        &fx,
        &BindTarget::Pm {
            project: "corral".into(),
        },
        "pm1",
        "sess-pm",
    );
    write_session(&pm.sessions.join("session.jsonl"), &fixture_lines().0);
    let pm_usage = collect_container(&ContainerRef::Pm, &fx.ctx.store).unwrap();
    assert_eq!(tokens(&pm_usage), totals());
    assert_eq!(
        pm_usage,
        collect_container(&ContainerRef::Pm, &fx.ctx.store).unwrap()
    );
    let mut state = pm_state::load(&fx.ctx.store).unwrap().unwrap().state;
    // Persist actual planning containers, not implementation runs relabeled as planning.
    state.binding.role = AgentRole::Planner;
    let planner_file = paths::container_sessions_dir(
        &fx.ctx.store,
        &ContainerRef::Planner {
            task: id.clone(),
            run: "D1".into(),
        },
    )
    .join("session.jsonl");
    write_session(&planner_file, &fixture_lines().0);
    state.generations[0].bound.as_mut().unwrap().session_file = planner_file;
    let planner = serde_json::from_value(serde_json::json!({
        "agent_host": state, "id": "D1", "profile": "planner", "harness": "pi-native",
        "spec_revision": "fixture", "started": "2026-10-29T00:00:00Z",
        "output_dir": "/fixture/output", "rounds": 0, "state": "planning"
    }))
    .unwrap();
    state.binding.role = AgentRole::PlanReviewer;
    let review_file = paths::container_sessions_dir(
        &fx.ctx.store,
        &ContainerRef::PlanReviewer {
            task: id.clone(),
            plan: "P1".into(),
            review: "PR1".into(),
        },
    )
    .join("session.jsonl");
    write_session(&review_file, &fixture_lines().0);
    state.generations[0].bound.as_mut().unwrap().session_file = review_file;
    let plan = serde_json::from_value(serde_json::json!({
        "id": "P1", "spec_revision": "fixture",
        "artifact": {"run":"D1", "kind":"plan", "title":"fixture", "path":"fixture.md",
            "source":"/fixture.md", "sha256":"fixture", "bytes":0, "added":"2026-10-29T00:00:00Z"},
        "document": {"goal":"fixture", "non_goals":[], "decisions":[], "affected_paths":[],
            "contracts":[], "dependencies":[], "acceptance":[], "verification":[], "owner_decisions":[]},
        "reviews": [{"id":"PR1", "reviewer":"fixture", "policy_digest":"fixture",
            "at":"2026-10-29T00:00:00Z", "agent_host":state}]
    })).unwrap();
    fx.ctx
        .store
        .update(&id, |task| {
            task.plan_runs.push(planner);
            task.plans.push(plan);
            Ok(())
        })
        .unwrap();
    for container in [
        ContainerRef::Planner {
            task: id.clone(),
            run: "D1".into(),
        },
        ContainerRef::PlanReviewer {
            task: id.clone(),
            plan: "P1".into(),
            review: "PR1".into(),
        },
    ] {
        let first = collect_container(&container, &fx.ctx.store).unwrap();
        assert_eq!(tokens(&first), totals());
        assert_eq!(first, collect_container(&container, &fx.ctx.store).unwrap());
    }
    crate::usage::refresh_usage(&fx.ctx.store, &id).unwrap();
    assert_eq!(tokens(&task(&fx, &id).total_usage().unwrap()), totals());
}

#[cfg(unix)]
#[test]
fn a_session_that_is_not_a_regular_file_is_refused() {
    let fx = fx();
    let (id, target) = worker(&fx);
    let host = bound_hello(&fx, &target, "p1", "sess-link");
    let real = host.sessions.join("real.jsonl");
    write_session(&real, &fixture_lines().0);
    std::os::unix::fs::symlink(&real, host.sessions.join("session.jsonl")).unwrap();
    let run = task(&fx, &id).runs[0].clone();
    let error = collect(&run, &fx.ctx.store).unwrap_err();
    assert!(
        format!("{error:#}").contains("is not a regular file"),
        "{error:#}"
    );
}

#[test]
fn an_unterminated_last_line_is_not_yet_counted() {
    let fx = fx();
    let (id, target) = worker(&fx);
    let host = bound_hello(&fx, &target, "p1", "sess-partial");
    let (lines, responses) = fixture_lines();
    let last = *responses.last().unwrap();
    let mut text: String = lines[..last].iter().map(|l| format!("{l}\n")).collect();
    text.push_str(&lines[last][..lines[last].len() / 2]);
    std::fs::create_dir_all(&host.sessions).unwrap();
    std::fs::write(host.sessions.join("session.jsonl"), text).unwrap();
    let run = task(&fx, &id).runs[0].clone();
    let usage = collect(&run, &fx.ctx.store).unwrap();
    assert_eq!(usage.entries.len(), responses.len() - 1);
}

#[test]
fn bedrock_models_price_from_the_bedrock_catalog_with_provenance() {
    let fx = fx();
    let (id, target) = worker(&fx);
    let host = bound_hello(&fx, &target, "p1", "sess-bedrock");
    let session = host.sessions.join("session.jsonl");
    std::fs::create_dir_all(&host.sessions).unwrap();
    let model = "us.anthropic.claude-sonnet-4-5-20250929-v1:0";
    // Given the fixture session recorded against a Bedrock model ID:
    let lines: Vec<String> = FIXTURE
        .lines()
        .map(|l| format!("{}\n", l.replace("gpt-6.1-sol", model)))
        .collect();
    std::fs::write(&session, lines.concat()).unwrap();
    let run = task(&fx, &id).runs[0].clone();
    let usage = collect(&run, &fx.ctx.store).unwrap();
    // Then every response is priced at the Bedrock catalog rates, with Bedrock provenance.
    let (price, source) = fx.ctx.store.config.price_with_source(model).unwrap();
    assert_eq!(source, "catalog-bedrock@pi-ai-1.3.0");
    let [input, output, cache_read, cache_write] = totals().map(|n| n as f64);
    let api = (input * price.input
        + output * price.output
        + cache_read * price.cache_read()
        + cache_write * price.cache_write())
        / 1_000_000.0;
    assert!(
        (usage.estimated_usd.unwrap() - api).abs() < 1e-12,
        "{usage:?}"
    );
    assert_eq!(usage.unpriced_tokens, 0);
    for entry in &usage.entries {
        assert_eq!(entry.price_source.as_deref(), Some(source));
    }
}

#[test]
fn bedrock_million_token_session_prices_at_catalog_input_plus_output() {
    let fx = fx();
    let (id, target) = worker(&fx);
    let host = bound_hello(&fx, &target, "p1", "sess-bedrock-1m");
    let session = host.sessions.join("session.jsonl");
    std::fs::create_dir_all(&host.sessions).unwrap();
    let model = "us.anthropic.claude-sonnet-4-5-20250929-v1:0";
    // Given a session whose only response used exactly 1,000,000 input and output tokens:
    let (lines, responses) = fixture_lines();
    let mut kept: Vec<String> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if responses.contains(&i) && i != responses[0] {
            continue;
        }
        let mut line = l.replace("gpt-6.1-sol", model);
        if i == responses[0] {
            let mut v: Value = serde_json::from_str(&line).unwrap();
            v["message"]["usage"] = serde_json::json!({
                "input": 1_000_000, "output": 1_000_000, "cacheRead": 0, "cacheWrite": 0,
                "reasoning": 0, "totalTokens": 2_000_000,
                "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}
            });
            line = v.to_string();
        }
        kept.push(format!("{line}\n"));
    }
    std::fs::write(&session, kept.concat()).unwrap();
    let run = task(&fx, &id).runs[0].clone();
    let usage = collect(&run, &fx.ctx.store).unwrap();
    // Then the totals are exact, and the estimate is catalog input plus output with provenance.
    assert_eq!(tokens(&usage), [1_000_000, 1_000_000, 0, 0]);
    assert!(
        (usage.estimated_usd.unwrap() - 19.8).abs() < 1e-9,
        "{usage:?}"
    );
    assert_eq!(usage.unpriced_tokens, 0);
    assert!(!usage.entries.is_empty());
    for entry in &usage.entries {
        assert_eq!(
            entry.price_source.as_deref(),
            Some("catalog-bedrock@pi-ai-1.3.0")
        );
    }
}

#[test]
fn response_ids_cannot_collide_with_summary_entry_ids() {
    let fx = fx();
    let (id, target) = worker(&fx);
    let host = bound_hello(&fx, &target, "p1", "sess-collision");
    write_session(
        &host.sessions.join("session.jsonl"),
        &[
            r#"{"type":"message","id":"m1","message":{"role":"assistant","responseId":"entry:s1","model":"unknown-model","usage":{"input":10,"output":1}}}"#,
            r#"{"type":"compaction","id":"s1","usage":{"input":20,"output":2}}"#,
        ],
    );
    let usage = collect(&task(&fx, &id).runs[0], &fx.ctx.store).unwrap();
    assert_eq!(tokens(&usage), [30, 3, 0, 0]);
    assert_eq!(usage.unpriced_tokens, 33);
    assert_eq!(usage.estimated_usd, None);
}

#[test]
fn compaction_and_branch_summary_usage_counts_once_toward_the_run() {
    // Given the fixture session plus a compaction and a branch summary that recorded their own
    // request usage, one compaction without usage, and the summary lines repeated as a copy would:
    let fx = fx();
    let (id, target) = worker(&fx);
    let host = bound_hello(&fx, &target, "p1", "sess-worker");
    let compaction = r#"{"type":"compaction","id":"cmp1","parentId":"x","timestamp":"2026-10-29T00:00:01Z","summary":"s","firstKeptEntryId":"x","tokensBefore":9,"usage":{"input":700,"output":70,"cacheRead":7,"cacheWrite":0,"cost":{"total":0.01}}}"#;
    let branch = r#"{"type":"branch_summary","id":"bs1","parentId":"x","timestamp":"2026-10-29T00:00:02Z","fromId":"x","summary":"b","usage":{"input":300,"output":30,"cacheRead":0,"cacheWrite":3}}"#;
    let unpriced = r#"{"type":"compaction","id":"cmp0","parentId":"x","timestamp":"2026-10-29T00:00:00Z","summary":"s","firstKeptEntryId":"x","tokensBefore":9}"#;
    let mut lines = fixture_lines().0;
    lines.extend([unpriced, compaction, branch, compaction, branch]);
    write_session(&host.sessions.join("session.jsonl"), &lines);
    // When usage is collected twice:
    let run = task(&fx, &id).runs[0].clone();
    let first = collect(&run, &fx.ctx.store).unwrap();
    let second = collect(&run, &fx.ctx.store).unwrap();
    // Then each summary request is added exactly once to the assistant totals.
    let [input, output, cache_read, cache_write] = totals();
    assert_eq!(
        tokens(&first),
        [input + 1000, output + 100, cache_read + 7, cache_write + 3]
    );
    assert_eq!(tokens(&second), tokens(&first));
}
