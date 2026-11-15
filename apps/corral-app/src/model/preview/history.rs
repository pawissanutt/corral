use super::{change, put, stamp};
use crate::model::daemon::Feed;
use corral_protocol::{app::*, board::CostBasis, detail::*, rows::*};

pub(super) fn coverage(feed: &Feed, rows: u64) -> UsageCompleteness {
    UsageCompleteness {
        partial: false,
        reason: Some("synthetic_preview".into()),
        scanned_rows: rows,
        scanned_from: Some(stamp(feed, feed.now.saturating_sub(29 * 86400))),
        scanned_to: Some(stamp(feed, feed.now)),
        unpriced_entries: 0,
        undated_entries: 0,
    }
}

pub(super) fn populate(feed: &mut Feed) {
    for (id, text, kind) in [
        (
            "T-893",
            "accepted plan T-885 P2 → 3 members published",
            ChangeKind::Plan,
        ),
        ("T-867", "worker started · pi-sol xhigh", ChangeKind::Work),
        ("T-871", "worker started · pi-sol xhigh", ChangeKind::Work),
        (
            "T-868",
            "worker replied in thread on T-868/R1/Q1",
            ChangeKind::Question,
        ),
        ("T-852", "gate went stale on af7a6dc", ChangeKind::Gate),
        ("T-890", "quick gate started on 1751bd2", ChangeKind::Gate),
        (
            "T-867",
            "gate went stale on 5949a4b (base moved)",
            ChangeKind::Gate,
        ),
        ("T-894", "gate started on 6b36916", ChangeKind::Gate),
    ] {
        change(feed, Some(id.into()), text.into(), kind);
    }
    feed.seen = Some(SeenCursor {
        through: 2,
        at: Some(stamp(feed, feed.now.saturating_sub(3600))),
    });
    feed.changes_unseen = Some((2, feed.changes_head.saturating_sub(2)));
    for (index, (id, connection, profile, used, today, week)) in [
        (
            "openai-subscription",
            "OpenAI · subscription",
            "pi-sol xhigh",
            0.78,
            61.2,
            318.4,
        ),
        (
            "anthropic-subscription",
            "Anthropic · subscription",
            "pi-opus high",
            0.34,
            22.7,
            141.0,
        ),
        (
            "openai-api",
            "OpenAI · API (metered)",
            "fallback · pi-sol",
            0.21,
            8.4,
            29.9,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let completeness = coverage(feed, 30);
        feed.usage.insert(
            id.into(),
            UsageRow {
                id: id.into(),
                connection: connection.into(),
                profile: Some(profile.into()),
                window: Some("5 h rolling · preview".into()),
                used: Some(used * 100.0),
                limit: Some(100.0),
                limit_basis: LimitBasis::Declared,
                resets_at: Some(stamp(
                    feed,
                    feed.now.saturating_add(6360 + index as u64 * 1200),
                )),
                today_usd: Some(today),
                week_usd: Some(week),
                cost_basis: Some(CostBasis::Estimated),
                today_completeness: completeness.clone(),
                week_completeness: completeness.clone(),
                quotas: vec![UsageQuota {
                    source: "Preview · synthetic allowance".into(),
                    scope: Some(id.into()),
                    window: Some("5 h".into()),
                    used: Some(used * 100.0),
                    limit: Some(100.0),
                    headroom: Some((1.0 - used) * 100.0),
                    resets_at: Some(stamp(feed, feed.now.saturating_add(6360))),
                    observed_at: Some(stamp(feed, feed.now)),
                    exhausted: Some(false),
                    completeness,
                }],
                rev: 1,
            },
        );
    }
    for period in ["today", "week", "month"] {
        let _ = detail(
            feed,
            &DetailRef::UsageSeries {
                period: period.into(),
            },
        );
    }
    let _ = detail(
        feed,
        &DetailRef::Burnup {
            scope: "workspace".into(),
        },
    );
    let rows = ["T-903", "T-902", "T-901"]
        .into_iter()
        .enumerate()
        .filter_map(|(index, id)| {
            let task = feed.board.task(id)?;
            Some(FiledIdea {
                id: format!("preview-filed:{id}"),
                seq: 3 - index as u64,
                task: id.into(),
                session: "A-7".into(),
                quick: true,
                device: Some("Preview desktop".into()),
                at: stamp(feed, feed.now.saturating_sub(index as u64 * 300)),
                title: task.title.clone(),
                status: "idea".into(),
                decision: None,
            })
        })
        .collect();
    put(
        feed,
        DetailRef::Filings { before: None },
        DetailBody::Filings(FilingsPage {
            rows,
            total: 3,
            older: None,
            newer: None,
        }),
    );
}

fn day(feed: &Feed, seconds: u64) -> String {
    let timestamp = stamp(feed, seconds);
    if let (Some(zone), Ok(timestamp)) = (&feed.time_zone, timestamp.parse::<jiff::Timestamp>()) {
        timestamp.to_zoned(zone.clone()).date().to_string()
    } else {
        timestamp[..10].into()
    }
}

pub(super) fn detail(feed: &mut Feed, target: &DetailRef) -> Result<bool, String> {
    let body = match target {
        DetailRef::UsageSeries { period } => {
            let days = match period.as_str() {
                "today" => 1,
                "week" => 7,
                "month" => 30,
                _ => return Err(format!("Unknown preview usage period {period}")),
            };
            let providers = feed
                .usage
                .values()
                .map(|row| ProviderSeries {
                    id: row.id.clone(),
                    points: (0..12)
                        .map(|hour| UsagePoint {
                            at: stamp(feed, feed.now.saturating_sub((11 - hour) * 3600)),
                            used: row.used.map(|used| used * (hour + 1) as f64 / 12.0),
                            limit: row.limit,
                            cost_usd: row.today_usd.map(|cost| cost * (hour + 1) as f64 / 12.0),
                            cost_basis: Some(CostBasis::Estimated),
                            completeness: coverage(feed, 1),
                        })
                        .collect(),
                })
                .collect();
            let days = (0..days)
                .rev()
                .map(|offset| UsageDay {
                    day: day(feed, feed.now.saturating_sub(offset * 86400)),
                    by_profile: feed
                        .usage
                        .values()
                        .map(|row| ProfileUsage {
                            profile: row.profile.clone().unwrap_or_default(),
                            connection: row.connection.clone(),
                            cost_usd: row
                                .today_usd
                                .map(|cost| cost * (1.0 - (offset % 5) as f64 * 0.1)),
                            cost_basis: Some(CostBasis::Estimated),
                            tokens: Some(12000 + offset * 100),
                            completeness: coverage(feed, 1),
                        })
                        .collect(),
                })
                .collect();
            DetailBody::UsageSeries(UsageSeries {
                providers,
                days,
                completeness: coverage(feed, 30),
            })
        }
        DetailRef::Timeline { from, to, by } => {
            let start = from
                .parse::<jiff::Timestamp>()
                .map_err(|error| format!("Preview timeline start: {error}"))?
                .as_second();
            let end = to
                .parse::<jiff::Timestamp>()
                .map_err(|error| format!("Preview timeline end: {error}"))?
                .as_second();
            if start >= end {
                return Err("Preview timeline range must increase".into());
            }
            let lower = u64::try_from(start).unwrap_or_default();
            let upper = u64::try_from(end).unwrap_or_default().min(feed.now);
            let lanes = if *by == TimelineBy::Agent {
                feed.agents
                    .values()
                    .map(|row| {
                        lane(
                            feed,
                            &row.id,
                            &row.title,
                            lower,
                            upper,
                            if row.role == "gate" {
                                TimelineKind::Gate
                            } else {
                                TimelineKind::Run
                            },
                        )
                    })
                    .collect()
            } else {
                feed.board
                    .tasks()
                    .iter()
                    .filter(|row| row.agent.is_some() || row.questions > 0)
                    .map(|row| {
                        lane(
                            feed,
                            &row.id,
                            &row.title,
                            lower,
                            upper,
                            if row.questions > 0 {
                                TimelineKind::Ask
                            } else {
                                TimelineKind::Run
                            },
                        )
                    })
                    .collect()
            };
            DetailBody::Timeline(TimelinePage {
                lanes,
                owner_actions: feed
                    .changes
                    .values()
                    .filter(|line| line.at >= *from && line.at < *to)
                    .map(|line| TimelineAction {
                        at: line.at.clone(),
                        task: line.task.clone(),
                        text: line.text.clone(),
                        source_ref: None,
                    })
                    .collect(),
                pm: vec![],
                last_visit: feed.seen.as_ref().and_then(|seen| seen.at.clone()),
                completeness: coverage(feed, 8),
            })
        }
        DetailRef::Burnup { scope } => {
            let total = if scope == "workspace" {
                feed.board.tasks().len()
            } else if let Some(plan) = scope.strip_prefix("plan:") {
                feed.board
                    .tasks()
                    .iter()
                    .filter(|row| row.plan.as_deref() == Some(plan))
                    .count()
            } else if let Some(project) = scope.strip_prefix("project:") {
                feed.board
                    .tasks()
                    .iter()
                    .filter(|row| row.project.as_deref() == Some(project))
                    .count()
            } else {
                return Err(format!("Unknown preview burn-up scope {scope}"));
            } as u32;
            DetailBody::Burnup(Burnup {
                scope: scope.clone(),
                points: (0..14)
                    .rev()
                    .map(|offset| BurnupPoint {
                        at: stamp(feed, feed.now.saturating_sub(offset * 86400)),
                        total,
                        landed: (14 - offset as u32).min(total) / 3,
                    })
                    .collect(),
                completeness: coverage(feed, 14),
            })
        }
        DetailRef::Filings { before } => {
            let source = DetailRef::Filings { before: None };
            let Some(DetailBody::Filings(mut page)) =
                feed.detail(&source).and_then(|cache| cache.body()).cloned()
            else {
                return Err("Preview filings are unavailable".into());
            };
            if let Some(before) = before {
                page.rows.retain(|row| row.seq < *before);
                page.newer = None;
            }
            DetailBody::Filings(page)
        }
        _ => return Ok(false),
    };
    put(feed, target.clone(), body);
    Ok(true)
}

fn lane(
    feed: &Feed,
    id: &str,
    title: &str,
    from: u64,
    to: u64,
    kind: TimelineKind,
) -> TimelineLane {
    TimelineLane {
        id: id.into(),
        title: format!("Preview · {title}"),
        segments: if from < to {
            vec![TimelineSegment {
                kind,
                from: Some(stamp(feed, from)),
                to: Some(stamp(feed, to)),
                note: Some("Synthetic preview interval".into()),
            }]
        } else {
            vec![]
        },
    }
}

pub(super) fn refresh_filings(feed: &mut Feed) {
    let targets: Vec<_> = feed
        .details
        .values()
        .filter_map(|cache| match cache.body() {
            Some(DetailBody::Filings(page)) => Some((cache.target.clone(), page.clone())),
            _ => None,
        })
        .collect();
    for (target, mut page) in targets {
        for row in &mut page.rows {
            if let Some(task) = feed.board.task(&row.task) {
                row.title = task.title.clone();
                row.status = task.now_code.clone();
            }
        }
        put(feed, target, DetailBody::Filings(page));
    }
    let board = &feed.board;
    for thread in feed.threads.values_mut() {
        for entry in thread.entries.values_mut() {
            if let Some(corral_protocol::assistant::EntryCard::Idea { task, title, .. }) =
                &mut entry.card
                && let Some(row) = board.task(task)
            {
                *title = row.title.clone();
            }
        }
    }
}
