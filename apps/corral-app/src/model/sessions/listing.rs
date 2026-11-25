use std::collections::BTreeMap;

use super::{SessionRow, State, filter::Query, row, summary};
use crate::model::{
    Id,
    agents::session_lines,
    daemon::Feed,
    effect::Env,
    list::Section,
    projects::Scope,
    status::{SessionState, session_look},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Activity,
    Started,
    Cost,
    Name,
}

impl Sort {
    pub const ALL: [Self; 4] = [Self::Activity, Self::Started, Self::Cost, Self::Name];

    pub fn slug(self) -> &'static str {
        match self {
            Self::Activity => "activity",
            Self::Started => "started",
            Self::Cost => "cost",
            Self::Name => "name",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Activity => "Last activity",
            Self::Started => "Started",
            Self::Cost => "Cost",
            Self::Name => "Name",
        }
    }

    /// The label where room is short (a phone, a terminal's section heading).
    pub fn short(self) -> &'static str {
        match self {
            Self::Activity => "Activity",
            other => other.label(),
        }
    }

    pub fn phone_fact(self, row: &SessionRow) -> Option<String> {
        let value = match self {
            Self::Activity => row.activity.as_ref(),
            Self::Started => row.age.as_ref(),
            Self::Cost => row.cost.as_ref(),
            Self::Name => return None,
        };
        value.map(|value| format!("{}: {value}", self.label()))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ModeChip {
    pub mode: String,
    pub label: String,
    pub count: usize,
    pub selected: bool,
}

#[derive(PartialEq, Eq)]
pub struct Listing {
    pub sections: Vec<Section>,
    pub chips: Vec<ModeChip>,
    pub had_rows: bool,
    pub summary: String,
}

/// Filter before counting or folding. Facets count matches to all terms except mode.
pub fn listing(
    feed: &Feed,
    scope: &Scope,
    state: &State,
    env: Env,
    pinned: impl Fn(&Id) -> bool,
) -> Listing {
    let query = Query::parse(&state.filter);
    let lines = session_lines(feed, &scope.filter());
    let had_rows = !lines.is_empty();
    let mut counts = BTreeMap::from_iter(
        ["chat", "code", "build", "worker", "reviewer"].map(|role| (role.to_owned(), 0)),
    );
    let mut total = 0;
    let mut matching = Vec::new();
    for line in lines {
        let row = row::from_line(feed, line, env);
        let count = counts.entry(row.role.clone()).or_default();
        if query.without_mode(&row, || pinned(&row.line.id)) {
            *count += 1;
            total += 1;
            if query.mode(&row.role) {
                matching.push(row);
            }
        }
    }
    let mut chips = vec![ModeChip {
        mode: "all".into(),
        label: format!("All {total}"),
        count: total,
        selected: !query.has_mode(),
    }];
    chips.extend(counts.into_iter().map(|(mode, count)| ModeChip {
        label: format!("{mode} {count}"),
        count,
        selected: query.selected_mode() == Some(mode.as_str()),
        mode,
    }));
    let sections = groups(feed, state, &matching);
    Listing {
        sections,
        chips,
        had_rows,
        summary: summary(&matching),
    }
}

fn groups(feed: &Feed, state: &State, matching: &[SessionRow]) -> Vec<Section> {
    SessionState::ORDER
        .into_iter()
        .filter_map(|group| {
            let mut rows: Vec<_> = matching
                .iter()
                .filter(|row| row.line.state == group)
                .collect();
            let sort = state.sort(group.slug());
            rows.sort_by(|a, b| {
                match sort {
                    Sort::Activity => b.line.activity(feed).cmp(&a.line.activity(feed)),
                    Sort::Started => row::started(feed, b.line.id.as_str())
                        .cmp(&row::started(feed, a.line.id.as_str())),
                    Sort::Cost => match (a.spent.0, b.spent.0) {
                        (Some(a), Some(b)) => b.total_cmp(&a),
                        (a, b) => b.is_some().cmp(&a.is_some()),
                    },
                    Sort::Name => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
                }
                .then_with(|| a.line.id.cmp(&b.line.id))
            });
            let count = rows.len();
            let open = state.unfolded.contains(group.slug());
            let section = Section {
                key: group.slug().to_owned(),
                label: group.word().to_owned(),
                look: Some(session_look(group)),
                count,
                note: group.note().map(str::to_owned),
                fold: group.folds().then_some(open),
                rows: if group.folds() && !open {
                    Vec::new()
                } else {
                    rows.into_iter().map(|row| row.line.id.clone()).collect()
                },
            };
            section.shown().then_some(section)
        })
        .collect()
}
