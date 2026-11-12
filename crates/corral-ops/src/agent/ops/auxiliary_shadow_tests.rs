use crate::agent::test_support::*;
use crate::agent::*;
use corral_core::agent::{AgentRole, AuxiliaryOutcome, UsageSessionKind, pm_state};
use serde_json::json;

#[test]
fn shadow_auxiliary_requires_pm_capability_and_records_separate_usage() {
    let fx = fx();
    let target = BindTarget::Pm {
        project: "corral".into(),
    };
    let host = bind(&fx, &target, "pm-pane", "pm-session");
    committed(&hello(&fx, &host));
    let call = |seq, action: &str, request: &str| {
        let mut payload = json!({"kind":"auxiliary", "action":action,
            "request":request, "usage_kind":"shadow"});
        if action == "prepare" {
            payload["model"] = json!("openai-codex/cheap");
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
        serde_json::from_value(json!({"pm":{"shadow":{"enabled":true,"model":"cheap"}}})).unwrap();
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
    committed(&call(2, "prepare", "on"));
    committed(&call(3, "started", "on"));
    committed(&call(4, "finish", "on"));
    let saved = state(&fx, &target.container());
    assert_eq!(saved.usage_sessions.len(), 1);
    assert_eq!(saved.usage_sessions[0].kind, UsageSessionKind::Shadow);
    assert_eq!(
        saved.usage_sessions[0].outcome,
        Some(AuxiliaryOutcome::Completed)
    );
    assert_eq!(
        saved.usage_sessions[0].model.as_deref(),
        Some("openai-codex/cheap")
    );
}
