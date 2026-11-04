use super::*;

#[test]
fn reviewer_reads_stored_refs_without_paths_and_cannot_cross_projects() {
    let fx = ts::fx();
    let (id, reviewer) = reviewing_task(&fx);
    let plan = fx.ctx.store.get(&id).unwrap().plans[0].clone();
    std::fs::remove_file(&plan.artifact.source).unwrap();
    let reference = format!("{id}@P1");
    let page = committed(&op(
        &fx,
        &reviewer,
        "read_content",
        "read-plan",
        json!({
            "reference": reference, "limit": 16,
        }),
    ));
    assert_eq!(page.result["text"], &PLAN[..16]);
    assert_eq!(page.result["next_offset"], 16);
    let rest = committed(&op(
        &fx,
        &reviewer,
        "read_content",
        "rest-plan",
        json!({
            "reference": reference, "offset": 16,
        }),
    ));
    assert_eq!(rest.result["text"], &PLAN[16..]);
    assert!(rest.result["next_offset"].is_null());
    let artifact = committed(&op(
        &fx,
        &reviewer,
        "read_content",
        "read-artifact",
        json!({
            "reference": plan.artifact.path,
        }),
    ));
    assert_eq!(artifact.result["text"], PLAN);
    let mut offset = 0;
    let mut spec = String::new();
    let mut total = None;
    loop {
        let page = committed(&op(
            &fx,
            &reviewer,
            "read_content",
            &format!("read-spec-{offset}"),
            json!({"reference": id, "offset": offset}),
        ));
        let bytes = page.result["bytes"].as_u64().unwrap();
        assert_eq!(
            *total.get_or_insert(bytes),
            bytes,
            "read receipts must not grow the manifest"
        );
        spec.push_str(page.result["text"].as_str().unwrap());
        let Some(next) = page.result["next_offset"].as_u64() else {
            break;
        };
        offset = next;
    }
    let spec: Value = serde_json::from_str(&spec).unwrap();
    assert_eq!(spec["spec"], fx.ctx.store.get(&id).unwrap().body);
    assert_eq!(spec["plans"][0]["id"], "P1");
    let foreign = fx
        .ctx
        .store
        .create(
            "foreign",
            Some("other"),
            corral_core::Status::Ready,
            "private",
        )
        .unwrap();
    let denied = op(
        &fx,
        &reviewer,
        "read_content",
        "foreign",
        json!({"reference": foreign.id}),
    );
    assert!(matches!(denied, CallResponse::Rejected { .. }));
    // Read receipts never change the plan or submit the review.
    let after = fx.ctx.store.get(&id).unwrap().plans.remove(0);
    assert_eq!(after.document, plan.document);
    assert_eq!(after.artifact, plan.artifact);
    assert_eq!(after.reviews[0].verdict, None);
    assert_eq!(after.reviews[0].notes, plan.reviews[0].notes);
}
