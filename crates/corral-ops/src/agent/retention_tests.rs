//! PM container retention through the request path: generations beyond the cap, startup at the
//! receipt cap, fenced refusal below the floor, and usage and budget facts that survive
//! retirement.
use corral_core::agent::{
    AgentGeneration, ContainerRef, GENERATIONS_MAX, GenerationFence, RequestIdentity,
    RequestReceipt, UsageSession, UsageSessionKind, paths, pm_state,
};
use corral_core::connection::{ConnectionBinding, ConnectionId};
use serde_json::{Value, json};

use super::{PM_CAPACITY, PmCapacity};
use crate::agent::test_support::*;
use crate::agent::{BindTarget, CallResponse, event_request_id};

const FIXTURE: &str = include_str!("../../../../tools/pi-probe/fixtures/session-usage.jsonl");

fn pm_target() -> BindTarget {
    BindTarget::Pm {
        project: "corral".into(),
    }
}

/// Retires every unpinned ended generation of a two-generation container.
fn retire_ended(fx: &Fx) {
    let limits = pm_state::RetentionLimits {
        receipts_high_water: u64::MAX,
        receipts_retained: 0,
        generations_high_water: 2,
        generations_retained: 1,
    };
    pm_state::update_retained(
        &fx.ctx.store,
        &pm_state::Selection::default(),
        &limits,
        |_| Ok(((), false)),
    )
    .unwrap();
}

fn budget_event(host: &Host, action: &str, tokens: u64) -> Vec<u8> {
    let mut payload = json!({"kind": "budget", "action": action, "request": "next"});
    if tokens > 0 {
        payload["tokens"] = json!(tokens);
        payload["priced"] = json!(true);
    }
    let request = event_request_id(host.identity.generation, 1);
    envelope(host, "event", &request, payload, false)
}

/// A small PM receipt cap on this test thread, restored on drop.
struct SmallCap;

impl SmallCap {
    fn set(capacity: PmCapacity) -> Self {
        PM_CAPACITY.with(|cell| cell.set(Some(capacity)));
        SmallCap
    }
}

impl Drop for SmallCap {
    fn drop(&mut self) {
        PM_CAPACITY.with(|cell| cell.set(None));
    }
}

#[test]
fn a_generation_beyond_the_cap_opens_when_earlier_ones_hold_only_their_hello() {
    let fx = fx();
    let first = bind(&fx, &pm_target(), "pm-pane", "s-pm");
    let hello_receipt = committed(&hello(&fx, &first));
    let last = GENERATIONS_MAX as u32;
    // Generations 2..=64 as their launches leave them: bound, each holding only its hello.
    pm_state::update(&fx.ctx.store, |pm| {
        let state = &mut pm.as_mut().unwrap().state;
        let template = state.generation(1).unwrap().clone();
        for n in 2..=last {
            state.generations.push(AgentGeneration {
                n,
                ..template.clone()
            });
            state.receipts.push(RequestReceipt {
                request_id: event_request_id(n, 0),
                identity: RequestIdentity {
                    generation: n,
                    ..hello_receipt.identity.clone()
                },
                ..hello_receipt.clone()
            });
        }
        let fence = GenerationFence {
            reason: "replaced".into(),
            at: template.created,
        };
        for generation in state.generations.iter_mut().filter(|g| g.n < last) {
            generation.fenced = Some(fence.clone());
        }
        state.binding.generation = last;
        Ok(((), true))
    })
    .unwrap();
    let store = &fx.ctx.store;
    assert_eq!(
        pm_state::child_count(store, "generations").unwrap(),
        GENERATIONS_MAX as u64
    );
    let host = bind(&fx, &pm_target(), "pm-pane", "s-pm");
    committed(&hello(&fx, &host));
    assert_eq!(host.identity.generation, last + 1);
    let count = pm_state::child_count(store, "generations").unwrap();
    assert!(count < GENERATIONS_MAX as u64, "{count}");
}

#[test]
fn hello_commits_from_a_receipt_family_at_its_cap() {
    const CAP: u64 = 8;
    let _cap = SmallCap::set(PmCapacity {
        cap: CAP,
        limits: pm_state::RetentionLimits {
            receipts_high_water: CAP - 2,
            receipts_retained: CAP / 2,
            ..pm_state::RetentionLimits::PM
        },
    });
    let fx = fx();
    let old = bind(&fx, &pm_target(), "pm-pane", "old-session");
    let template = committed(&hello(&fx, &old));
    let current = bind(&fx, &pm_target(), "pm-pane", "current-session");
    pm_state::update(&fx.ctx.store, |pm| {
        let receipts = &mut pm.as_mut().unwrap().state.receipts;
        let fill = CAP as usize - receipts.len();
        receipts.extend((0..fill).map(|i| RequestReceipt {
            request_id: format!("settled-{i}"),
            ..template.clone()
        }));
        Ok(((), true))
    })
    .unwrap();
    let store = &fx.ctx.store;
    assert_eq!(pm_state::child_count(store, "receipts").unwrap(), CAP);
    let bound = committed(&hello(&fx, &current));
    assert_eq!(bound.identity.generation, current.identity.generation);
    assert!(pm_state::child_count(store, "receipts").unwrap() < CAP);
}

