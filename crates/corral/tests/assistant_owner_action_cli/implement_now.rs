//! T-653 implement-now: after the assistant reports a scratch's candidate, the owner's one
//! land-now answer finishes exactly that candidate for gate, independent review and automatic
//! landing; a scratch that moved after the report refuses `stale_candidate`; dropping it saves
//! the work and files nothing; and
//! `assistant show` tells whether the session runs the installed assets.
use std::path::PathBuf;

use corral_ops::agent::bridge::assets::asset_digest;
use serde_json::{Value, json};

use super::{Fixture, Host, git, text};

const SHELL: [(&str, &str); 1] = [("CORRAL_ASSISTANT", "A-1")];

/// The workspace as project corral's repository, and A-1's scratch cache-fix with one commit.
fn reported_scratch(fx: &Fixture) -> (PathBuf, String) {
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.name", "corral test"],
        &["config", "user.email", "corral@test.invalid"],
    ] {
        git(&fx.root, args);
    }
    std::fs::write(fx.root.join(".gitignore"), ".corral/\n.worktrees/\n").unwrap();
    git(&fx.root, &["add", ".gitignore"]);
    git(&fx.root, &["commit", "-q", "-m", "base"]);
    let new = [
        "assistant",
        "scratch",
        "new",
        "-p",
        "corral",
        "--slug",
        "cache-fix",
    ];
    fx.ok(&new, &SHELL);
    let scratch = fx.root.join(".worktrees/A-1-cache-fix");
    std::fs::write(scratch.join("cache.txt"), "fixed\n").unwrap();
    git(&scratch, &["add", "cache.txt"]);
    git(&scratch, &["commit", "-q", "-m", "Fix the cache"]);
    let head = git(&scratch, &["rev-parse", "HEAD"]);
    (scratch, head)
}

fn finish(head: &str, input: &str) -> Value {
    json!({"kind": "finish", "target": "cache-fix",
        "args": ["--title", "Fix the cache", "--expected-head", head], "input": input})
}

#[test]
fn one_land_now_answer_finishes_the_reported_candidate_and_approves_it() {
    // Given A-1 reported scratch cache-fix at its candidate, and the owner answered "land it now".
    let fx = Fixture::new();
    fx.configure_cowork_shipping();
    let (_, head) = reported_scratch(&fx);
    let mut host = Host::bind(&fx);
    let answer = host.journal("land it now", None);
    // The assistant's finish cites that answer and hands the candidate to gate and review.
    let finished = host.act("rq-finish", finish(&head[..12], &answer));
    assert_eq!(
        (
            finished["state"].as_str(),
            finished["result"]["outcome"].as_str()
        ),
        (Some("committed"), Some("executed")),
        "{finished}"
    );
    // Then T-002 holds one finished co-work run on the scratch: nothing is reimplemented.
    let task = fx.store().get("T-002").unwrap();
    assert_eq!(task.status, corral_core::Status::Review);
    assert_eq!(task.runs.len(), 1);
    assert_eq!(task.runs[0].branch, "corral/A-1/cache-fix");
    assert!(task.cowork.as_ref().unwrap().owner_confirmed);
    assert_eq!(
        task.cowork.as_ref().unwrap().review,
        corral_core::CoworkReview::Required
    );
    assert_eq!(
        task.runs[0].gate.as_ref().unwrap().result,
        corral_core::GateResult::Pass
    );
    assert_eq!(
        task.runs[0].review.as_ref().unwrap().reviewer,
        "cowork-reviewer"
    );
    // Once its independent reviewer approves, no second owner action is needed to land it.
    fx.ok(
        &[
            "verdict",
            "T-002",
            "approve",
            "--notes",
            "independent fixture review",
        ],
        &[],
    );
    fx.ok(&["sync"], &[]);
    assert_eq!(
        fx.store().get("T-002").unwrap().status,
        corral_core::Status::Done
    );
    assert_eq!(git(&fx.root, &["rev-parse", "main"]), head);
    let session = fx.store().get_assistant("A-1").unwrap();
    assert_eq!(session.landed[0].commits, vec![head.clone()]);
    // The finish carries the one owner answer; acceptance is automatic, not a second approval.
    let approved = fx
        .events()
        .into_iter()
        .find(|event| event["kind"] == "approved")
        .unwrap();
    assert_eq!(approved["actor"], "autopilot");
    let event = fx
        .events()
        .into_iter()
        .find(|event| event["kind"] == "cowork_finished")
        .unwrap();
    let authority = &event["detail"]["owner_authority"];
    assert_eq!(
        (
            authority["input"].as_str(),
            authority["owner_message"].as_str()
        ),
        (Some(answer.as_str()), Some("land it now"))
    );
}

