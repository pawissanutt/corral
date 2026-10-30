//! Actual stored native containers with scalable old generations/turns and rich task history.
use corral_core::agent::{AgentBinding, AgentRole, AgentRunState, HostHealth};
use serde_json::json;

#[path = "../../../corral-core/tests/store_support/mod.rs"]
pub(crate) mod data;

/// Expected full hydration from canonical row families, independent of measured work.
pub(crate) fn full_read_cost(
    doc: &corral_core::backend::TaskDoc,
) -> std::collections::BTreeMap<corral_core::work::Counter, u64> {
    use corral_core::{backend::TaskPart, work::Counter};
    let mut cost: std::collections::BTreeMap<_, _> = Counter::ALL
        .into_iter()
        .map(|counter| (counter, 0))
        .collect();
    cost.insert(Counter::FullTaskHydrations, 1);
    cost.insert(Counter::ViewsOpened, 1);
    cost.insert(Counter::TaskHeadDecodes, 1);
    let bytes = |value: &serde_json::Value| serde_json::to_vec(value).unwrap().len() as u64;
    let mut total = bytes(&doc.head.doc) + bytes(&doc.head.summary);
    for row in &doc.rows {
        let counter = match row.part {
            TaskPart::Run if row.current => Counter::RunDecodes,
            TaskPart::PlanRun | TaskPart::Plan | TaskPart::AggregateAttempt if row.current => {
                Counter::TaskHeadDecodes
            }
            TaskPart::Run
            | TaskPart::PlanRun
            | TaskPart::Plan
            | TaskPart::AggregateAttempt
            | TaskPart::Comment
            | TaskPart::Artifact
            | TaskPart::ScopeRevision
            | TaskPart::PmReceipt
            | TaskPart::MemberGateHistory
            | TaskPart::Recovery
            | TaskPart::RecoveryOutcome
            | TaskPart::PaneRef
            | TaskPart::Finding
            | TaskPart::Prompt
            | TaskPart::GateAttempt
            | TaskPart::ReviewEvidence => Counter::HistoryRowDecodes,
            _ => Counter::NestedRowDecodes,
        };
        *cost.get_mut(&counter).unwrap() += 1;
        total += bytes(&row.doc);
    }
    cost.insert(Counter::DecodedDocBytes, total);
    cost
}

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
