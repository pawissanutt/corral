//! Facts in a terminal side column (the thread page's session facts, a session page's run).
//! Such a column is under 30 Cells, too narrow for a label column beside a value, so a label
//! stands over its value; an id, profile or machine leading the value stays one line, cut with
//! an ellipsis rather than broken mid-word.

use nsh_core::prelude::*;

use crate::model::text::Rich;
use crate::state::AppState;
use crate::ui::kit::text::{dim, mono, rich_small};
use crate::ui::term;

/// `label` (quiet, one line) over `value`, indented two Cells.
pub fn stacked(app: AppState, label: impl Into<SharedString>, value: AnyElement) -> AnyElement {
    view()
        .min_w(px(0.0))
        .child(dim(app, label).truncate())
        .child(view().pl(term::cells(2.0)).min_w(px(0.0)).child(value))
        .into_element()
}

/// A value whose leading mono runs (the id it names) are one line cut to fit, still opening the
/// record they link; any words after them wrap on the lines below.
pub fn identified(app: AppState, value: &Rich) -> AnyElement {
    let lead = value.iter().take_while(|run| run.mono).count();
    let (ids, rest) = value.split_at(lead);
    let id_line = (!ids.is_empty()).then(|| {
        let words: String = ids.iter().map(|run| run.text.as_str()).collect();
        let line = mono(app, words).truncate().min_w(px(0.0));
        match ids.iter().find_map(|run| run.live.clone()) {
            Some(live) => line
                .underline()
                .cursor_pointer()
                .role(AccessibilityRole::Link)
                .on_press(move || app.open_live(live.clone()))
                .into_element(),
            None => line.into_element(),
        }
    });
    let after = (!rest.is_empty()).then(|| {
        let mut rest = rest.to_vec();
        // On its own line the words need no separator from the id above them.
        if !ids.is_empty()
            && let Some(first) = rest.first_mut()
        {
            first.text = first.text.trim_start_matches([' ', '·']).to_owned();
        }
        rich_small(app, &rest)
    });
    view()
        .min_w(px(0.0))
        .children(id_line)
        .children(after)
        .into_element()
}
