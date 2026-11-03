use super::*;

#[test]
fn focus_e2e_pm_and_owner_hold_clear_land_and_drop() {
    // The fixture is a private fake herdr protocol and disposable repositories, never owner panes.
    let mut fx = queue_fixture();
    fx.ctx.store.config.limits.running = 4;
    fx.ctx.store.config.review.gate = Some(vec!["true".into()]);
    fx.ctx.store.save_config().unwrap();
    let urgent = fx.task("app", Status::Ready);
    fx.ctx
        .store
        .update(&urgent.id, |t| {
            t.priority = "P0".into();
            Ok(())
        })
        .unwrap();
    let peer = fx.task("app", Status::Ready);
    let foreign = fx.task("other", Status::Review);
    let manual = fx.task("app", Status::Backlog);
    fx.ok(&["hold", &manual.id, "--reason", "manual maintenance"]);
    let pm_hold = fx
        .command(&["hold", &manual.id, "--reason", "manual maintenance"])
        .env("CORRAL_PM", "1")
        .output()
        .unwrap();
    assert!(pm_hold.status.success(), "{pm_hold:?}");
    let output = fx
        .command(&["focus", &urgent.id])
        .env("CORRAL_PM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fx.ctx.store.head(&peer.id).unwrap().hold.is_some());
    assert!(fx.ctx.store.head(&foreign.id).unwrap().hold.is_none());
    assert_eq!(fx.ctx.store.head(&peer.id).unwrap().status, Status::Ready);
    fx.ok(&["focus", "--clear"]);
    assert!(fx.ctx.store.head(&peer.id).unwrap().hold.is_none());
    assert!(fx.ctx.store.head(&manual.id).unwrap().hold.is_some());
    fx.ok(&["focus", &urgent.id, "--all"]);
    assert!(fx.ctx.store.head(&foreign.id).unwrap().hold.is_some());
    fx.ok(&["dispatch", &urgent.id]);
    fx.ok(&["gate", &urgent.id]);
    let script = corral_ops::gate_paths(&fx.ctx, &urgent.id, "R1").script;
    let ran = std::process::Command::new("bash")
        .arg(script)
        .env("CORRAL_ROOT", fx.temp.path())
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(ran.status.success(), "{ran:?}");
    corral_ops::gate_collect(&fx.ctx, &urgent.id).unwrap();
    fx.ok(&["done", &urgent.id, "--summary", "focus fixture"]);
    fx.ok(&["approve", &urgent.id]);
    fx.ok(&["sync"]);
    assert_eq!(fx.ctx.store.head(&urgent.id).unwrap().status, Status::Done);
    assert!(fx.ctx.store.head(&peer.id).unwrap().hold.is_none());
    assert!(fx.ctx.store.head(&foreign.id).unwrap().hold.is_none());
    assert!(fx.ctx.store.head(&manual.id).unwrap().hold.is_some());
    let second = fx.task("app", Status::Backlog);
    fx.ctx
        .store
        .update(&second.id, |t| {
            t.priority = "P0".into();
            Ok(())
        })
        .unwrap();
    fx.ok(&["focus", &second.id]);
    fx.ok(&["move", &second.id, "dropped"]);
    fx.ok(&["sync"]);
    assert!(fx.ctx.store.head(&peer.id).unwrap().hold.is_none());
    let output = fx
        .command(&["release", &manual.id])
        .env("CORRAL_PM", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(fx.ctx.store.head(&manual.id).unwrap().hold.is_none());
}
