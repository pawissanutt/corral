//! Prepared-review comparison projects canonical native/inbox facts, never empty histories.
mod store_support;
use corral_core::{PlanReviewHead, agent::AgentRunState};
use serde_json::json;

#[test]
fn actual_plan_review_projection_matches_full_and_keyed_heads_with_native_history() {
    for scale in [1, 10] {
        let (_dir, store) = store_support::store();
        let mut task = store_support::rich(&store, 2, 2, 2, 2 * scale);
        let plan = task.plans.last_mut().unwrap();
        let review = plan.reviews.last_mut().unwrap();
        let mut state = AgentRunState::new(
            serde_json::from_value(json!({
                "role":"plan_reviewer", "store":"fixture", "project":"app", "generation":1
            }))
            .unwrap(),
        );
        for i in 0..2 * scale {
            state.wakes.push(serde_json::from_value(json!({"key":format!("old-{i}"),
                "request_id":format!("old-{i}"), "kind":"wait", "target":"W1", "state":"fired", "at":store_support::at(0)})).unwrap());
        }
        review.agent_host = Some(Box::new(state));
        let expected = PlanReviewHead::of(review);
        let plan_id = plan.id.clone();
        let review_id = review.id.clone();
        store.save(&task).unwrap();
        let actual = store
            .head(&task.id)
            .unwrap()
            .plan_review(&plan_id, &review_id)
            .unwrap()
            .unwrap();
        assert_eq!(expected, actual);
        assert_eq!(
            expected.agent_host.as_ref().unwrap().records.counts[4],
            (2 * scale) as u64
        );
    }
}
