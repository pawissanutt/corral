//! Recipient::ReviewRun keys: `<task>_<run>_rv-<review>` round-trips and never collides with
//! PlanReview's three-part key; the other recipients' keys are unchanged.
use super::Recipient;

#[test]
fn review_run_keys_parse_unambiguously_and_leave_other_keys_unchanged() {
    let cases = [
        (
            Recipient::Run {
                task: "T-1".into(),
                run: "R1".into(),
            },
            "T-1_R1",
        ),
        (
            Recipient::PlanRun {
                task: "T-1".into(),
                plan_run: "P1".into(),
            },
            "T-1_plan-P1",
        ),
        (
            Recipient::PlanReview {
                task: "T-1".into(),
                plan: "P1".into(),
                review: "PR1".into(),
            },
            "T-1_P1_PR1",
        ),
        (
            Recipient::ReviewRun {
                task: "T-1".into(),
                run: "R1".into(),
                review: "R1-review1".into(),
            },
            "T-1_R1_rv-R1-review1",
        ),
        (Recipient::Pm, "pm"),
    ];
    for (recipient, key) in cases {
        assert_eq!(recipient.key().unwrap(), key);
        assert_eq!(Recipient::parse_key(key).unwrap(), recipient, "{key}");
    }
    // A plan review id shaped like a review-run key is refused rather than aliased.
    let alias = Recipient::PlanReview {
        task: "T-1".into(),
        plan: "R1".into(),
        review: "rv-x".into(),
    };
    assert!(alias.key().is_err());
}

#[test]
fn review_run_state_resolves_the_named_review_child() {
    let tmp = tempfile::tempdir().unwrap();
    let store = crate::Store::init(tmp.path()).unwrap();
    let task = store
        .create("Inbox", None, crate::Status::Review, "")
        .unwrap();
    let run = |id: &str| -> crate::Run {
        serde_json::from_value(serde_json::json!({
            "id": id, "agent": "pi", "worktree": "/tmp/w", "branch": "b", "base": "main",
            "base_sha": "abc", "started": "2026-10-23T00:00:00Z"
        }))
        .unwrap()
    };
    let mut task = store
        .update(&task.id, |t| {
            let mut parent = run("R1");
            parent.review_runs.push(run("R1-review1"));
            t.runs.push(parent);
            Ok(())
        })
        .unwrap();
    let child = Recipient::ReviewRun {
        task: task.id.clone(),
        run: "R1".into(),
        review: "R1-review1".into(),
    };
    let parent_seq = task.runs[0].inbox.next_seq;
    child.state_mut(&mut task).unwrap().next_seq = 7;
    assert_eq!(task.runs[0].review_runs[0].inbox.next_seq, 7);
    assert_eq!(task.runs[0].inbox.next_seq, parent_seq);
    let missing = Recipient::ReviewRun {
        task: task.id.clone(),
        run: "R1".into(),
        review: "R1-review2".into(),
    };
    assert!(missing.state_mut(&mut task).is_err());
}
