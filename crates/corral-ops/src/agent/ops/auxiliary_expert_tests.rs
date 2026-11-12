use crate::agent::test_support::*;
use crate::agent::*;
use corral_core::agent::{AgentRole, AuxiliaryOutcome, UsageSessionKind, pm_state};
use serde_json::json;

#[test]
fn expert_auxiliary_is_capability_gated_capped_and_separately_accounted() {
    let fx = fx();
    let target = BindTarget::Pm {
        project: "corral".into(),
    };
    let host = bind(&fx, &target, "pm-pane", "pm-session");
    committed(&hello(&fx, &host));
    let call = |seq, action: &str, request: &str| {
        let mut payload = json!({"kind":"auxiliary", "action":action,
            "request":request, "usage_kind":"expert"});
        if action == "prepare" {
            payload["model"] = json!("openai-codex/strong");
        }
        if action == "finish" {
            payload["outcome"] = json!("completed");
        }
        send(
            &fx,
            &envelope(&host, "event", &format!("ev-1-{seq}"), payload, false),
            "event",
        )
    };
    assert_eq!(code(&call(1, "prepare", "off")), Some("op_not_allowed"));
    let config: corral_core::agent_capabilities::AgentCapabilities =
        serde_json::from_value(json!({"pm":{"expert":{"enabled":true,"model":"strong"}}})).unwrap();
    let snapshot = config.resolve(AgentRole::Pm, None);
    pm_state::update(&fx.ctx.store, |pm| {
        pm.as_mut()
            .unwrap()
            .state
            .generation_mut(1)
            .unwrap()
            .capabilities = Some(snapshot);
        Ok(((), true))
    })
    .unwrap();
    committed(&call(2, "prepare", "first"));
    committed(&call(3, "started", "first"));
    committed(&call(4, "prepare", "second"));
    assert_eq!(code(&call(5, "started", "second")), Some("bad_request"));
    committed(&call(6, "finish", "first"));
    let saved = state(&fx, &target.container());
    assert_eq!(saved.usage_sessions.len(), 2);
    assert_eq!(saved.usage_sessions[0].kind, UsageSessionKind::Expert);
    assert_eq!(
        saved.usage_sessions[0].outcome,
        Some(AuxiliaryOutcome::Completed)
    );
    assert_eq!(
        saved.usage_sessions[0].model.as_deref(),
        Some("openai-codex/strong")
    );
}
