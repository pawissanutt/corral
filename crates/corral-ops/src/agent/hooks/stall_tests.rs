//! Native provider evidence must come from indexed turns, never empty compatibility state.
use super::*;
use crate::agent::bridge::fixture as f;
use corral_core::agent::{AgentBinding, AgentRole, AgentRunState, HostHealth};
use corral_core::work::{self, Counter, HotScope};

#[test]
fn reviewer_auth_failure_is_selected_from_real_current_generation_with_constant_work() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let fx = f::fixture(false, 5);
        let target = f::reviewing(&fx);
        let container = target.container();
        let mut host = AgentRunState::new(AgentBinding {
            role: AgentRole::Reviewer,
            store: "fixture".into(),
            project: "corral".into(),
            generation: 2,
            health: HostHealth::Healthy,
            callback: None,
            closing: false,
        });
        let at: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
        for n in 0..2 * scale {
            host.turns.push(serde_json::from_value(json!({"generation":1,"seq":n,"origin":{"kind":"unknown"},"started":at,"ended":null,"class":null,"provider_error":null})).unwrap());
            host.turns.push(serde_json::from_value(json!({"generation":2,"seq":n,"origin":{"kind":"unknown"},"started":at,"ended":at,"class":"completed","provider_error":null})).unwrap());
        }
        host.turns.push(serde_json::from_value(json!({"generation":2,"seq":9999,"origin":{"kind":"unknown"},"started":at,"ended":at,"class":"provider_failed","provider_error":"auth"})).unwrap());
        // A later append in an older generation cannot conceal current authentication failure.
        host.turns.push(serde_json::from_value(json!({"generation":1,"seq":99999,"origin":{"kind":"unknown"},"started":at,"ended":null,"class":null,"provider_error":null})).unwrap());
        fx.ctx
            .store
            .update(&fx.id, |task| {
                task.runs[0].review_runs[0].agent_host = Some(Box::new(host));
                Ok(())
            })
            .unwrap();
        let head = fx.ctx.store.head(&fx.id).unwrap();
        let hot = HotScope::enter();
        let before = work::snapshot();
        let Drive::Escalated(reason) = advance_reviewer(&fx.ctx, &head, &container, at).unwrap()
        else {
            panic!("authentication failure was hidden")
        };
        assert!(reason.contains("corral agent login") && reason.contains("no paid fallback"));
        let after = work::snapshot();
        measured.push(Counter::ALL.map(|c| after.total(c) - before.total(c)));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
        drop(hot);
        assert_eq!(fx.events("provider_escalated").len(), 1);
        assert!(fx.events("provider_retry").is_empty());
    }
    assert_eq!(measured[0], measured[1]);
}
