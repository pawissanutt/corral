use super::*;
use corral_core::backend::TaskNum;
use corral_core::connection::ConnectionId;
use corral_core::{Run, Status, Store};

fn run(id: &str, agent: &str, entries: Vec<UsageEntry>) -> Result<Run> {
    let mut run: Run = serde_json::from_value(serde_json::json!({
        "id": id, "agent": agent, "worktree": "/unused", "branch": "fixture",
        "base": "main", "base_sha": "fixture", "started": Timestamp::now(),
    }))?;
    run.usage = Some(Usage::from_entries(entries));
    Ok(run)
}

fn entry(at: Timestamp, connection: ConnectionId, cost: f64) -> UsageEntry {
    UsageEntry {
        at: Some(at),
        connection: Some(connection),
        input: 1,
        reported_usd: Some(cost),
        ..UsageEntry::default()
    }
}

#[test]
fn usage_row_cap_returns_aggregated_prefix_instead_of_failing_snapshot() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let store = Store::init(tmp.path())?;
    let now = Timestamp::now();
    let mut task = store.create("bounded accounting", None, Status::Running, "")?;
    task.runs.push(run(
        "R1",
        "same-profile",
        (0..6)
            .map(|_| entry(now, ConnectionId::AnthropicApi, 1.0))
            .collect(),
    )?);
    store.fixture_put_task(&task)?;
    let num = TaskNum(task.id.strip_prefix("T-").unwrap().parse()?);
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    // One run header plus two response rows fit; four further response rows do not.
    let facts = task_facts(&ctx, num, now - jiff::SignedDuration::from_hours(1), now, 3)?;
    assert!(facts.budget.coverage.partial);
    assert_eq!(facts.budget.coverage.scanned_rows, 3);
    assert_eq!(facts.budget.coverage.reason.as_deref(), Some("row_limit"));
    assert_eq!(facts.groups.len(), 1);
    let total = facts
        .groups
        .values()
        .next()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(total.cost(), Some(2.0));
    assert_eq!(total.tokens, 2);
    assert!(total.completeness(&facts.budget.coverage, 0).partial);
    assert_eq!(facts.budget.coverage.scanned_from, Some(now.to_string()));
    assert_eq!(facts.budget.coverage.scanned_to, Some(now.to_string()));
    Ok(())
}

#[test]
fn unpriced_usage_cannot_be_presented_as_a_complete_known_cost() {
    for remainder in [false, true] {
        let mut total = Total::default();
        total.add(&UsageEntry {
            reported_usd: Some(2.0),
            ..UsageEntry::default()
        });
        total.add(&UsageEntry {
            input: 3,
            reported_usd: remainder.then_some(1.0),
            unpriced_tokens: if remainder { 3 } else { 0 },
            ..UsageEntry::default()
        });
        assert_eq!(total.cost(), None);
        assert_eq!(total.basis(), None);
        let coverage = total.completeness(&budget::Budget::new(10).coverage, 0);
        assert!(coverage.partial);
        assert_eq!(coverage.unpriced_entries, 1);
    }
}

#[test]
fn daily_series_keeps_connection_identity_for_shared_and_unknown_profiles() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let store = Store::init(tmp.path())?;
    let now = Timestamp::now();
    let mut task = store.create("connection attribution", None, Status::Running, "")?;
    task.runs.push(run(
        "R1",
        "same-profile",
        vec![
            entry(now, ConnectionId::AnthropicApi, 2.0),
            entry(now, ConnectionId::OpenaiApi, 3.0),
        ],
    )?);
    task.runs.push(run(
        "R2",
        "",
        vec![entry(now, ConnectionId::OpenaiApi, 7.0)],
    )?);
    store.fixture_put_task(&task)?;
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let DetailBody::UsageSeries(series) = usage_series(&ctx, "30d")? else {
        panic!("expected usage series");
    };
    let totals: BTreeMap<_, _> = series
        .days
        .iter()
        .flat_map(|day| &day.by_profile)
        .map(|row| {
            (
                (row.connection.as_str(), row.profile.as_str()),
                row.cost_usd,
            )
        })
        .collect();
    assert_eq!(totals.len(), 3);
    assert_eq!(totals[&("anthropic-api", "same-profile")], Some(2.0));
    assert_eq!(totals[&("openai-api", "same-profile")], Some(3.0));
    assert_eq!(totals[&("openai-api", "")], Some(7.0));
    assert!(!series.completeness.partial);
    Ok(())
}

