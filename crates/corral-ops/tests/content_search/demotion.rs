use super::*;
use corral_core::work::{self, Counter, HotScope};

fn run(id: &str, summary: &str) -> Run {
    serde_json::from_value(json!({
        "id": id, "agent": "test", "worktree": "/tmp/test", "branch": "test",
        "base": "main", "base_sha": "abc", "started": "2026-01-01T00:00:00Z",
        "summary": summary
    }))
    .unwrap()
}

fn demotion(n: usize, cold: bool) -> Vec<(String, u64)> {
    corral_core::clock::pin_thread(Some("2026-01-01T00:00:00Z".parse().unwrap()));
    let mut f = Fixture::new();
    f.ctx
        .store
        .update(&f.task, |task| {
            let mut parent = run("R1", "parentmarker");
            for i in 0..n {
                parent
                    .review_runs
                    .push(run(&format!("H{i:04}"), &format!("childmarker{i:04}")));
            }
            task.runs.push(parent);
            Ok(())
        })
        .unwrap();
    corral_ops::refresh_search_index(&f.ctx).unwrap();
    f.ctx
        .store
        .update_head(&f.task, |head| {
            let parent = head.current_run_mut().unwrap();
            parent.summary = Some("changedparentmarker".into());
            parent.current_child.as_mut().unwrap().summary = Some("changedchildmarker".into());
            head.start_run(run("R2", "newrunmarker"))
        })
        .unwrap();
    if cold {
        f.ctx.store = Store::open(f.ctx.store.root()).unwrap();
    }
    let start = work::snapshot();
    {
        let _hot = HotScope::enter();
        corral_ops::refresh_search_index(&f.ctx).unwrap();
    }
    let delta = work::snapshot().since(&start);
    let before = f.cache()["documents"].clone();
    search_content(
        &f.ctx,
        "marker",
        &ContentSearch {
            scope: "reviews".into(),
            reindex: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(before, f.cache()["documents"]);
    Counter::ALL
        .iter()
        .map(|c| (c.name().to_owned(), delta.cells(*c).unattributed))
        .collect()
}

#[test]
fn demoted_run_search_matches_full_rebuild_and_work_is_bounded() {
    if let Ok(n) = std::env::var("CORRAL_DEMOTION_TEST_SIZE") {
        let cells = demotion(n.parse().unwrap(), true);
        println!("CELLS={}", serde_json::to_string(&cells).unwrap());
        return;
    }
    assert_eq!(demotion(10, false), demotion(100, false), "warm");
    let cold = |n: &str| {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "demotion::demoted_run_search_matches_full_rebuild_and_work_is_bounded",
                "--nocapture",
            ])
            .env("CORRAL_DEMOTION_TEST_SIZE", n)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let line = stdout.lines().find(|line| line.contains("CELLS=")).unwrap();
        line.split_once("CELLS=").unwrap().1.to_owned()
    };
    assert_eq!(cold("10"), cold("100"), "cold processes");
}
