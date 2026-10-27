use super::*;

fn published() -> Fixture {
    let mut fx = Fixture::new();
    fx.config.review.reviewer = Some("omo".into());
    fx.add("app", Status::Backlog, -2);
    fx.add("app", Status::Ready, -1);
    let mut parent = fx.tasks[0].clone();
    grow_history(&mut parent, 1);
    let mut plan = reviewed(&parent);
    plan.run = Some("D000001".into());
    plan.document.owner_decisions = vec!["Choose delivery".into()];
    parent.plans.push(plan);
    let policy = crate::plan_policy::policy_digest(&fx.config, &parent).unwrap();
    let source = fx.store.dir().join("approved-plan.md");
    let plan = &mut parent.plans[0];
    plan.reviews[0].policy_digest = policy.clone();
    let mut owner = plan.reviews[0].clone();
    owner.id = "PR2".into();
    owner.reviewer = "owner".into();
    owner.notes = Some("Resolved: Choose delivery => Main".into());
    plan.reviews.push(owner);
    plan.approval = Some(crate::PlanApproval {
        by: "owner".into(),
        at: clock(),
        policy_digest: policy,
        authorization: None,
    });
    let bytes = format!(
        "---\n{}---\n",
        serde_yaml_ng::to_string(&plan.document).unwrap()
    );
    use sha2::{Digest, Sha256};
    fs::write(&source, &bytes).unwrap();
    plan.artifact.sha256 = format!("{:x}", Sha256::digest(bytes.as_bytes()));
    plan.artifact.source = source.to_str().unwrap().into();
    plan.artifact.path = "approved-plan.md".into();
    fx.put(&parent);
    let member = oracle::Oracle(&fx.store)
        .operating_member(&fx.config, &parent)
        .unwrap();
    fx.config
        .automation
        .scope
        .tasks
        .insert(parent.id.clone(), member);
    let plan = parent.plans[0].clone();
    fx.config.automation.scope.plans.push(ScopedPlan {
        task: parent.id.clone(),
        revision: plan.id.clone(),
        artifact_sha256: plan.artifact.sha256.clone(),
        scopes: BTreeMap::from([(
            "scope".into(),
            digest(&plan.document.dependencies[0]).unwrap(),
        )]),
    });
    let source_contract = operating_contract_revision(&parent).unwrap();
    let backlink = format!("{}@{}", parent.id, plan.id);
    let mut child = fx.tasks[1].clone();
    grow_history(&mut child, 1);
    child.parent = Some(parent.id.clone());
    child.plan = Some(backlink.clone());
    fx.put(&child);
    parent.status = Status::Done;
    parent.plan = Some(backlink.clone());
    parent.children = vec![child.id.clone()];
    fx.put(&parent);
    let journal = json!({
        "backlink":backlink, "document":plan.document,
        "source_contract":source_contract,
        "projected_contract":operating_contract_revision(&parent).unwrap(),
        "artifact_sha256":plan.artifact.sha256,
        "scopes":{"scope":{
            "task":child.id,
            "scope_digest":digest(&plan.document.dependencies[0]).unwrap(),
            "contract_revision":operating_contract_revision(&child).unwrap()
        }}
    });
    crate::records::RecordArea::new("plan-decompose")
        .save(&fx.store, &parent.id, &journal)
        .unwrap();
    fx.tasks = vec![parent, child];
    fx.grant(None, true);
    fx
}

#[test]
fn operating_policy_heads_match_published_plan_oracle_with_keyed_planner_and_reviews() {
    for case in 0..4 {
        // Given owner-approved publication with an older named planner and two current reviews.
        let fx = published();
        if case == 1 {
            let mut child = fx.tasks[1].clone();
            child.body.push_str("Changed");
            fx.put(&child);
        } else if case == 2 {
            let mut parent = fx.tasks[0].clone();
            parent.plans[0].reviews.last_mut().unwrap().verdict = Some(Verdict::Changes);
            fx.put(&parent);
        } else if case == 3 {
            let mut parent = fx.tasks[0].clone();
            parent.plans[0].reviews[0].policy_digest = "stale".into();
            fx.put(&parent);
        }
        // When both algorithms resolve the publication, then all outputs and reason ordering match.
        fx.parity(case);
        let (value, _) = measure(&fx);
        if case == 0 {
            assert!(value["reasons"].as_array().unwrap().is_empty(), "{value:#}");
            let policy = fx.store.resolve_operating_policy(&fx.config).unwrap();
            assert!(policy.allows(&fx.tasks[1].id, OperatingActionKind::Dispatch));
        }
    }
}

#[test]
fn operating_policy_heads_publication_budget_ignores_earlier_reviews_and_history() {
    // Given the valid publication with fixed current reviews and a named historical planner.
    let mut fx = published();
    let base = measure(&fx);
    for task in &mut fx.tasks {
        grow_history(task, 10);
        if let Some(plan) = task.plans.last_mut() {
            let previous = plan.reviews[0].clone();
            for n in 0..9 {
                let mut earlier = previous.clone();
                earlier.id = format!("PR-old-{n}");
                plan.reviews.insert(0, earlier);
            }
        }
        fx.store.fixture_put_task(task).unwrap();
    }
    // When only history grows, then the keyed predecessor and planner reads have constant work.
    let grown = measure(&fx);
    assert_eq!(base.0, grown.0);
    for counter in Counter::ALL {
        assert_eq!(
            base.1.get(counter),
            grown.1.get(counter),
            "{}",
            counter.name()
        );
    }
}
