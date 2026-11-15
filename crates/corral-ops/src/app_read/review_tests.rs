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
