use super::*;
use std::os::unix::fs::PermissionsExt;

pub(crate) struct Fixture {
    _dir: tempfile::TempDir,
    _data: crate::agent::test_support::PrivateDataRoot,
    pub(crate) ctx: Ctx,
    pub(crate) repo: PathBuf,
    pub(crate) worktree: PathBuf,
    pub(crate) base: String,
    pub(crate) head: String,
}

pub(crate) fn git(repo: &Path, args: &[&str]) -> String {
    let output = crate::test_support::command("git", repo)
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().into()
}

impl Fixture {
    pub(crate) fn new(commands: &[&str]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = crate::agent::test_support::private_data_root();
        let repo = dir.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-qb", "main"]);
        git(&repo, &["config", "user.name", "Fixture"]);
        git(&repo, &["config", "user.email", "fixture@example.invalid"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("source"), "base\n").unwrap();
        corral_git::commit_all(&repo, "base").unwrap();
        let base = corral_git::rev_sha(&repo, "HEAD").unwrap();
        let worktree = dir.path().join("candidate");
        corral_git::worktree_add(&repo, &worktree, "candidate", "main").unwrap();
        std::fs::write(worktree.join("source"), "candidate\n").unwrap();
        corral_git::commit_all(&worktree, "candidate").unwrap();
        let head = corral_git::rev_sha(&worktree, "HEAD").unwrap();
        let mut store = corral_core::Store::init(dir.path()).unwrap();
        store.config.projects.insert(
            "app".into(),
            serde_json::from_value(serde_json::json!({
                "path": repo, "base": "main",
            }))
            .unwrap(),
        );
        store.config.review.gate = Some(commands.iter().map(|c| (*c).into()).collect());
        let ctx = crate::resources::FixedResourceProbe::ctx(store);
        Self {
            _dir: dir,
            _data: data,
            ctx,
            repo,
            worktree,
            base,
            head,
        }
    }

    pub(crate) fn gate(&self) -> Result<GateReceipt> {
        run(
            &self.ctx,
            &GateRequest {
                session: "B-fixture",
                worktree: &self.worktree,
                project: "app",
                tier: "full",
                base: Some(&self.base),
            },
        )
    }
}

#[test]
fn configured_command_outcomes_are_persisted_in_unique_private_receipts() {
    let mut fixture = Fixture::new(&["printf out; printf err >&2"]);
    let first = fixture.gate().unwrap();
    assert_eq!(first.attempt, "G1");
    assert_eq!(first.result, "pass");
    let log = Path::new(first.log.as_ref().unwrap());
    let output = std::fs::read_to_string(log).unwrap();
    assert!(output.contains("outerr"));
    assert_eq!(log.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(
        Path::new(&first.receipt)
            .parent()
            .unwrap()
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        Path::new(&first.receipt)
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let saved: GateReceipt =
        serde_json::from_slice(&std::fs::read(&first.receipt).unwrap()).unwrap();
    assert_eq!(saved.head, fixture.head);
    fixture.ctx.store.config.review.gate = Some(vec!["false".into(), "touch must-not-run".into()]);
    let second = fixture.gate().unwrap();
    assert_eq!(second.attempt, "G2");
    assert_eq!(second.result, "fail");
    assert_eq!(second.failed_command.as_deref(), Some("false"));
    assert!(!fixture.worktree.join("must-not-run").exists());
    fixture.ctx.store.config.review.gate = Some(vec![]);
    let skipped = fixture.gate().unwrap();
    assert_eq!(skipped.attempt, "G3");
    assert_eq!(skipped.result, "skipped");
    assert_eq!(skipped.log, None);
    assert!(Path::new(&skipped.receipt).is_file());
    assert!(
        fixture
            .ctx
            .store
            .events()
            .unwrap()
            .iter()
            .all(|event| event.kind != "gate_started")
    );
}

#[test]
fn dirty_candidates_are_refused_before_an_attempt_is_created() {
    let fixture = Fixture::new(&["true"]);
    for file in ["untracked", "source"] {
        std::fs::write(fixture.worktree.join(file), "dirty\n").unwrap();
        assert!(
            fixture
                .gate()
                .unwrap_err()
                .to_string()
                .starts_with("worktree_dirty:")
        );
        assert!(
            !super::super::session_dir("B-fixture")
                .unwrap()
                .join("gates")
                .exists()
        );
        if file == "untracked" {
            std::fs::remove_file(fixture.worktree.join(file)).unwrap();
        }
    }
}

#[test]
fn successful_commands_cannot_pass_after_modifying_the_candidate() {
    for command in [
        "printf dirty >> source",
        "git -c user.name=Fixture -c user.email=fixture@example.invalid commit --allow-empty -qm moved",
    ] {
        let fixture = Fixture::new(&[command]);
        let receipt = fixture.gate().unwrap();
        assert_eq!(receipt.result, "fail");
        assert_eq!(
            receipt.failed_command.as_deref(),
            Some("gate modified the candidate")
        );
    }
}

#[test]
fn quick_gate_receives_configured_environment_and_base_relative_context() {
    let mut fixture = Fixture::new(&[]);
    let undefined = run(
        &fixture.ctx,
        &GateRequest {
            session: "A-fixture",
            worktree: &fixture.worktree,
            project: "app",
            tier: "quick",
            base: Some(&fixture.base),
        },
    )
    .unwrap_err();
    assert!(undefined.to_string().starts_with("gate_tier_undefined:"));
    fixture.ctx.store.config.review.gate_tiers = Some(std::collections::BTreeMap::from([
        ("quick".into(), vec!["test \"$BUILD_INPUT\" = configured && test \"$CORRAL_GATE_TIER\" = quick && test -n \"$CORRAL_INTEGRATION_BASE\" && test -z \"${AI_AGENT:-}${CORRAL_ASSISTANT:-}\" && cat \"$CORRAL_GATE_CONTEXT\"".into()]),
    ]));
    fixture.ctx.store.config.review.env = Some(std::collections::BTreeMap::from([(
        "BUILD_INPUT".into(),
        "configured".into(),
    )]));
    let receipt = run(
        &fixture.ctx,
        &GateRequest {
            session: "A-fixture",
            worktree: &fixture.worktree,
            project: "app",
            tier: "quick",
            base: Some(&fixture.base),
        },
    )
    .unwrap();
    assert_eq!(receipt.result, "pass");
    let context: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            super::super::session_dir("A-fixture")
                .unwrap()
                .join("gates/G1.context.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(context["integration_base"], fixture.base);
    assert_eq!(context["changed_paths"], serde_json::json!(["source"]));
}
