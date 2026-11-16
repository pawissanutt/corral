use super::*;
use crate::agent::test_support as ts;
use serde_json::json;
use std::path::{Path, PathBuf};

struct Fixture {
    dir: tempfile::TempDir,
    _data: ts::PrivateDataRoot,
    ctx: Ctx,
    repo: PathBuf,
    scratch: corral_core::assistant::AssistantScratch,
    base: String,
    head: String,
}

fn git(repo: &Path, args: &[&str]) {
    let output = crate::test_support::command("git", repo)
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
}

impl Fixture {
    fn new(policy: BuildExit, command: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = ts::private_data_root();
        let repo = dir.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-qb", "main"]);
        git(&repo, &["config", "user.name", "Fixture"]);
        git(&repo, &["config", "user.email", "fixture@example.invalid"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("source"), "base\n").unwrap();
        corral_git::commit_all(&repo, "base").unwrap();
        let base = corral_git::rev_sha(&repo, "HEAD").unwrap();
        let mut store = corral_core::Store::init(dir.path()).unwrap();
        store.config.projects.insert(
            "app".into(),
            serde_json::from_value(json!({
                "path":repo,"base":"main","build_exit":policy,
            }))
            .unwrap(),
        );
        store.config.review.gate = Some(vec![command.into()]);
        store.create_assistant("fixture", None, None, None).unwrap();
        let ctx = crate::resources::FixedResourceProbe::ctx(store);
        let scratch = super::super::create(&ctx, "A-1", "app", Some("fix"), None).unwrap();
        std::fs::write(scratch.path.join("source"), "candidate\n").unwrap();
        corral_git::commit_all(&scratch.path, "candidate").unwrap();
        let head = corral_git::rev_sha(&scratch.path, "HEAD").unwrap();
        Self {
            dir,
            _data: data,
            ctx,
            repo,
            scratch,
            base,
            head,
        }
    }

    fn finish(&self, reason: Option<&str>) -> Result<FinishedHere> {
        finish_here(
            &self.ctx,
            "A-1",
            "fix",
            &FinishHere {
                title: "Fix source",
                expected_head: &self.head,
                override_reason: reason,
            },
        )
    }
}

#[test]
fn gate_policy_lands_records_and_retires_without_creating_board_work() {
    let f = Fixture::new(BuildExit::Gate, "true");
    let result = serde_json::to_value(f.finish(None).unwrap()).unwrap();
    assert_eq!(result["landed"], f.head);
    assert_eq!(result["base_branch"], "main");
    assert_eq!(result["gate_attempt"], "G1");
    assert_eq!(corral_git::rev_sha(&f.repo, "main").unwrap(), f.head);
    let records = f.ctx.store.list_build_records().unwrap();
    assert_eq!(records.len(), 1);
    let record = f.ctx.store.get_build_record("A-1-fix").unwrap();
    assert_eq!(record.kind, corral_core::build_record::BuildKind::Assistant);
    assert_eq!(record.commits.len(), 1);
    assert_eq!(record.verification[0].attempt, "G1");
    assert_eq!(record.usage.total.cost, None);
    assert!(f.ctx.store.list().unwrap().is_empty());
    let session = f.ctx.store.get_assistant("A-1").unwrap();
    assert!(session.scratch.is_empty());
    assert_eq!(session.linked_scratch, None);
    assert!(!f.scratch.path.exists());
    assert!(!super::super::git::branch_exists(&f.repo, &f.scratch.branch).unwrap());
}

#[test]
fn review_policy_requires_host_approval_or_records_the_owners_override() {
    let f = Fixture::new(BuildExit::Review, "true");
    assert!(
        f.finish(None)
            .err()
            .unwrap()
            .to_string()
            .starts_with("review_required:")
    );
    assert!(
        f.finish(Some("  "))
            .err()
            .unwrap()
            .to_string()
            .starts_with("review_required:")
    );
    assert_eq!(corral_git::rev_sha(&f.repo, "main").unwrap(), f.base);
    f.finish(Some("Owner accepts the missing independent review"))
        .unwrap();
    let record = f.ctx.store.get_build_record("A-1-fix").unwrap();
    assert_eq!(
        record.outcome.override_reason.as_deref(),
        Some("Owner accepts the missing independent review")
    );
}

#[test]
fn failed_gate_returns_the_receipt_and_leaves_the_scratch_in_place() {
    let f = Fixture::new(BuildExit::Gate, "false");
    let FinishedHere::GateFailed {
        landed,
        gate_attempt,
        gate,
    } = f.finish(None).unwrap()
    else {
        panic!("failed gate landed");
    };
    assert_eq!(landed, None);
    assert_eq!(gate_attempt, "G1");
    assert_eq!(gate.failed_command.as_deref(), Some("false"));
    assert!(Path::new(gate.log.as_ref().unwrap()).exists());
    assert_eq!(corral_git::rev_sha(&f.repo, "main").unwrap(), f.base);
    assert!(f.ctx.store.list_build_records().unwrap().is_empty());
    assert!(f.scratch.path.exists());
}

#[test]
fn moved_dirty_and_disabled_candidates_are_refused_without_landing() {
    let mut f = Fixture::new(BuildExit::Gate, "true");
    let old = f.head.clone();
    std::fs::write(f.scratch.path.join("next"), "next\n").unwrap();
    corral_git::commit_all(&f.scratch.path, "next").unwrap();
    assert!(
        f.finish(None)
            .err()
            .unwrap()
            .to_string()
            .starts_with("stale_candidate:")
    );
    f.head = corral_git::rev_sha(&f.scratch.path, "HEAD").unwrap();
    std::fs::write(f.scratch.path.join("dirty"), "untracked\n").unwrap();
    assert!(
        f.finish(None)
            .err()
            .unwrap()
            .to_string()
            .starts_with("scratch_dirty:")
    );
    std::fs::remove_file(f.scratch.path.join("dirty")).unwrap();
    f.ctx
        .store
        .config
        .projects
        .get_mut("app")
        .unwrap()
        .build_exit = Some(BuildExit::None);
    assert!(
        f.finish(None)
            .err()
            .unwrap()
            .to_string()
            .starts_with("in_session_finish_disabled:")
    );
    assert_ne!(old, f.head);
    assert_eq!(corral_git::rev_sha(&f.repo, "main").unwrap(), f.base);
    assert!(f.ctx.store.list_build_records().unwrap().is_empty());
}

#[test]
#[ignore = "entry point for the isolated marked owner-action child"]
fn marked_owner_action_child() {
    let root = PathBuf::from(std::env::var("CORRAL_FINISH_HERE_ROOT").unwrap());
    let _data = ts::private_data_root();
    let ctx = crate::resources::FixedResourceProbe::ctx(
        corral_core::Store::open_unobserved(&root).unwrap(),
    );
    let scratch = find(&ctx, "A-1", "fix").unwrap();
    let head = corral_git::rev_sha(&scratch.path, "HEAD").unwrap();
    assert_eq!(std::env::var("CORRAL_ASSISTANT").unwrap(), "A-1");
    // The CLI-facing path still refuses an agent marker, even in the same owner-action child.
    assert!(land::run(&ctx, "A-1-fix", &scratch.path, "app", &head, "G1").is_err());
    owner_action::claim_from_env(&root).unwrap().unwrap();
    let result = finish_here(
        &ctx,
        "A-1",
        "fix",
        &FinishHere {
            title: "Fix source",
            expected_head: &head,
            override_reason: None,
        },
    )
    .unwrap();
    assert_eq!(serde_json::to_value(result).unwrap()["landed"], head);
    assert!(land::run(&ctx, "A-1-fix", &scratch.path, "app", &head, "G1").is_err());
}

#[test]
fn claimed_owner_action_lands_with_agent_marker_but_direct_build_landing_stays_refused() {
    use corral_core::agent::{AGENT_RECORD_VERSION, ReceiptState, RequestReceipt};
    use corral_core::assistant::{NewOwnerInput, OwnerInputSource};
    let f = Fixture::new(BuildExit::Review, "true");
    f.ctx.store.save_config().unwrap();
    let grant = crate::agent::open_generation(
        &f.ctx.store,
        &crate::agent::BindTarget::Assistant {
            session: "A-1".into(),
        },
        "fixture",
    )
    .unwrap();
    let argv: Vec<String> = [
        "--exact",
        "assistant::scratch::finish_here::tests::marked_owner_action_child",
        "--ignored",
        "--nocapture",
        "--test-threads",
        "1",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let executor = crate::agent::effects::executor().unwrap();
    f.ctx.store.settle_assistant_container("A-1", |container, _| {
        let input = container.journal_owner_input("A-1", NewOwnerInput {
            generation: grant.generation, turn: None, source: OwnerInputSource::Pane,
            sha256: corral_core::agent::paths::sha256_hex(b"Finish it here"), text: "Finish it here".into(),
            truncated: false, command: None, answers: None, request: "owner-input".into(),
        });
        container.state.receipts.push(RequestReceipt {
            version: AGENT_RECORD_VERSION, request_id: "finish-here".into(), op: "owner_action".into(),
            payload_sha256: "fixture".into(), identity: serde_json::from_value(json!({
                "store":corral_core::agent::paths::store_id(&f.ctx.store),"project":"app","role":"assistant",
                "assistant":"A-1","session_id":"fixture","generation":grant.generation,
            })).unwrap(), state: ReceiptState::Reserved,
            result: json!({"kind":"finish_here","argv":argv,"input":input.id,"sha256":input.sha256,
                "confirmed":true,"finish_here_evidence":{"review":{
                    "reviewer":"review-1","model":"provider/reviewer","verdict":"approve",
                    "summary":"Approved","findings":[],"head":f.head,"at":corral_core::clock::now().to_string(),
                },"slices":[],"messages":[]}}),
            executor: Some(executor.clone()), effect: None, evidence: vec![], post_pending: vec![],
            at: corral_core::clock::now(), committed_at: None,
        });
        Ok(())
    }).unwrap();
    let output = crate::test_support::command(std::env::current_exe().unwrap(), f.dir.path())
        .args(&argv)
        .env("CORRAL_FINISH_HERE_ROOT", f.dir.path())
        .env("CORRAL_ASSISTANT", "A-1")
        .env("CORRAL_OWNER_ACTION", "A-1:finish-here")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(corral_git::rev_sha(&f.repo, "main").unwrap(), f.head);
    assert_eq!(f.ctx.store.list_build_records().unwrap().len(), 1);
    assert_eq!(
        f.ctx.store.get_build_record("A-1-fix").unwrap().intent[0].text,
        "Finish it here"
    );
    let record = f.ctx.store.get_build_record("A-1-fix").unwrap();
    assert_eq!(record.reviews[0].reviewer, "review-1");
    assert_eq!(record.reviews[0].head, f.head);
}
