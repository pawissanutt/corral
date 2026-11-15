//! Availability episodes are canonical provider quota observations. Agent `max_usd`
//! settings cap an individual execution, not a connection's allowance: never turn
//! those local budgets into provider quotas or fabricate a reset/window/limit.
use std::collections::BTreeMap;

use anyhow::{Context, Result};
use corral_core::AvailabilityClass;
use corral_core::backend::{PageReq, RecordQuery};
use corral_protocol::detail::UsageQuota;
use jiff::Timestamp;

use super::{MAX_ROWS, PAGE, aggregate::Key, budget::Budget};
use crate::Ctx;

pub(super) type Quotas = BTreeMap<Key, Vec<UsageQuota>>;

pub(super) fn unknown() -> UsageQuota {
    UsageQuota {
        source: "unknown".into(),
        scope: None,
        window: None,
        used: None,
        limit: None,
        headroom: None,
        resets_at: None,
        observed_at: None,
        exhausted: None,
        completeness: Default::default(),
    }
}

pub(super) fn read(
    ctx: &Ctx,
    now: Timestamp,
) -> Result<(Quotas, corral_protocol::detail::UsageCompleteness)> {
    let view = ctx.store.view()?;
    let query = RecordQuery {
        area: Some("availability".into()),
        key: None,
    };
    let mut page = PageReq::first(PAGE);
    let mut budget = Budget::new(MAX_ROWS);
    let mut latest: BTreeMap<(Key, String), (Timestamp, UsageQuota)> = BTreeMap::new();
    'records: loop {
        let found = view
            .records(&query, page.clone())
            .context("reading provider quota observations")?;
        for row in found.items {
            if !budget.admit(1) {
                break 'records;
            }
            let Some(episode) = crate::availability::load(ctx, &row.key)
                .context("decoding provider quota observation")?
            else {
                budget.partial("quota_record_changed");
                continue;
            };
            budget.observe(episode.started);
            for attempt in episode.attempts {
                for failure in attempt.failures {
                    if !budget.admit(1) {
                        break 'records;
                    }
                    budget.observe(failure.at);
                    let AvailabilityClass::QuotaExhausted { account, reset_at } = failure.class
                    else {
                        continue;
                    };
                    if failure.at > now {
                        continue;
                    }
                    let scope = if account {
                        "account".to_owned()
                    } else {
                        format!("model:{}", attempt.model)
                    };
                    let key = (
                        (attempt.connection.clone(), Some(episode.profile.clone())),
                        scope.clone(),
                    );
                    if latest.get(&key).is_some_and(|(at, _)| *at >= failure.at) {
                        continue;
                    }
                    let exhausted = if failure.recovered_at.is_some_and(|at| at <= now) {
                        Some(false)
                    } else if reset_at.is_some_and(|at| at <= now) {
                        // A scheduled reset is not an observation that service recovered.
                        None
                    } else {
                        Some(true)
                    };
                    latest.insert(
                        key,
                        (
                            failure.at,
                            UsageQuota {
                                source: "availability_exhaustion".into(),
                                scope: Some(scope),
                                resets_at: reset_at.map(|at| at.to_string()),
                                observed_at: Some(failure.at.to_string()),
                                exhausted,
                                ..unknown()
                            },
                        ),
                    );
                }
            }
        }
        match found.next {
            Some(next) => page.after = Some(next),
            None => break,
        }
    }
    let mut quotas = Quotas::new();
    for ((key, _), (_, mut quota)) in latest {
        quota.completeness = budget.coverage.clone();
        quotas.entry(key).or_default().push(quota);
    }
    Ok((quotas, budget.coverage))
}