#[test]
fn a_moved_scratch_refuses_the_stale_candidate_and_dropping_it_files_nothing() {
    // Given A-1 reported cache-fix at its candidate, then committed once more in it.
    let fx = Fixture::new();
    let (scratch, reported) = reported_scratch(&fx);
    std::fs::write(scratch.join("more.txt"), "more\n").unwrap();
    git(&scratch, &["add", "more.txt"]);
    git(&scratch, &["commit", "-q", "-m", "More"]);
    let moved = git(&scratch, &["rev-parse", "HEAD"]);
    let mut host = Host::bind(&fx);
    let answer = host.journal("land it now", None);
    // When the owner's land-now answer finishes the reported candidate, it refuses.
    let stale = host.act("rq-stale", finish(&reported, &answer));
    assert_eq!(stale["result"]["outcome"], "refused", "{stale}");
    assert!(stale.to_string().contains("stale_candidate"), "{stale}");
    // Then nothing was filed and the scratch is unchanged.
    assert_eq!(fx.store().list().unwrap().len(), 1);
    assert_eq!(git(&scratch, &["rev-parse", "HEAD"]), moved);
    let session = fx.store().get_assistant("A-1").unwrap();
    assert_eq!(session.scratch[0].kind, "spike");
    // When the owner then says to drop it, the assistant's shell discards it, saving its work.
    let out = fx.run(
        &["assistant", "scratch", "discard", "cache-fix", "--json"],
        &SHELL,
    );
    assert!(out.status.success(), "{}", text(&out));
    let removed: Value = serde_json::from_slice(&out.stdout).unwrap();
    let artifacts = removed["artifacts"].as_array().unwrap();
    assert!(
        artifacts
            .iter()
            .any(|a| a.as_str().is_some_and(|a| a.ends_with("commits.bundle"))),
        "{removed}"
    );
    assert!(!scratch.exists());
    assert_eq!(fx.store().list().unwrap().len(), 1);
    assert!(fx.store().get_assistant("A-1").unwrap().scratch.is_empty());
}

#[test]
fn show_tells_whether_the_session_runs_the_installed_assets() {
    // Given A-1's generation 1, opened before corral recorded its assets, and no installation.
    let fx = Fixture::new();
    let _host = Host::bind(&fx);
    let host = || -> Value {
        let shown: Value =
            serde_json::from_str(&fx.ok(&["assistant", "show", "A-1", "--json"], &[])).unwrap();
        shown["host"].clone()
    };
    let unknown = host();
    assert_eq!(
        [
            &unknown["generation"],
            &unknown["assets_sha256"],
            &unknown["installed_sha256"],
            &unknown["current"]
        ],
        [&json!(1), &Value::Null, &Value::Null, &Value::Null]
    );
    let kinds = unknown["owner_actions"].as_array().unwrap();
    assert!(kinds.contains(&json!("finish")), "{unknown}");
    // When generation 1 launched with the configured assets, it runs the current ones.
    let assets = fx.root.join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("package.json"), "{}\n").unwrap();
    let mut store = fx.store();
    store.config.agent.assets_dir = Some(assets.clone());
    store.save_config().unwrap();
    let digest = asset_digest(&assets).unwrap();
    store
        .update_assistant_container("A-1", |container| {
            let generation = container.state.generations.last_mut().unwrap();
            generation.assets_sha256 = Some(digest.clone());
            Ok(())
        })
        .unwrap();
    let current = host();
    assert_eq!(
        (&current["current"], &current["installed_sha256"]),
        (&json!(true), &json!(digest))
    );
    // When the assets change after it launched, it no longer runs the installed ones.
    std::fs::write(assets.join("package.json"), "{\"v\": 2}\n").unwrap();
    let stale = host();
    assert_eq!(
        (&stale["current"], &stale["assets_sha256"]),
        (&json!(false), &json!(digest))
    );
    assert_ne!(stale["installed_sha256"], json!(digest));
}