#[test]
fn a_retired_generation_is_fenced_and_the_current_generation_still_replays() {
    let fx = fx();
    let old = bind(&fx, &pm_target(), "pm-pane", "old-session");
    committed(&hello(&fx, &old));
    let current = bind(&fx, &pm_target(), "pm-pane", "current-session");
    committed(&hello(&fx, &current));
    let input = budget_event(&current, "state", 0);
    let first = committed(&send(&fx, &input, "event"));
    retire_ended(&fx);
    assert!(state(&fx, &ContainerRef::Pm).generation(1).is_none());

    let stale = send(&fx, &budget_event(&old, "state", 0), "event");
    assert_eq!(code(&stale), Some("agent_fenced"));
    match send(&fx, &input, "event") {
        CallResponse::Receipt {
            receipt,
            replayed: true,
        } => assert_eq!(receipt, first),
        other => panic!("expected the stored receipt, got {other:?}"),
    }
}

fn write_lines(path: &std::path::Path, lines: &[Value]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(path, text).unwrap();
}

#[test]
fn usage_totals_are_unchanged_by_retiring_a_generation() {
    let fx = fx();
    let old = bind(&fx, &pm_target(), "pm-pane", "old-session");
    committed(&hello(&fx, &old));
    let current = bind(&fx, &pm_target(), "pm-pane", "current-session");
    committed(&hello(&fx, &current));
    // Generation 1 has its own transcript under a recorded connection, plus an oracle session.
    let fixture: Vec<Value> = FIXTURE
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let responses = FIXTURE
        .lines()
        .filter(|l| l.contains("\"responseId\""))
        .count();
    let transcript = old.sessions.join("generation-1.jsonl");
    let mut lines = fixture.clone();
    lines.insert(
        1,
        json!({"type": "custom", "customType": "corral_host_bound", "data": {"generation": 1}}),
    );
    write_lines(&transcript, &lines);
    let oracle = paths::usage_session_file(&fx.ctx.store, &ContainerRef::Pm, "oracle");
    let mut lines = fixture;
    lines[0]["id"] = "oracle-session".into();
    write_lines(&oracle, &lines);
    let binding =
        ConnectionBinding::new(ConnectionId::AnthropicApi, None, "same-model".into(), None)
            .unwrap();
    pm_state::update(&fx.ctx.store, |pm| {
        let state = &mut pm.as_mut().unwrap().state;
        let generation = state.generation_mut(1).unwrap();
        generation.bound.as_mut().unwrap().session_file = transcript.clone();
        generation.connection = Some(binding);
        state.usage_sessions.push(UsageSession {
            generation: 1,
            kind: UsageSessionKind::Oracle,
            session_id: "oracle-session".into(),
            session_file: oracle.clone(),
            model: None,
            request: None,
            outcome: None,
        });
        Ok(((), true))
    })
    .unwrap();
    let collect = || crate::agent::usage::collect_container(&ContainerRef::Pm, &fx.ctx.store);
    let before = collect().unwrap();
    assert_eq!(before.entries.len(), 2 * responses);

    retire_ended(&fx);

    assert!(state(&fx, &ContainerRef::Pm).generation(1).is_none());
    assert_eq!(collect().unwrap(), before);
}

#[test]
fn a_legacy_usage_gap_survives_retiring_its_generation() {
    let mut fx = fx();
    let old = bind(&fx, &pm_target(), "pm-pane", "old-session");
    committed(&hello(&fx, &old));
    pm_state::update(&fx.ctx.store, |pm| {
        let generation = pm.as_mut().unwrap().state.generation_mut(1).unwrap();
        generation.capabilities.as_mut().unwrap().catalog = 6;
        Ok(((), true))
    })
    .unwrap();
    fx.ctx
        .store
        .config
        .agent
        .capabilities
        .set_settings(
            corral_core::agent::AgentRole::Pm,
            "budget",
            [("max_tokens".into(), json!(100))].into(),
        )
        .unwrap();
    let current = bind(&fx, &pm_target(), "pm-pane", "current-session");
    committed(&hello(&fx, &current));
    retire_ended(&fx);
    assert!(state(&fx, &ContainerRef::Pm).generation(1).is_none());

    let reply = committed(&send(&fx, &budget_event(&current, "reserve", 10), "event"));
    assert_eq!(reply.result["admitted"], false);
    assert_eq!(reply.result["legacy_usage_gap"], true);
}
