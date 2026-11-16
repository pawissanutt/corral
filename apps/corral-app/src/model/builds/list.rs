//! corrald's build rows as the Builds list: grouped by outcome (in flight first, then builds
//! that landed over open findings), newest first within a group, with the list's loading,
//! empty, error and stale states. Absent facts stay absent; nothing falls back to samples.

use std::cmp::Ordering;

use corral_protocol::rows::BuildRow;

use super::Action;
use super::wording::{self, Outcome};
use crate::model::daemon::{Feed, Link};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::projects::{ProjectFilter, project_chip};
use crate::model::text::{Rich, span};
use crate::model::work::when;
use crate::model::{Id, Selection};

/// What stands in for, or above, the rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListNotice {
    /// The builds topic has not delivered its first snapshot.
    Loading,
    /// A complete snapshot holds no build.
    Empty {
        heading: &'static str,
        body: &'static str,
    },
    /// The builds topic failed before any row arrived.
    Error(String),
    /// Rows are the last ones received; the list is not current.
    Stale(String),
}

/// The Builds list, including the phone's large title.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildList {
    pub title: &'static str,
    /// Loading, empty, error or stale state; absent when the rows are current.
    pub notice: Option<ListNotice>,
    /// Outcome groups in order; empty groups are left out.
    pub sections: Vec<ListSection>,
}

/// One outcome group.
#[derive(Debug, Clone, PartialEq)]
pub struct ListSection {
    pub label: &'static str,
    pub hue: Hue,
    pub count: u32,
    pub rows: Vec<ListRow>,
}

/// One build in the list.
#[derive(Debug, Clone, PartialEq)]
pub struct ListRow {
    pub id: Id,
    pub project_chip: Option<String>,
    pub title: String,
    /// The dot's hue: the outcome's.
    pub hue: Hue,
    /// The outcome in words, with the landed commit when there is one.
    pub state: String,
    /// A chip beside the title: "override" for a build landed over open review findings.
    pub flag: Option<(&'static str, Hue)>,
    /// Project, kind, slices, review rounds and the gate summary.
    pub facts: Rich,
    /// The override's callout, under the facts on desktop and phone.
    pub alert: Option<Rich>,
    /// When the record last changed.
    pub updated: String,
    /// The recorded cost, or "cost unknown".
    pub cost: String,
    pub cost_known: bool,
    pub selected: bool,
    /// The whole row for screen readers.
    pub label: String,
    pub action: Action,
}

fn outcome(row: &BuildRow) -> Outcome {
    Outcome::of(&row.state, row.override_reason.as_deref())
}

fn instant(at: &str) -> Option<jiff::Timestamp> {
    at.parse().ok()
}

/// Served builds in list order: by outcome group, then newest update first, then id.
fn sorted<'a>(feed: &'a Feed, filter: &ProjectFilter) -> Vec<&'a BuildRow> {
    let filter = filter.resolved(feed);
    let mut rows: Vec<&BuildRow> = feed
        .builds
        .values()
        .filter(|row| filter.matches(row.project.as_deref()))
        .collect();
    rows.sort_by(|a, b| {
        outcome(a)
            .rank()
            .cmp(&outcome(b).rank())
            .then_with(|| match (instant(&a.updated), instant(&b.updated)) {
                (Some(a), Some(b)) => b.cmp(&a),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => b.updated.cmp(&a.updated),
            })
            .then_with(|| a.id.cmp(&b.id))
    });
    rows
}

/// Served build ids in list order, for j/k.
pub(super) fn order_filtered(feed: &Feed, filter: &ProjectFilter) -> Vec<Id> {
    sorted(feed, filter)
        .into_iter()
        .map(|row| Id::from(&row.id))
        .collect()
}

/// The build the desktop and terminal detail shows: the root selection while corrald still
/// lists it, otherwise the first build of the list.
pub fn current_build_filtered(
    selection: &Selection,
    feed: &Feed,
    filter: &ProjectFilter,
) -> Option<Id> {
    let filter = filter.resolved(feed);
    selection
        .build
        .clone()
        .filter(|id| {
            feed.builds
                .get(id.as_str())
                .is_some_and(|row| filter.matches(row.project.as_deref()))
        })
        .or_else(|| order_filtered(feed, &filter).into_iter().next())
}

