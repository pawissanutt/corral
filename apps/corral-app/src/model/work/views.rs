//! Feed-backed work content descriptors and phone destination navigation.

use super::{Action, Button, ChangeFilter, Press, State};
use crate::model::board::ChangeKind;
use crate::model::hue::Hue;
use crate::model::text::Rich;

/// A connection banner with its entire inline emphasis and handled controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Banner {
    /// Connection state hue.
    pub hue: Hue,
    /// Complete message; the renderer inserts no additional wording.
    pub text: Rich,
    /// Desktop/terminal controls; the phone banner has none.
    pub actions: Vec<Button>,
}

/// Staleness flags and text used by every work-domain screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// Dim and desaturate board content when the daemon is down or the device is offline.
    pub dimmed: bool,
    /// Snapshot timestamp to accompany stale views.
    pub as_of: Option<String>,
    /// Connection banner; healthy and relay connections have no banner.
    pub banner: Option<Banner>,
}
/// A model-owned tab label, selected state and reducer action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tab {
    /// Complete visible tab wording.
    pub label: &'static str,
    /// Whether this tab is selected.
    pub selected: bool,
    /// The domain action that selects it.
    pub action: Action,
}

/// One feed row in Now or Changes, with link and illustrative flags resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedRow {
    /// Local time, `18:40` today or `Nov 12` before.
    pub at: String,
    /// Project context when the workspace-wide feed needs disambiguation.
    pub project_chip: Option<String>,
    /// The task the event concerns; empty for an event about none.
    pub id: String,
    /// The mark drawn for the event.
    pub kind: ChangeKind,
    /// Exact event wording.
    pub text: String,
    /// Who did it and through what, when corrald records attribution.
    pub by: Option<String>,
    /// New since this device's last visit.
    pub unseen: bool,
    /// The historical rows after the seen line are dimmed.
    pub dim: bool,
    /// Whether the source explicitly marks this event as illustrative.
    pub illustrative: bool,
    /// Rows about a task open it; the historical source rows are plain text.
    pub action: Option<Press>,
}

impl FeedRow {
    /// What a screen reader hears: time, task, event and attribution.
    pub fn label(&self) -> String {
        [self.at.as_str(), self.id.as_str(), self.text.as_str()]
            .into_iter()
            .chain(self.by.as_deref())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}
/// A Changes filter control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedFilter {
    pub label: &'static str,
    /// Whether the feed shows this filter's events now.
    pub selected: bool,
    /// The filter it selects.
    pub filter: ChangeFilter,
}

/// The filter controls, the same on every form.
pub fn feed_filters(state: &State) -> Vec<FeedFilter> {
    ChangeFilter::ALL
        .into_iter()
        .map(|filter| FeedFilter {
            label: filter.label(),
            selected: state.filter == filter,
            filter,
        })
        .collect()
}

/// Complete Changes content before and after the per-device seen line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changes {
    /// Page title.
    pub title: &'static str,
    /// Desktop subtitle explaining the per-device cursor, absent on phone.
    pub subtitle: Option<String>,
    /// The filter controls.
    pub filters: Vec<FeedFilter>,
    /// Recent rows before the seen marker.
    pub recent: Vec<FeedRow>,
    /// Seen-cursor line; desktop names the device, phone uses a shorter last-visit label.
    pub seen_line: String,
    /// Rows this device has seen, dimmed.
    pub history: Vec<FeedRow>,
    /// Optional source disclosure below the feed.
    pub illustrative: Option<&'static str>,
    /// Desktop-only Mark seen control.
    pub mark_seen: Option<Button>,
    /// Fetch the page before the oldest row shown, while corrald has more.
    pub older: Option<Button>,
    /// One line instead of, or after, the rows: loading, empty, refused or failed.
    pub notice: Option<String>,
    /// Connection banner and stale flags.
    pub status: Status,
}
