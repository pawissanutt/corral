use corral_core::{Run, Status};
use serde_json::json;

use super::operating_fixture;

fn identity(task: &corral_core::Task) -> Vec<u8> {
    let mut value = serde_json::to_value(task).unwrap();
    value.as_object_mut().unwrap().remove("attention");
    serde_json::to_vec(&value).unwrap()
}

#[test]
fn real_attend_and_ledger_writes_preserve_a_scoped_executable_action() {
    // Given a scoped Ready task with a nonblocking question alongside its executable dispatch.
    let mut fx = operating_fixture::Fx::new();
    let task = fx.task("Executable with attention", Status::Ready);
    fx.ctx.store.update(&task.id, |task| {
        let run: Run = serde_json::from_value(json!({
            "id":"R0","agent":"fake","worktree":"/missing-attention-worktree",
            "branch":"old","base":"main","base_sha":"abc","started":super::at(0),
            "agent_state":"working",
            "agent_host":{
                "version":1,
                "binding":{"role":"worker","store":"store","project":"app","generation":1,"health":"healthy"},
                "questions":[{
                    "version":1,"key":format!("{}/R0/Q1", task.id),"revision":1,
                    "kind":"clarification","blocking":false,"text":"Which constraint applies?","state":"open",
                    "asked":{"request_id":"q1","payload_sha256":"sha","session_id":"session","generation":1,"at":super::at(0)},
                    "wake":{"route":"none","key":"q1","state":"pending"}
                }]
            }
        }))?;
        task.runs.push(run);
        Ok(())
    }).unwrap();
    fx.grant(&[&task.id]);
    let before = fx.ctx.store.get(&task.id).unwrap();
    let workset = corral_ops::pm_workset(&fx.fresh()).unwrap();
    let action = workset
        .actions
        .iter()
        .find(|action| action.action.task.as_deref() == Some(task.id.as_str()))
        .unwrap()
        .id
        .clone();
    let key = corral_ops::attention::board(&fx.ctx, super::at(0))
        .unwrap()
        .tasks
        .into_iter()
        .find(|row| row.task == task.id)
        .unwrap()
        .assessment
        .conditions
        .into_iter()
        .find_map(|condition| condition.key)
        .unwrap();
    // When the public attend operation commits, followed by real ledger level/clear writes.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "action_identity::authorized_handling_subprocess",
            "--nocapture",
        ])
        .env("ATTENTION_ATTEND_ROOT", fx.ctx.store.root())
        .env("ATTENTION_ATTEND_KEY", &key.0)
        .env("CORRAL_PM", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    fx.ctx
        .store
        .update_attention(&task.id, |ledger| {
            ledger.records.get_mut(&key).unwrap().levels.rewoke_at = Some(super::at(900));
            Ok(())
        })
        .unwrap();
    fx.ctx
        .store
        .update_attention(&task.id, |ledger| {
            ledger.outbox.clear();
            Ok(())
        })
        .unwrap();
    // Then both byte identity and the original executable ID survive.
    let after = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(after.updated, before.updated);
    assert_eq!(identity(&after), identity(&before));
    assert!(
        corral_ops::pm_workset(&fx.fresh())
            .unwrap()
            .actions
            .iter()
            .any(|row| row.id == action)
    );
    assert!(!matches!(
        corral_ops::pm_act(&fx.fresh(), &action).unwrap().outcome,
        corral_ops::PmActionOutcome::Stale
    ));
}

#[test]
fn authorized_handling_subprocess() {
    let Ok(root) = std::env::var("ATTENTION_ATTEND_ROOT") else {
        return;
    };
    let ctx =
        corral_ops::Ctx::from_store(corral_core::Store::open(std::path::Path::new(&root)).unwrap());
    corral_ops::attention::attend(
        &ctx,
        &std::env::var("ATTENTION_ATTEND_KEY").unwrap(),
        "Investigating",
        1800,
        "real-attend",
        super::at(1),
    )
    .unwrap();
}

#[test]
fn non_attention_comment_and_hold_writes_invalidate_real_action_ids() {
    for change in ["comment", "hold"] {
        // Given an executable scoped action.
        let mut fx = operating_fixture::Fx::new();
        let task = fx.task("Executable", Status::Ready);
        fx.grant(&[&task.id]);
        let action = corral_ops::pm_workset(&fx.fresh())
            .unwrap()
            .actions
            .into_iter()
            .find(|action| action.action.task.as_deref() == Some(task.id.as_str()))
            .unwrap()
            .id;
        // When any other task field changes through Store.
        fx.ctx
            .store
            .update(&task.id, |task| {
                match change {
                    "comment" => task.body.push_str("\nA changed contract."),
                    "hold" => task.dispatch_hold = true,
                    _ => unreachable!(),
                }
                Ok(())
            })
            .unwrap();
        // Then the original authority token is stale.
        assert!(matches!(
            corral_ops::pm_act(&fx.fresh(), &action).unwrap().outcome,
            corral_ops::PmActionOutcome::Stale
        ));
    }
}
