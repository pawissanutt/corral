//! The phone's More › Projects page: a row per configured project (its name, what it holds now,
//! a badge while it needs the owner) and the Unassigned row while tasks without a project exist.
//! A project row pushes its page; Unassigned opens Now scoped to those tasks. test ids:
//! "projects-list", "project-<id>", "filter-unassigned".

use nsh_core::prelude::*;

use super::open_entry;
use crate::model::projects::{PROJECTS_HEADING, ProjectEntry, Projects};
use crate::model::{Hue, ProjectFilter};
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::chip::pill;
use crate::ui::kit::notice;
use crate::ui::kit::rows::list_row_tall;
use crate::ui::kit::text::{dim, plain};
use crate::ui::tokens::{Token, color};

pub fn phone_list(app: AppState) -> PhonePage {
    let entries = computed(move || {
        app.feed
            .with(|feed| Projects::build(feed, &ProjectFilter::All).entries)
    });
    let body = dynamic(move || {
        let entries = entries.get();
        untracked(move || {
            if entries.is_empty() {
                return notice::empty(
                    app,
                    "No projects",
                    "corrald lists no configured project in this workspace.",
                    None,
                );
            }
            view()
                .test_id("projects-list")
                .mt(px(6.0))
                .border_t_1()
                .border_t_color(color(Token::Rule))
                .children(entries.into_iter().map(|entry| row(app, entry)))
                .into_element()
        })
    });
    PhonePage {
        title: PROJECTS_HEADING.to_owned(),
        body,
        trailing: None,
        bar: None,
        ask: true,
        scroll: true,
    }
}

/// One project (`.prow`): the folder, its name over what it holds, the needs-me badge, the
/// chevron that says it opens a page.
fn row(app: AppState, entry: ProjectEntry) -> AnyElement {
    let needs = entry.needs_me.filter(|n| *n > 0);
    let summary = entry.summary();
    let label = entry.label.clone();
    list_row_tall(
        app,
        entry.spoken(),
        entry.id.clone(),
        || false,
        move || open_entry(app, &entry),
        vec![
            view()
                .w(px(28.0))
                .flex_shrink_0()
                .child(icon(Icon::Project, 20.0, color(Token::Ink)))
                .into_element(),
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(plain(app, label).truncate())
                .child(dim(app, summary).truncate())
                .into_element(),
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .flex_shrink_0()
                .accessibility_elements_hidden(true)
                .children(needs.map(|n| pill(app, n, Hue::You)))
                .child(icon(Icon::Chevron, 16.0, color(Token::Ink3)))
                .into_element(),
        ],
    )
}
