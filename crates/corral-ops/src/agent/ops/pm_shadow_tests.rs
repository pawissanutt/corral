use super::*;
use crate::agent::{
    BindTarget,
    test_support::{self as ts, committed, envelope, hello, send},
    tool_request_id,
};

fn fixture() -> (ts::Fx, ts::Host, String) {
    let mut fx = ts::fx();
    fx.ctx.store.config.projects.insert(
        "corral".into(),
        serde_json::from_value(json!({"path": fx.ctx.store.root()})).unwrap(),
    );
    fx.ctx.store.save_config().unwrap();
    let task = ts::worker_task(&fx, "worker-pane");
    let host = ts::bind(
        &fx,
        &BindTarget::Pm {
            project: "corral".into(),
        },
        "pm-pane",
        "pm",
    );
    committed(&hello(&fx, &host));
    (fx, host, task)
}

fn current(fx: &ts::Fx, host: &ts::Host) -> (String, String) {
    let input = envelope(
        host,
        "read_brief",
        &tool_request_id(&host.identity.session_id, "read-brief"),
        json!({}),
        false,
    );
    let receipt = committed(&send(fx, &input, "read_brief"));
    assert!(
        serde_json::to_vec(&receipt.result).unwrap().len() <= corral_core::agent::RESULT_MAX_BYTES
    );
    (
        receipt.result["shadow_digest"].as_str().unwrap().to_owned(),
        receipt.result["next"]["options"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned(),
    )
}

fn input(host: &ts::Host, digest: &str, choice: &str) -> Vec<u8> {
    envelope(
        host,
        "pm_shadow",
        &tool_request_id(&host.identity.session_id, "shadow"),
        json!({
            "digest": digest, "choice": choice, "requested_escalation": false,
            "model": "test/cheap", "reason": "closed choice",
        }),
        false,
    )
}

#[test]
fn current_shadow_commits_once_without_changing_tasks_or_events() {
    let (fx, host, task) = fixture();
    let (digest, choice) = current(&fx, &host);
    let before_task = ts::task_bytes(&fx, &task);
    let before_events = serde_json::to_vec(fx.ctx.store.events_shared().unwrap().as_ref()).unwrap();
    let input = input(&host, &digest, &choice);
    let first = committed(&send(&fx, &input, "pm_shadow"));
    assert_eq!(first.result["recorded"], true);
    assert_eq!(first.result["stale"], false);
    assert_eq!(
        first.result["agrees"],
        first.result["code_recommendation"] == choice
    );
    assert_eq!(send(&fx, &input, "pm_shadow").receipt(), Some(&first));
    let shadow = pm_shadow::load(&fx.ctx.store).unwrap();
    assert_eq!(shadow.counters.total, 1);
    assert_eq!(shadow.decisions.len(), 1);
    assert_eq!(shadow.decisions[0].model, "test/cheap");
    assert_eq!(ts::task_bytes(&fx, &task), before_task);
    assert_eq!(
        serde_json::to_vec(fx.ctx.store.events_shared().unwrap().as_ref()).unwrap(),
        before_events
    );
    let rows = crate::pm_shadow_export(&fx.ctx).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].case_id, digest);
    assert_eq!(rows[0].option, choice);
    assert_eq!(rows[0].reported_cost_usd, None);
}

#[test]
fn another_cards_digest_is_recorded_stale_and_not_exported() {
    let (fx, host, _) = fixture();
    let (current_digest, choice) = current(&fx, &host);
    let mut different = crate::pm_card(
        &crate::PmObservation::AutomaticWait {
            task: "other-task".into(),
            condition: "external".into(),
        },
        "corral",
    );
    different.kind = crate::PmActionKind::Gate;
    let different_digest = pm_escalation::shadow_digest(&different);
    assert_ne!(different_digest, current_digest);
    let receipt = committed(&send(
        &fx,
        &input(&host, &different_digest, &choice),
        "pm_shadow",
    ));
    assert_eq!(receipt.result["recorded"], true);
    assert_eq!(receipt.result["stale"], true);
    assert!(receipt.result["agrees"].is_null());
    let shadow = pm_shadow::load(&fx.ctx.store).unwrap();
    assert_eq!(shadow.counters.total, 1);
    assert_eq!(shadow.counters.stale, 1);
    assert_eq!(shadow.counters.agree + shadow.counters.disagree, 0);
    assert_eq!(shadow.decisions[0].digest, different_digest);
    assert!(crate::pm_shadow_export(&fx.ctx).unwrap().is_empty());
}

#[test]
fn choice_outside_current_options_is_refused_without_recording() {
    let (fx, host, _) = fixture();
    let (digest, _) = current(&fx, &host);
    let response = send(&fx, &input(&host, &digest, "not-an-option"), "pm_shadow");
    assert_eq!(ts::code(&response), Some("bad_request"));
    assert_eq!(pm_shadow::load(&fx.ctx.store).unwrap().counters.total, 0);
}

#[test]
fn worker_container_cannot_record_shadow_choices() {
    let fx = ts::fx();
    let task = ts::worker_task(&fx, "worker-pane");
    let host = ts::bind(
        &fx,
        &BindTarget::Worker {
            task,
            run: "R1".into(),
        },
        "worker-pane",
        "worker",
    );
    committed(&hello(&fx, &host));
    let response = send(&fx, &input(&host, &"a".repeat(64), "escalate"), "pm_shadow");
    assert_eq!(ts::code(&response), Some("op_not_allowed"));
    assert_eq!(pm_shadow::load(&fx.ctx.store).unwrap().counters.total, 0);
}

#[test]
fn brief_digest_preserves_cards_and_omits_at_wire_cap() {
    let (fx, host, _) = fixture();
    let read = |op: &str| {
        committed(&send(
            &fx,
            &envelope(
                &host,
                op,
                &tool_request_id(&host.identity.session_id, op),
                json!({}),
                false,
            ),
            op,
        ))
        .result
    };
    let next = read("read_next");
    assert!(next.get("shadow_digest").is_none());
    let brief = read("read_brief");
    assert_eq!(brief["next"], next);
    let card: crate::PmAction = serde_json::from_value(next).unwrap();
    assert_eq!(brief["shadow_digest"], pm_escalation::shadow_digest(&card));

    let mut capped = brief;
    capped.as_object_mut().unwrap().remove("shadow_digest");
    capped["padding"] = json!("");
    let remaining =
        corral_core::agent::RESULT_MAX_BYTES - serde_json::to_vec(&capped).unwrap().len();
    capped["padding"] = json!("x".repeat(remaining));
    assert_eq!(
        serde_json::to_vec(&capped).unwrap().len(),
        corral_core::agent::RESULT_MAX_BYTES
    );
    let observed = with_brief_digest(capped.clone(), &card).unwrap();
    assert!(observed.get("shadow_digest").is_none());
    assert_eq!(observed, capped);
}