#[test]
fn quota_rows_use_canonical_exhaustion_resets_without_inventing_allowances() -> Result<()> {
    use corral_core::{AttemptFailure, AvailabilityClass, AvailabilityEpisode, CandidateAttempt};
    let tmp = tempfile::tempdir()?;
    let store = Store::init(tmp.path())?;
    let now = Timestamp::now();
    let reset = now + jiff::SignedDuration::from_hours(1);
    let episode = AvailabilityEpisode {
        version: 1,
        id: "quota-observation".into(),
        task: "T-1".into(),
        role: corral_core::ProfileRole::Implementation,
        profile: "same-profile".into(),
        revision: "fixture".into(),
        started: now,
        attempts: vec![CandidateAttempt {
            candidate: "candidate".into(),
            provider: "anthropic".into(),
            connection: "anthropic-subscription".into(),
            model: "model".into(),
            run: None,
            selected_at: now,
            reason: "fixture".into(),
            failures: vec![AttemptFailure {
                class: AvailabilityClass::QuotaExhausted {
                    account: true,
                    reset_at: Some(reset),
                },
                message: String::new(),
                retries: 0,
                at: now,
                recovered_at: None,
            }],
        }],
        exhausted: None,
        notice: None,
    };
    corral_core::records::RecordArea::new("availability").save(&store, &episode.id, &episode)?;
    let mut task = store.create("unknown provider quota", None, Status::Running, "")?;
    task.runs.push(run(
        "R1",
        "same-profile",
        vec![entry(now, ConnectionId::OpenaiApi, 1.0)],
    )?);
    store.fixture_put_task(&task)?;
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let rows = usage_rows(&ctx)?;
    let observed = rows
        .iter()
        .find(|row| row.connection == "anthropic-subscription")
        .unwrap();
    assert_eq!(observed.limit_basis, LimitBasis::Observed);
    assert_eq!(observed.resets_at, Some(reset.to_string()));
    let proof = &observed.quotas[0];
    assert_eq!(proof.exhausted, Some(true));
    assert_eq!(proof.observed_at, Some(now.to_string()));
    assert_eq!(proof.limit, None);
    assert_eq!(proof.used, None);
    assert_eq!(proof.headroom, None);
    assert!(!proof.completeness.partial);
    let unknown = rows
        .iter()
        .find(|row| row.connection == "openai-api")
        .unwrap();
    assert_eq!(unknown.limit_basis, LimitBasis::None);
    assert_eq!(unknown.quotas[0].source, "unknown");
    assert_eq!(unknown.quotas[0].limit, None);
    assert_eq!(unknown.quotas[0].headroom, None);
    Ok(())
}

#[test]
fn summary_cost_is_grouped_under_its_profile_instead_of_the_session_profile() -> Result<()> {
    let now = Timestamp::now();
    let mut summary = entry(now, ConnectionId::OpenaiSubscription, 0.01);
    summary.profile = Some("cheap-summary".into());
    let mut facts = Aggregate::new(10);
    add_usage(
        &mut facts,
        Usage::from_entries(vec![summary]),
        Some("conversation-profile".into()),
        now - jiff::SignedDuration::from_hours(1),
        now,
    );
    assert_eq!(facts.groups.len(), 1);
    let summary = facts
        .groups
        .get(&("openai-subscription".into(), Some("cheap-summary".into())))
        .unwrap();
    assert_eq!(summary.values().next().unwrap().cost(), Some(0.01));
    Ok(())
}
