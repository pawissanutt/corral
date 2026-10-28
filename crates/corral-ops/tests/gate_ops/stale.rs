use super::*;

#[test]
fn main_moves_between_command_success_and_collection() {
    // The fake engine runs commands synchronously; collect only after main moves.
    let (tmp, ctx, id, _) = running_task(&["true"]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    let main = tmp.path().join("app");
    git(&main, &["commit", "--allow-empty", "-qm", "advance main"]);
    let gate = corral_ops::gate_collect(&ctx, &id).unwrap().unwrap();
    assert_eq!(gate.result, GateResult::Stale);
    assert!(gate.commands.iter().all(|r| r.exit_code == 0));
    let detail = gate.failed_cmd.as_deref().unwrap();
    assert!(detail.contains("integration_base"), "{detail}");
    assert!(detail.contains("rebase") && detail.contains("once"));
    let (title, log) = gate_log(&ctx, &id);
    assert_eq!(title, "Gate: stale");
    assert!(log.contains("corral gate: pass"));
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(
        serde_json::to_value(&task).unwrap()["runs"][0]["gate"]["result"],
        "stale"
    );
    ctx.store
        .update(&id, |t| {
            t.review_wait = Some(corral_core::ReviewWork::Gate);
            Ok(())
        })
        .unwrap();
    let queue = corral_ops::dispatch_queue(&ctx).unwrap();
    let entry = queue.iter().find(|entry| entry.id == id).unwrap();
    assert!(entry.reason.contains("gate stale") && entry.reason.contains("integration_base"));
    ctx.store
        .update(&id, |t| {
            t.status = Status::Review;
            Ok(())
        })
        .unwrap();
    let error = corral_ops::approve(&ctx, &id, corral_ops::ApproveStrategy::Rebase, false, true)
        .unwrap_err();
    assert!(format!("{error:#}").contains("gate"), "{error:#}");
    assert_eq!(corral_ops::gate_collect(&ctx, &id).unwrap().unwrap(), gate);
}

#[test]
fn command_failure_is_still_failure_when_main_also_moves() {
    let (tmp, ctx, id, _) = running_task(&["false"]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    git(
        &tmp.path().join("app"),
        &["commit", "--allow-empty", "-qm", "advance main"],
    );
    let gate = corral_ops::gate_collect(&ctx, &id).unwrap().unwrap();
    assert_eq!(gate.result, GateResult::Fail);
    assert_eq!(gate.failed_cmd.as_deref().unwrap().trim(), "false");
}
