//! Actual stored native containers with scalable old generations/turns and rich task history.
use corral_core::agent::{AgentBinding, AgentRole, AgentRunState, HostHealth};
use serde_json::json;

#[path = "../../../corral-core/tests/store_support/mod.rs"]
pub(crate) mod data;

pub(crate) fn host(role: AgentRole, n: usize) -> AgentRunState {
    let at = data::at(0);
    let mut state = AgentRunState::new(AgentBinding {
        role,
        store: "fixture".into(),
        project: "app".into(),
        generation: 999,
        health: HostHealth::Healthy,
        callback: None,
        closing: false,
    });
    let generation = |n| {
        serde_json::from_value(json!({"n":n,"bind_nonce_sha256":"00".repeat(32),"pane":"p1","created":at,"bound":{
        "host_cap_sha256":"11".repeat(32),"ctl_cap_sha256":"22".repeat(32),"session_id":"fixed-session","session_file":"/tmp/native-delivery-session.jsonl",
        "pid":3,"start_time":"1234","request_id":"hello","at":at}})).unwrap()
    };
    for i in 1..=n {
        state.generations.push(generation(i as u32));
        state.turns.push(serde_json::from_value(json!({"generation":i,"seq":1,"origin":{"kind":"unknown"},"started":at,"ended":at,"class":"completed","provider_error":null})).unwrap());
    }
    state.generations.push(generation(999));
    state
}
