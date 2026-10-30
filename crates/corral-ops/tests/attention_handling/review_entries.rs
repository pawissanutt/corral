use crate::attention::handling_tests::fixture::{Fx, at};
use corral_core::Status;
use serde_json::json;

#[test]
fn actual_legacy_and_native_review_entries_keep_distinct_single_notice_claims() {
    for producer in [
        "legacy_review_watchdog",
        "native_review_watchdog",
        "native_review_host_lost",
    ] {
        let fx = Fx::new();
        let task = fx.stalled(None);
        fx.ctx.store.update(&task.id, |task| {
            let parent = task.current_run_mut().unwrap();
            parent.review = Some(serde_json::from_value(json!({
                "reviewer":"fixture","sha":"abc","at":at(0),"pane":"worker:p1"
            }))?);
            if producer != "legacy_review_watchdog" {
                parent.review_runs.push(serde_json::from_value(json!({
                    "id":"R1-review1","agent":"fixture","harness":"pi-native",
                    "worktree":"/missing-fixture-worktree","branch":"corral/T",
                    "base":"main","base_sha":"abc","started":at(0),
                    "agent_host":{
                        "version":1,
                        "binding":{"role":"reviewer","store":"store","project":"app","generation":1,
                            "health":if producer == "native_review_host_lost" {"lost"} else {"healthy"}},
                        "generations":[{"n":1,"bind_nonce_sha256":"nonce","pane":"worker:p1",
                            "created":at(0),"nudged_at":at(1)}],
                        "turns":[
                            {"generation":1,"seq":1,"origin":{"kind":"bridge"},"ended":at(1),"class":"no_verdict"},
                            {"generation":1,"seq":2,"origin":{"kind":"bridge"},"ended":at(2),"class":"no_verdict"}
                        ]
                    }
                }))?);
            }
            Ok(())
        }).unwrap();
        let invoke = || {
            if producer == "legacy_review_watchdog" {
                let pane = corral_herdr::PaneInfo {
                    pane_id: "worker:p1".into(),
                    workspace_id: "worker".into(),
                    tab_id: "worker:t1".into(),
                    agent: None,
                    status: corral_herdr::AgentStatus::Idle,
                    title: None,
                    cwd: None,
                };
                crate::reviewer::reconcile_review_at(
                    &fx.ctx,
                    &task.id,
                    at(3),
                    &std::collections::HashMap::from([("worker:p1".into(), pane)]),
                )
                .unwrap()
                .is_some()
            } else if producer == "native_review_watchdog" {
                crate::agent::hooks::review::nudge(&fx.ctx, &task.id).unwrap()
            } else {
                crate::agent::hooks::review::hold_lost(&fx.ctx, &task.id).unwrap()
            }
        };
        assert!(invoke(), "{producer}");
        assert!(!invoke(), "{producer}");
        assert_eq!(fx.ctx.store.get(&task.id).unwrap().status, Status::NeedsMe);
        crate::attention::reconcile(&fx.reopen(), at(3), Some(true)).unwrap();
        crate::attention::reconcile(&fx.reopen(), at(7200), Some(true)).unwrap();
        assert_eq!(fx.calls(), 1, "{producer}");
        let events = fx.ctx.store.events().unwrap();
        let notices: Vec<_> = events
            .iter()
            .filter(|event| event.kind == "owner_notified")
            .collect();
        assert_eq!(notices.len(), 1, "{producer}");
        assert_eq!(notices[0].detail["producer"], producer);
    }
}
