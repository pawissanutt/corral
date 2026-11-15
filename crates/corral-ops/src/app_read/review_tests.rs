use super::*;
use corral_core::{Run, Status, Store};
use serde_json::json;

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "user.name=Corral Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .context("running fixture git")?;
    ensure!(
        output.status.success(),
        "fixture git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

#[test]
fn review_file_returns_the_immutable_candidate_that_produced_its_patch() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo)?;
    git(&repo, &["init", "-q", "-b", "main"])?;
    std::fs::write(repo.join("file"), "before\n")?;
    git(&repo, &["add", "file"])?;
    git(&repo, &["commit", "-qm", "base"])?;
    let base = git(&repo, &["rev-parse", "HEAD"])?;
    std::fs::write(repo.join("file"), "after\n")?;
    git(&repo, &["add", "file"])?;
    git(&repo, &["commit", "-qm", "candidate"])?;
    let head = git(&repo, &["rev-parse", "HEAD"])?;
    let store = Store::init(tmp.path())?;
    let mut task = store.create("candidate", None, Status::Running, "")?;
    let run: Run = serde_json::from_value(json!({
        "id":"R1", "agent":"fixture", "worktree":repo, "branch":"main", "base":"main",
        "base_sha":base, "started":"2026-11-14T00:00:00Z"
    }))?;
    task.runs.push(run);
    store.fixture_put_task(&task)?;
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let (body, next) = review_file(&ctx, &task.id, "file", 0)?;
    let DetailBody::ReviewFile(page) = body else {
        anyhow::bail!("wrong detail body")
    };
    assert!(next.is_none());
    assert!(page.patch.contains("-before\n+after\n"));
    assert_eq!(
        page.candidate,
        Some(ReviewCandidate {
            run: "R1".into(),
            base,
            head
        })
    );
    Ok(())
}

#[test]
fn review_keeps_canonical_gates_separate_and_bound_to_the_candidate() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo)?;
    git(&repo, &["init", "-q", "-b", "main"])?;
    std::fs::write(repo.join("file"), "before\n")?;
    git(&repo, &["add", "file"])?;
    git(&repo, &["commit", "-qm", "base"])?;
    let base = git(&repo, &["rev-parse", "HEAD"])?;
    std::fs::write(repo.join("file"), "after\n")?;
    git(&repo, &["add", "file"])?;
    git(&repo, &["commit", "-qm", "candidate"])?;
    let head = git(&repo, &["rev-parse", "HEAD"])?;
    let store = Store::init(tmp.path())?;
    let mut task = store.create("gate review", None, Status::Running, "")?;
    task.runs.push(serde_json::from_value(json!({
        "id":"R1", "agent":"fixture", "worktree":repo, "branch":"main", "base":"main",
        "base_sha":base, "started":"2026-11-14T00:00:00Z",
        "review": {
            "verdict":"approve", "reviewer":"owner", "sha":head,
            "at":"2026-11-14T00:01:00Z"
        }
    }))?);
    store.fixture_put_task(&task)?;
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let read = || -> Result<ReviewDetail> {
        let DetailBody::Review(detail) = review(&ctx, &task.id)? else {
            anyhow::bail!("wrong detail body")
        };
        Ok(detail)
    };
    let without_gate = read()?;
    assert!(without_gate.gates.is_empty());
    assert_eq!(without_gate.verdicts.len(), 1);
    task.runs[0].gate = Some(serde_json::from_value(json!({
        "result":"pass", "sha":head, "at":"2026-11-14T00:02:00Z",
        "commands":[{
            "command":"scripts/gate.sh", "exit_code":0, "log_start":0, "log_end":12
        }]
    }))?);
    ctx.store.fixture_put_task(&task)?;
    let current = read()?;
    assert_eq!(current.candidate_sha.as_deref(), Some(head.as_str()));
    assert_eq!(current.verdicts, without_gate.verdicts);
    assert_eq!(
        current.gates,
        vec![ReviewGate {
            by: "scripts/gate.sh".into(),
            sha: head.clone(),
            verdict: "pass".into(),
            at: Some("2026-11-14T00:02:00Z".into()),
            text: None,
            stale: false,
        }]
    );

    // A canonical pass remains visible after HEAD moves, but cannot appear fresh.
    git(&repo, &["commit", "--allow-empty", "-qm", "next candidate"])?;
    let next = git(&repo, &["rev-parse", "HEAD"])?;
    let moved = read()?;
    assert_eq!(moved.candidate_sha.as_deref(), Some(next.as_str()));
    assert_eq!(moved.gates[0].sha, head);
    assert_eq!(moved.gates[0].verdict, "pass");
    assert!(moved.gates[0].stale);
    assert!(moved.verdicts[0].stale);

    // Canonical stale diagnostics win even when the recorded SHA matches HEAD.
    let gate = task.runs[0].gate.as_mut().context("missing fixture gate")?;
    gate.sha = next;
    gate.result = corral_core::GateResult::Stale;
    gate.failed_cmd = Some("verification inputs changed".into());
    ctx.store.fixture_put_task(&task)?;
    let stale = read()?;
    assert_eq!(stale.gates[0].verdict, "stale");
    assert!(stale.gates[0].stale);
    assert_eq!(
        stale.gates[0].text.as_deref(),
        Some("verification inputs changed")
    );
    Ok(())
}
