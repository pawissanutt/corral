//! Small deterministic competing snapshots, not a load/race fixture. Both backends must
//! roll back every financial/native/receipt write when either CAS owner is stale.
use super::samples::*;
use super::*;
use crate::{
    agent::{AgentRole, BudgetLedger, ContainerRef},
    connection::{ConnectionBinding, ConnectionId},
    paid_episode::*,
};
use anyhow::ensure;
use serde_json::json;

pub(super) fn sample(fx: &Fixture) -> Result<Operation> {
    let binding = ConnectionBinding::new(
        ConnectionId::OpenaiApi,
        Some("approved".into()),
        "fixture".into(),
        None,
    )?;
    let owner = PaidBinding {
        container: ContainerRef::Worker {
            task: "T-001".into(),
            run: "R1".into(),
        },
        generation: 1,
        binding: binding.clone(),
    };
    let mut episode = PaidEpisode {
        version: 1,
        id: "opaque".into(),
        root: PaidRoot {
            store: "fixture".into(),
            role: AgentRole::Worker,
            subject: "T-001".into(),
            attempt: "R1".into(),
        },
        approval: PaidApproval {
            policy_digest: "11".repeat(32),
            approval: "owner-receipt".into(),
            allowance_nanos: 100,
            candidates: vec![binding],
        },
        epoch: 1,
        switches: 0,
        phase: PaidPhase::Active {
            owner: owner.clone(),
        },
        ledger: BudgetLedger::default(),
        owners: vec![],
        legacy_usage_gap: false,
    };
    let now = fx.clock.now();
    apply(
        fx,
        Operation::CommitPaidEpisode(CommitPaidEpisode {
            episode: record(
                AREA,
                "opaque",
                None,
                Some(serde_json::to_value(&episode)?),
                now,
            ),
            task: None,
            records: PutRecords {
                writes: vec![record(
                    "native-fixture",
                    "binding-budget",
                    None,
                    Some(json!({"generation":1})),
                    now,
                )],
                events: vec![],
                reservations: vec![],
            },
        }),
    )?;
    let request = crate::agent::BudgetRequest {
        connection: Some(ConnectionId::OpenaiApi),
        request: "physical-1".into(),
        generation: 1,
        tokens: 10,
        usd_nanos: 60,
        priced: true,
        unbounded: false,
        response: None,
    };
    episode.reserve(
        &PaidLink {
            episode: "opaque".into(),
            epoch: 1,
        },
        &owner,
        request,
    )?;
    Ok(Operation::CommitPaidEpisode(CommitPaidEpisode {
        episode: record(
            AREA,
            "opaque",
            Some(1),
            Some(serde_json::to_value(&episode)?),
            now,
        ),
        task: None,
        records: PutRecords {
            writes: vec![
                record(
                    "native-fixture",
                    "binding-budget",
                    Some(1),
                    Some(json!({"generation":1,"reserved":60})),
                    now,
                ),
                record(
                    "native-fixture",
                    "receipt-1",
                    None,
                    Some(json!({"admitted":true})),
                    now,
                ),
            ],
            events: vec![],
            reservations: vec![],
        },
    }))
}
pub(super) fn atomic(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    let op = sample(&fx)?;
    let competing = op.clone();
    apply(&fx, op)?;
    let before = dump(&*fx.backend.view()?)?;
    ensure!(
        matches!(fx.second.apply(competing), Err(ApplyError::Conflict(_))),
        "stale financial snapshot admitted twice"
    );
    ensure!(
        dump(&*fx.backend.view()?)? == before,
        "stale financial commit partially wrote"
    );
    let fx = make();
    let op = sample(&fx)?;
    apply(
        &fx,
        put_records(
            vec![record(
                "native-fixture",
                "binding-budget",
                Some(1),
                Some(json!({"generation":2})),
                fx.clock.now(),
            )],
            vec![],
        ),
    )?;
    let before = dump(&*fx.backend.view()?)?;
    ensure!(
        matches!(fx.backend.apply(op), Err(ApplyError::Conflict(_))),
        "stale native generation admitted"
    );
    ensure!(
        dump(&*fx.backend.view()?)? == before,
        "episode changed despite stale native generation"
    );
    Ok(())
}
