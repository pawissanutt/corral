use corral_protocol::detail::{
    BuildDetail, BuildMessage, BuildOutcome, BuildUsage, BuildUsageTotals, DetailBody,
};
use corral_protocol::rows::BuildRow;

use super::*;
use crate::model::daemon::{DetailCache, Link, TopicState, detail_key};
use crate::model::effect::{Conn, Env};
use crate::model::health::{Availability, availability};
use crate::model::nav::{Destination, Form};
use crate::model::projects::ProjectFilter;

const DESKTOP: Env = Env {
    conn: Conn::Healthy,
    form: Form::Desktop,
};

fn row(id: &str, state: &str, override_reason: Option<&str>, cost_usd: Option<f64>) -> BuildRow {
    BuildRow {
        id: id.into(),
        kind: "build".into(),
        project: Some("corral".into()),
        title: format!("Build {id}"),
        state: state.into(),
        created: "2026-11-15T09:00:00Z".into(),
        updated: "2026-11-15T10:00:00Z".into(),
        landed: (state == "landed").then(|| "4e1f2a9c0d".into()),
        slices: 2,
        review_rounds: 1,
        gate_attempts: 1,
        gate_failures: 0,
        messages: 0,
        cost_usd,
        tokens: 1_000,
        override_reason: override_reason.map(str::to_owned),
        rev: 1,
    }
}

fn usage() -> BuildUsage {
    BuildUsage {
        input: 10,
        output: 5,
        cost_usd: None,
    }
}

fn detail(id: &str, override_reason: Option<&str>) -> BuildDetail {
    BuildDetail {
        id: id.into(),
        kind: "build".into(),
        project: Some("corral".into()),
        title: format!("Build {id}"),
        rev: 1,
        state: "landed".into(),
        outcome: BuildOutcome {
            state: "landed".into(),
            at: Some("2026-11-15T10:00:00Z".into()),
            head: Some("aaaaaaaaaa".into()),
            landed: Some("bbbbbbbbbb".into()),
            base_branch: Some("main".into()),
            override_reason: override_reason.map(str::to_owned),
        },
        branch: None,
        base: None,
        intent: vec![],
        intent_total: 0,
        diff: None,
        commits: vec![],
        commits_total: 0,
        slices: vec![],
        slices_total: 0,
        verification: vec![],
        verification_total: 0,
        reviews: vec![],
        reviews_total: 0,
        messages: vec![],
        messages_total: 0,
        decisions: vec![],
        decisions_total: 0,
        usage: BuildUsageTotals {
            parent: usage(),
            children: usage(),
            reviewers: usage(),
            total: usage(),
        },
        timeline: vec![],
        timeline_total: 0,
    }
}

/// A connected feed serving builds, with `rows` listed and `details` received.
fn feed(rows: Vec<BuildRow>, details: Vec<BuildDetail>) -> Feed {
    let mut feed = Feed {
        link: Link::Up,
        ..Feed::default()
    };
    feed.capabilities
        .insert(corral_protocol::BUILDS_V1.to_owned());
    feed.topics.insert("builds".into(), TopicState::default());
    for row in rows {
        feed.builds.insert(Id::from(&row.id), row);
    }
    for body in details {
        let target = build_ref(&body.id);
        feed.details.insert(
            detail_key(&target),
            DetailCache {
                target,
                pages: vec![],
                loading: false,
                stale: false,
                error: None,
                merged: Some(DetailBody::Build(body)),
            },
        );
    }
    feed
}

fn ready(feed: &Feed, id: &str) -> BuildView {
    match detail_of(feed, &Id::from(id)) {
        Detail::Ready(view) => *view,
        other => panic!("build {id} is not ready: {other:?}"),
    }
}

#[test]
fn a_daemon_without_builds_v1_shows_builds_unavailable_not_empty_or_sample() {
    let mut old = Feed {
        link: Link::Up,
        ..Feed::default()
    };
    old.capabilities
        .insert(corral_protocol::PLANS_V1.to_owned());
    assert_eq!(
        availability(&old, Destination::Builds),
        Availability::Missing(corral_protocol::BUILDS_V1)
    );
    assert!(old.builds.is_empty());
    // A daemon that serves builds but has not sent its first snapshot is loading, not empty.
    let mut serving = feed(vec![], vec![]);
    serving.topics.insert(
        "builds".into(),
        TopicState {
            loading: true,
            stale: true,
            error: None,
        },
    );
    assert_eq!(
        availability(&serving, Destination::Builds),
        Availability::Ready
    );
    let list = live_list_filtered(
        &Selection::default(),
        &serving,
        DESKTOP,
        &ProjectFilter::All,
    );
    assert_eq!(list.notice, Some(ListNotice::Loading));
}

#[test]
fn shortened_messages_say_how_many_of_the_total_are_shown() {
    let mut body = detail("B-long", None);
    body.messages = (0..100)
        .map(|n| BuildMessage {
            at: "2026-11-15T10:00:00Z".into(),
            from: "parent".into(),
            to: format!("S{n}"),
            text: "status?".into(),
            delivery: "delivered".into(),
        })
        .collect();
    body.messages_total = 240;
    let feed = feed(vec![row("B-long", "landed", None, None)], vec![body]);
    let view = ready(&feed, "B-long");
    let messages = view
        .sections
        .iter()
        .find(|section| section.key == "messages")
        .expect("the messages section is shown");
    let note = messages.note.clone().unwrap_or_default();
    assert!(note.contains("100") && note.contains("240"), "{note}");
    // Sections without entries are left out rather than shown empty.
    assert!(view.sections.iter().all(|section| section.key != "slices"));
}

#[test]
fn a_build_landed_over_open_findings_is_called_out_in_the_list_and_the_detail() {
    let reason = "reviewer finding is a false positive";
    let feed = feed(
        vec![
            row("B-over", "landed", Some(reason), Some(2.5)),
            row("B-plain", "landed", None, Some(1.0)),
        ],
        vec![detail("B-over", Some(reason))],
    );
    let list = live_list_filtered(&Selection::default(), &feed, DESKTOP, &ProjectFilter::All);
    let holding = |id: &str| {
        list.sections
            .iter()
            .find(|section| section.rows.iter().any(|row| row.id == id))
            .expect("the build is listed")
    };
    assert_ne!(holding("B-over").label, holding("B-plain").label);
    let over = holding("B-over")
        .rows
        .iter()
        .find(|row| row.id == "B-over")
        .expect("listed");
    assert!(over.flag.is_some() && over.alert.is_some());
    assert!(crate::model::text::flat(over.alert.as_ref().unwrap()).contains(reason));
    let view = ready(&feed, "B-over");
    assert!(crate::model::text::flat(view.alert.as_ref().unwrap()).contains(reason));
}

#[test]
fn an_unknown_cost_is_never_shown_as_zero() {
    let feed = feed(
        vec![row("B-free", "open", None, None)],
        vec![detail("B-free", None)],
    );
    let list = live_list_filtered(&Selection::default(), &feed, DESKTOP, &ProjectFilter::All);
    let listed = &list.sections[0].rows[0];
    assert!(!listed.cost_known);
    assert!(!listed.cost.contains('$'), "{}", listed.cost);
    let view = ready(&feed, "B-free");
    let Some(Block::Table { rows, .. }) = view
        .sections
        .iter()
        .find(|section| section.key == "usage")
        .and_then(|section| section.blocks.first())
    else {
        panic!("the usage table is shown");
    };
    assert!(rows.iter().all(|cells| !cells[3].contains('$')));
}
