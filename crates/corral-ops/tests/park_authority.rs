#![cfg(unix)]

use corral_core::{
    AttentionHandling, OperatingActionKind as Action, OperatingAuthority, OperatingSelection, Park,
    Responsible, Status,
};
use corral_ops::attention;
use jiff::{SignedDuration, Timestamp};
use serde_json::json;
use sha2::{Digest, Sha256};

#[path = "operating_launch/fixture.rs"]
mod fixture;

fn authority(fx: &fixture::Fx, id: &str, now: Timestamp) -> corral_core::Condition {
    attention::board(&fx.fresh(), now)
        .unwrap()
        .tasks
        .into_iter()
        .find(|row| row.task == id)
        .unwrap()
        .assessment
        .conditions
        .into_iter()
        .find(|condition| condition.kind == "authority_blocked")
        .unwrap()
}

fn owner_authority_history(case: &str, action: Action) {
    let mut fx = fixture::Fx::new();
    let task = fx.task(case, Status::Ready);
    fx.grant(&[&task.id]);
    if case == "coding gate" {
        fx.start(&task.id);
        fx.done(&task.id, "candidate.txt");
    } else {
        fx.ctx.store.update(&task.id, |task| {
                task.status = Status::Review;
                let mut plan: corral_core::PlanRecord = serde_json::from_value(json!({
                    "id":"P1","spec_revision":"spec",
                    "artifact":{"run":"P1","kind":"plan","title":"Design","path":"plan.md",
                        "source":"/fixture/plan.md","sha256":"a","bytes":1,"added":Timestamp::now()},
                    "document":{"goal":"Test owner authority","non_goals":[],"decisions":[],
                        "affected_paths":["src/main.rs"],"contracts":[],"dependencies":[],
                        "acceptance":[{"scenario":"works","expected":"done"}],
                        "verification":["cargo test"],"owner_decisions":[]}
                }))?;
                if case == "plan decomposition" {
                    plan.approval = Some(serde_json::from_value(json!({
                        "by":"pm","at":Timestamp::now(),"policy_digest":"policy"
                    }))?);
                } else if case != "new plan review" {
                    plan.reviews.push(serde_json::from_value(json!({
                        "id":"PR1","reviewer":"rev","policy_digest":"policy","at":Timestamp::now(),
                        "verdict":if case=="planner revision" {Some("changes")} else {None},
                        "herdr":null
                    }))?);
                }
                let contents = format!("---\n{}\n---\n", serde_json::to_string(&plan.document)?);
                let source = fx.ctx.store.dir().join("plan.md");
                std::fs::write(&source, &contents)?;
                plan.artifact.source = source.display().to_string();
                plan.artifact.sha256 = format!("{:x}", Sha256::digest(contents.as_bytes()));
                plan.artifact.bytes = u64::try_from(contents.len())?;
                plan.spec_revision = corral_core::plan_contract_revision(task)?;
                task.plans.push(plan);
                Ok(())
            }).unwrap();
    }
    let now = Timestamp::now();
    let selection = OperatingSelection {
        tasks: [task.id.clone()].into_iter().collect(),
        authority: OperatingAuthority {
            dispatch: false,
            accept: true,
            plan_accept: false,
            aggregate_accept: false,
        },
        ..Default::default()
    };
    let preview = fx.ctx.store.preview_operating_policy(&selection).unwrap();
    fx.ctx.store.config = fx
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", now)
        .unwrap();
    corral_ops::sync_at(&fx.fresh(), now).unwrap();
    let before = authority(&fx, &task.id, now);
    let key = before.key.clone().unwrap();
    let aged = now
        .checked_add(SignedDuration::from_secs(
            i64::try_from(fx.ctx.store.config.pm.attention_owner_secs + 1).unwrap(),
        ))
        .unwrap();
    corral_ops::sync_at(&fx.fresh(), aged).unwrap();
    fx.ctx
        .store
        .update_attention(&task.id, |ledger| {
            let record = ledger.records.get_mut(&key).unwrap();
            record.first_handled_at = Some(aged);
            record.handling = Some(AttentionHandling {
                revision: 1,
                request: "handling".into(),
                note: "Owner retains authority".into(),
                at: aged,
                next_check: aged.checked_add(SignedDuration::from_hours(1))?,
                by: Responsible::Owner,
            });
            Ok(())
        })
        .unwrap();
    let handled = fx.ctx.store.get(&task.id).unwrap();
    let history = handled.attention.records.get(&key).unwrap().clone();
    assert!(history.handling.is_some(), "{case}");
    assert!(history.levels.owner_claimed_at.is_some(), "{case}");
    let submissions = fx.records("submissions.jsonl").len();
    for operation in 0..3 {
        let at = aged
            .checked_add(SignedDuration::from_secs(operation + 1))
            .unwrap();
        fx.ctx
            .store
            .update(&task.id, |task| {
                task.park = if operation < 2 {
                    Some(Park {
                        version: 1,
                        revision: u64::try_from(operation + 1)?,
                        by: Responsible::Pm,
                        reason: "Wait".into(),
                        at,
                        resume_when: vec![],
                        resume_action: String::new(),
                        review_by: at.checked_add(SignedDuration::from_hours(24))?,
                        ready_at: None,
                    })
                } else {
                    None
                };
                Ok(())
            })
            .unwrap();
        let current = fx.ctx.store.get(&task.id).unwrap();
        let policy = fx.ctx.store.effective_operating_policy().unwrap();
        assert!(!policy.allows(&task.id, action), "{case}");
        if let Some(park) = &current.park {
            assert!(policy.tasks[&task.id].refusals[&action].contains(&park.refusal(&task.id)));
        }
        let projected = authority(&fx, &task.id, at);
        assert_eq!(projected.key, before.key, "{case}: operation {operation}");
        assert_eq!(projected.reason, before.reason, "{case}");
        assert_eq!(projected.since, before.since, "{case}");
        corral_ops::sync_at(&fx.fresh(), at).unwrap();
        let (_, retained) = corral_ops::sync_with_attention(&fx.fresh()).unwrap();
        let retained = retained
            .unwrap()
            .tasks
            .into_iter()
            .find(|row| row.task == task.id)
            .unwrap()
            .assessment
            .conditions
            .into_iter()
            .find(|condition| condition.kind == "authority_blocked")
            .unwrap();
        assert_eq!(retained.key, before.key, "{case}: retained projection");
        assert_eq!(
            retained.reason, before.reason,
            "{case}: retained projection"
        );
        let after = fx.ctx.store.get(&task.id).unwrap();
        assert_eq!(after.attention.records.get(&key), Some(&history), "{case}");
        assert_eq!(fx.records("submissions.jsonl").len(), submissions, "{case}");
    }
    assert!(
        !fx.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .any(|event| { event.kind == "attention_resolved" && event.detail["key"] == key.0 }),
        "{case}"
    );
}

#[test]
fn review_authority_survives_park_replacement_and_unpark() {
    owner_authority_history("coding gate", Action::Gate);
}

#[test]
fn new_plan_review_authority_survives_parking() {
    owner_authority_history("new plan review", Action::PlanReview);
}

#[test]
fn pending_plan_review_authority_survives_parking() {
    owner_authority_history("pending plan review", Action::PlanReview);
}

#[test]
fn planner_revision_authority_survives_parking() {
    owner_authority_history("planner revision", Action::Plan);
}

#[test]
fn plan_decomposition_authority_survives_parking() {
    owner_authority_history("plan decomposition", Action::AcceptPlan);
}
