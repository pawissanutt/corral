use super::*;

pub(super) fn probe(block_event_log: bool) {
    let fx = Fx::new();
    let panes = fx.review();
    let ws = fx.run().herdr.unwrap().workspace_id;
    fx.edit(|s| {
        s["fail"] = json!([format!("pane close {}", panes[0])]);
        if block_event_log {
            s["block_event_log"] = json!(true);
            s["event_fault_exe"] = json!(std::env::current_exe().unwrap());
        }
    });
    fx.reset_calls();
    assert_eq!(
        corral_ops::discard(&fx.ctx, &fx.id).unwrap().status,
        Status::Dropped
    );
    assert!(fx.pane_exists(&panes[0]));
    assert!(fx.workspace_exists(&ws));
    assert!(
        !fx.called(&["workspace", "close", &ws]),
        "failed pane close must protect workspace"
    );
    for pane in &panes[1..] {
        assert!(
            !fx.pane_exists(pane),
            "one failure must not stop other pane closes"
        );
    }
    if !block_event_log {
        assert!(fx.ctx.store.events().unwrap().iter().any(|e| {
            e.kind == "cleanup_failed"
                && e.task.as_deref() == Some(&fx.id)
                && e.detail["resource"] == panes[0]
                && e.detail["error"]
                    .as_str()
                    .unwrap()
                    .contains("injected-cleanup-error")
        }));
    } else {
        let logged = fx
            .ctx
            .store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "cleanup_failed" && e.detail["resource"] == panes[0]);
        assert!(!logged);
    }
    fx.edit(|s| {
        s["fail"] = json!([format!("workspace close {ws}")]);
        s.as_object_mut().unwrap().remove("block_event_log");
    });
    fx.sync();
    assert!(!fx.pane_exists(&panes[0]), "sync retries the failed pane");
    assert!(fx.workspace_exists(&ws));
    assert!(fx.ctx.store.events().unwrap().iter().any(|e| {
        e.kind == "cleanup_failed"
            && e.task.as_deref() == Some(&fx.id)
            && e.detail["resource"] == ws
            && e.detail["error"]
                .as_str()
                .unwrap()
                .contains("injected-cleanup-error")
    }));
    fx.edit(|s| s["fail"] = json!([]));
    fx.sync();
    assert!(
        !fx.workspace_exists(&ws),
        "sync retries the failed workspace"
    );
}
