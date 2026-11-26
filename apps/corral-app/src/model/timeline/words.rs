//! What the Timeline's lane and plan header buttons and the inspector say, so the canvas and
//! the terminal say the same.

use super::{PlacedHeader, PlacedLane};

/// What a press on a lane does: on the selected lane it opens what the lane opens, on any
/// other it selects it.
pub fn lane_verb(lane: &PlacedLane) -> String {
    if lane.selected {
        lane.opens.label.clone()
    } else {
        format!("Select {}", lane.id)
    }
}

/// The inspector's button that deselects the lane.
pub const CLOSE_INSPECTOR: &str = "Close";

/// An out-of-order warning behind the danger mark: `! started before T-2 landed`.
pub fn flagged(warning: &str) -> String {
    format!("! {warning}")
}

/// A canvas lane button's name, and its hover hint: the press and any warning, led by the
/// lane's full id and title when the canvas `cut` its label to fit.
pub fn lane_words(lane: &PlacedLane, cut: bool) -> (String, String) {
    let verb = lane_verb(lane);
    let warning = lane
        .warning
        .as_deref()
        .map(|words| format!(" · {}", flagged(words)))
        .unwrap_or_default();
    let spoken = format!("{verb}: {} {}{warning}", lane.id, lane.title);
    let hint = if cut {
        format!("{} {} · {verb}{warning}", lane.id, lane.title)
    } else {
        format!("{verb}{warning}")
    };
    (spoken, hint)
}

/// A plan header button's name: the plan, its tally and what a press does to it.
pub fn header_spoken(header: &PlacedHeader) -> String {
    let verb = if header.folded { "unfold" } else { "fold" };
    format!("{} · {verb}", header.spoken)
}
