use super::*;
use crate::connection::{ConnectionBinding, ConnectionId};
use serde_json::json;

fn owner(run: &str, generation: u32) -> PaidBinding {
    PaidBinding {
        container: ContainerRef::Worker {
            task: "T-1".into(),
            run: run.into(),
        },
        generation,
        binding: ConnectionBinding::new(
            ConnectionId::OpenaiApi,
            Some("approved-api".into()),
            "same-model".into(),
            None,
        )
        .unwrap(),
    }
}
fn episode() -> PaidEpisode {
    let owner = owner("R1", 1);
    PaidEpisode {
        version: 1,
        id: "opaque-episode".into(),
        root: PaidRoot {
            store: "store".into(),
            role: AgentRole::Worker,
            subject: "T-1".into(),
            attempt: "R1".into(),
        },
        approval: PaidApproval {
            policy_digest: "11".repeat(32),
            approval: "owner-receipt".into(),
            allowance_nanos: 100,
            candidates: vec![owner.binding.clone()],
        },
        epoch: 1,
        switches: 0,
        phase: PaidPhase::Active { owner },
        ledger: BudgetLedger::default(),
        owners: vec![],
        legacy_usage_gap: false,
    }
}
fn state(owner: &PaidBinding, link: Option<PaidLink>) -> AgentRunState {
    let mut state = AgentRunState::new(serde_json::from_value(json!({
        "role":"worker", "store":"store", "project":"p", "generation":owner.generation, "health":"healthy"
    })).unwrap());
    state.generations.push(serde_json::from_value(json!({
        "n":owner.generation, "bind_nonce_sha256":"00", "pane":"fixture", "created":"2026-11-01T00:00:00Z",
        "connection":owner.binding, "bound":{"host_cap_sha256":"00", "ctl_cap_sha256":"00", "endpoint":"/fixture.sock", "session_id":"fixture",
            "session_file":"/fixture.jsonl", "pid":1, "start_time":"fixture", "request_id":"fixture", "at":"2026-11-01T00:00:00Z"}
    })).unwrap());
    state.paid_episode = link;
    state
}
fn request(id: &str, generation: u32, nanos: u64) -> BudgetRequest {
    BudgetRequest {
        connection: Some(ConnectionId::OpenaiApi),
        request: id.into(),
        generation,
        tokens: 10,
        usd_nanos: nanos,
        priced: true,
        unbounded: false,
        response: None,
    }
}
#[test]
fn paid_lineage_retains_uncertainty_and_fences_before_activation() {
    let mut episode = episode();
    let source = owner("R1", 1);
    let link = PaidLink {
        episode: episode.id.clone(),
        epoch: 1,
    };
    assert!(
        episode
            .reserve(&link, &source, request("uncertain", 1, 60))
            .unwrap()
    );
    assert!(
        !episode
            .reserve(&link, &source, request("parallel", 1, 50))
            .unwrap()
    );
    let successor = owner("R2", 1);
    let mut source_state = state(&source, Some(link.clone()));
    let next = episode
        .prepare_transfer(&link, &source, successor.clone(), &mut source_state)
        .unwrap();
    assert_eq!(source_state.binding.health, HostHealth::Fenced);
    assert!(
        episode
            .reserve(&link, &source, request("stale", 1, 1))
            .is_err()
    );
    assert!(
        episode
            .settle(&link, &source, "uncertain", "late".into(), 1, 1)
            .is_err()
    );
    // Crash/reload in the no-active-owner window does not release a cent.
    let mut episode: PaidEpisode =
        serde_json::from_slice(&serde_json::to_vec(&episode).unwrap()).unwrap();
    assert!(
        episode
            .reserve(&next, &successor, request("too-early", 1, 1))
            .is_err()
    );
    episode
        .activate(&next, &successor, &mut state(&successor, None))
        .unwrap();
    assert!(
        !episode
            .reserve(&next, &successor, request("over-cap", 1, 50))
            .unwrap()
    );
    assert!(
        episode
            .reserve(&next, &successor, request("final", 1, 40))
            .unwrap()
    );
    episode
        .settle(&next, &successor, "final", "response".into(), 5, 20)
        .unwrap();
    assert_eq!(episode.ledger.totals(), (15, 80));
    assert!(
        episode
            .settle(&next, &successor, "final", "response".into(), 5, 20)
            .is_err()
    );
    episode
        .prepare_transfer(
            &next,
            &successor,
            owner("R3", 1),
            &mut state(&successor, Some(next.clone())),
        )
        .unwrap();
    assert_eq!(episode.switches, 0); // same API binding, not another billing-route switch
    assert_eq!(episode.ledger.requests[0].response, None);
}