/// The grouped list of corrald's builds with its loading, empty, error and stale states.
pub fn live_list_filtered(
    selection: &Selection,
    feed: &Feed,
    env: Env,
    filter: &ProjectFilter,
) -> BuildList {
    let filter = filter.resolved(feed);
    let phone = env.form == Form::Phone;
    let topic = feed.topics.get("builds");
    let rows = sorted(feed, &filter);
    let failed = topic.and_then(|topic| topic.error.as_ref());
    let notice = if rows.is_empty() {
        Some(if let Some(error) = failed {
            ListNotice::Error(format!("corrald could not send builds: {}", error.message))
        } else if feed.link == Link::Down {
            ListNotice::Error("No builds received from corrald yet; the connection is down.".into())
        } else if topic.is_none_or(|topic| topic.loading) && feed.builds.is_empty() {
            ListNotice::Loading
        } else {
            ListNotice::Empty {
                heading: if filter == ProjectFilter::All {
                    "No builds"
                } else {
                    "No matching builds"
                },
                body: if filter == ProjectFilter::All {
                    "No interactive build session has published a record in this workspace yet."
                } else {
                    "No served build matches this project filter."
                },
            }
        })
    } else if let Some(error) = failed {
        Some(ListNotice::Stale(format!(
            "Showing the last builds received · {}",
            error.message
        )))
    } else if feed.link == Link::Down || topic.is_some_and(|topic| topic.stale) {
        Some(ListNotice::Stale(
            "Showing the last builds received; corrald is not current.".into(),
        ))
    } else {
        None
    };
    let selected = (!phone)
        .then(|| current_build_filtered(selection, feed, &filter))
        .flatten();
    let sections = Outcome::ORDER
        .iter()
        .filter_map(|group| {
            let rows: Vec<ListRow> = rows
                .iter()
                .filter(|row| outcome(row) == *group)
                .map(|row| list_row(feed, row, selected.as_ref(), &filter))
                .collect();
            (!rows.is_empty()).then(|| ListSection {
                label: group.section(),
                hue: group.hue(),
                count: u32::try_from(rows.len()).unwrap_or(u32::MAX),
                rows,
            })
        })
        .collect();
    BuildList {
        title: "Builds",
        notice,
        sections,
    }
}

fn list_row(feed: &Feed, row: &BuildRow, selected: Option<&Id>, filter: &ProjectFilter) -> ListRow {
    let id = Id::from(&row.id);
    let outcome = outcome(row);
    let state = match (&row.landed, outcome) {
        (Some(sha), Outcome::Landed | Outcome::Override) => {
            format!("{} · {}", outcome.label(), wording::short(sha))
        }
        _ => outcome.label().to_owned(),
    };
    let (gate, gate_hue) = wording::gate(row.gate_attempts, row.gate_failures);
    let mut facts: Rich = Vec::new();
    let project_chip = project_chip(feed, filter, row.project.as_deref());
    if row.kind != "build" {
        facts.push(span(format!("{} session · ", row.kind)).dim());
    }
    facts.push(span(format!(
        "{} · {} · ",
        wording::count(row.slices, "slice", "slices"),
        wording::reviews(row.review_rounds),
    )));
    facts.push(span(gate.clone()).hue(gate_hue));
    let alert = row.override_reason.as_deref().map(wording::override_line);
    let updated = when(&row.updated, feed);
    let cost_known = row.cost_usd.is_some_and(f64::is_finite);
    let cost = wording::cost(row.cost_usd);
    let mut label = vec![
        format!("{} {}", row.id, row.title),
        state.clone(),
        crate::model::text::flat(&facts),
    ];
    label.extend(alert.as_ref().map(|line| crate::model::text::flat(line)));
    if !updated.is_empty() {
        label.push(format!("updated {updated}"));
    }
    label.push(cost.clone());
    ListRow {
        title: row.title.clone(),
        project_chip,
        hue: outcome.hue(),
        state,
        flag: (outcome == Outcome::Override).then_some(("override", Hue::Held)),
        facts,
        alert,
        updated,
        cost,
        cost_known,
        selected: selected == Some(&id),
        label: label.join(", "),
        action: Action::Open(id.clone()),
        id,
    }
}
