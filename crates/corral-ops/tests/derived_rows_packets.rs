//! Approval-cache recovery through PM -> landing_blocker -> IntegrationPacket::build.
use std::path::Path;
use std::process::{Command, Stdio};

use corral_core::work::{self, Counter, WorkSnapshot};
use corral_core::{Project, Run, Status, Store};
use corral_ops::{Ctx, PmActionKind};
use serde_json::{Value, json};

mod support;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = support::command("git", repo)
        .env("GIT_MASTER", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn context(store: Store) -> Ctx {
    let mut ctx = corral_ops::resources::FixedResourceProbe::ctx(store);
    ctx.herdr = corral_herdr::Herdr::with_bin("/nonexistent/derived-rows-herdr".into(), None);
    ctx
}

fn fixture() -> (tempfile::TempDir, Ctx, String, Run) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("app");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("base.rs"), "fn base() {}\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(
        &repo,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "base"],
    );
    let initial = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-qb", "candidate"]);
    std::fs::write(repo.join("candidate.rs"), "fn candidate() {}\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(
        &repo,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "candidate"],
    );
    git(&repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("landed.rs"), "fn landed() {}\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(
        &repo,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "landed"],
    );
    let base = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "candidate"]);
    let mut store = Store::init(dir.path()).unwrap();
    store
        .fixture_pin_now("2026-10-01T00:00:00Z".parse().unwrap())
        .unwrap();
    store.config.projects.insert(
        "app".into(),
        Project {
            path: "app".into(),
            dispatch: Default::default(),
            default_profile: None,
            base: None,
            review: Default::default(),
            artifact_retention: None,
            post_land: vec![],
        },
    );
    store.save_config().unwrap();
    let ctx = context(store);
    let mut task = ctx
        .store
        .create("Candidate", Some("app"), Status::Review, "")
        .unwrap();
    let run: Run = serde_json::from_value(json!({
        "id":"R1","agent":"generic","worktree":repo,"branch":"candidate","base":"main",
        "base_sha":initial,"started":"2026-10-01T00:00:00Z","ended":"2026-10-02T00:00:00Z",
        "outcome":"done","summary":"alpha"
    }))
    .unwrap();
    task.runs.push(run.clone());
    task.created = "2026-10-01T00:00:00Z".parse().unwrap();
    task.updated = task.created;
    ctx.store.fixture_put_task(&task).unwrap();
    let landed = ctx
        .store
        .create("Landed", Some("app"), Status::Done, "")
        .unwrap();
    ctx.store
        .log_event(
            Some(&landed.id),
            "approved",
            json!({"sha":base,"previous_sha":initial,"strategy":"rebase","run":"R1"}),
        )
        .unwrap();
    (dir, ctx, task.id, run)
}

fn grow(ctx: &Ctx, run: &Run, count: usize) {
    for i in 0..count {
        let mut old = ctx
            .store
            .create("Unrelated", Some("app"), Status::Done, "")
            .unwrap();
        old.runs.extend((0..count).map(|n| {
            let mut run = run.clone();
            run.id = format!("R{n:04}");
            run
        }));
        ctx.store.fixture_put_task(&old).unwrap();
        ctx.store
            .log_event(
                Some(&old.id),
                "approved",
                json!({"sha":format!("unrelated-{i}"),"strategy":"rebase","run":"R1"}),
            )
            .unwrap();
    }
}

fn measure(ctx: &Ctx) -> (Value, WorkSnapshot) {
    let before = work::snapshot();
    let workset = corral_ops::pm_workset(ctx).unwrap();
    let counts = work::snapshot().since(&before);
    counts.check_identity().unwrap();
    assert_eq!(counts.unattributed_hydrations(), 0);
    assert_eq!(
        workset.next.kind,
        PmActionKind::StaleVerdict,
        "must reach the successful integration packet branch"
    );
    assert!(counts.get(Counter::RecordDecodes) > 0);
    (serde_json::to_value(workset.next).unwrap(), counts)
}

fn same(left: &WorkSnapshot, right: &WorkSnapshot) {
    for counter in Counter::ALL {
        assert_eq!(left.get(counter), right.get(counter), "{counter:?}");
    }
    assert_eq!(
        left.unattributed_byte_remainder(),
        right.unattributed_byte_remainder()
    );
}

#[test]
fn pm_packet_lookup_follows_new_approvals_and_recovers_without_history_growth() {
    // Given a real candidate missing a landed main commit and one recorded approval.
    let (_dir, ctx, id, run) = fixture();
    grow(&ctx, &run, 1);
    // Warm only via the real production entry (also establishes packet output).
    let (oracle, _) = measure(&ctx);
    let (_, base) = measure(&ctx);
    // When board history and unrelated approval history grow tenfold.
    grow(&ctx, &run, 9);
    let (grown, grown_counts) = measure(&ctx);
    assert_eq!(grown, oracle);
    same(&base, &grown_counts);
    // Then an actual current-run mutation and new approval event keep keyed lookup work fixed.
    ctx.store
        .update_head(&id, |head| {
            head.current_run_mut().unwrap().summary = Some("omega".into());
            Ok(())
        })
        .unwrap();
    let mut mutated = ctx.store.get(&id).unwrap();
    mutated.updated = "2026-10-01T00:00:00Z".parse().unwrap();
    ctx.store.fixture_put_task(&mutated).unwrap();
    ctx.store
        .log_event(
            None,
            "approved",
            json!({"sha":"new-unrelated","strategy":"rebase"}),
        )
        .unwrap();
    let (mutated, mutated_counts) = measure(&ctx);
    assert_eq!(mutated, oracle);
    same(&grown_counts, &mutated_counts);
    std::fs::remove_dir_all(ctx.store.cache_root().unwrap()).unwrap();
    let (recovered, recovered_counts) = measure(&ctx);
    assert_eq!(recovered, oracle);
    same(&mutated_counts, &recovered_counts);
}

fn child(root: &Path) -> Value {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "derived_packet_child", "--nocapture"])
        .env("CORRAL_DERIVED_PACKET_CHILD", root)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    serde_json::from_str(
        stdout
            .lines()
            .find_map(|line| line.strip_prefix("PACKET:"))
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn derived_packet_child() {
    let Some(root) = std::env::var_os("CORRAL_DERIVED_PACKET_CHILD") else {
        return;
    };
    let ctx = context(Store::discover(Path::new(&root)).unwrap());
    let (action, counts) = measure(&ctx);
    println!(
        "PACKET:{}",
        json!({"action":action,"cells":Counter::ALL.map(|counter|counts.get(counter))})
    );
}

#[test]
fn fresh_process_pm_packet_reads_recover_deleted_approval_cache() {
    // Given a packet-producing PM board and a warm Git memo established through production.
    let (dir, ctx, _id, run) = fixture();
    grow(&ctx, &run, 1);
    measure(&ctx);
    let expected = child(dir.path());
    // When approved-event and terminal-task history grow, then the derived cache is lost.
    grow(&ctx, &run, 9);
    assert_eq!(child(dir.path()), expected);
    std::fs::remove_dir_all(ctx.store.cache_root().unwrap()).unwrap();
    // Then a fresh process returns the same real PM action with identical hot cells.
    assert_eq!(child(dir.path()), expected);
}
