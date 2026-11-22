//! Configuration's Extensions part: a row per configured extension (its lines, projects,
//! whether it is on, corrald's last error and skipped count), pressing one shows its fields
//! under the rows. With none, one sentence and the guide. IDs: `seg-config-extensions-<name>`,
//! `config-extension-<name>`, `btn-config-extensions-guide`.

use nsh_core::prelude::*;

use super::super::parts::{cell, space};
use super::group;
use crate::model::settings::editor::Part;
use crate::model::settings::extensions::{self, Row};
use crate::model::settings::form::{Entries, Group, Live};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::text::h3;
use crate::ui::tokens::Token;

pub fn part(app: AppState, listed: &Entries, groups: Vec<Group>, live: Memo<Live>) -> AnyElement {
    if listed.names.is_empty() {
        return view()
            .min_w(px(0.0))
            .gap(px(8.0))
            .child(cell(app, extensions::EMPTY, Token::Ink3, false))
            .child(guide(app))
            .into_element();
    }
    let chosen = listed.chosen.clone();
    let rows = computed(move || {
        app.settings.with(|state| {
            app.feed
                .with(|feed| extensions::rows(&state.config.draft, feed))
        })
    });
    let listing = dynamic(move || {
        let rows = rows.get();
        let chosen = chosen.clone();
        untracked(move || {
            view()
                .min_w(px(0.0))
                .gap(px(10.0))
                .children(
                    rows.into_iter()
                        .map(|shown| row(app, &shown, chosen.as_deref() == Some(&shown.name))),
                )
                .into_element()
        })
    })
    .into_element();
    let mut parts = vec![
        cell(app, extensions::NOTE, Token::Ink3, false).into_element(),
        guide(app),
        space(app, 10.0),
        listing,
    ];
    if let Some(chosen) = &listed.chosen {
        parts.push(space(app, 12.0));
        parts.push(h3(app, chosen.clone(), None));
        for shape in groups {
            parts.push(space(app, 10.0));
            parts.push(group(app, shape, live));
        }
    }
    view().min_w(px(0.0)).children(parts).into_element()
}

/// One extension: its name to press, then whether it is on and failing, what it runs, and its
/// last error as corrald reported it.
fn row(app: AppState, shown: &Row, chosen: bool) -> AnyElement {
    let name = shown.name.clone();
    let pick = name.clone();
    let mut lines = vec![
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .min_w(px(0.0))
            .child(button(
                app,
                name.clone(),
                if chosen {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Ghost
                },
                format!("seg-config-extensions-{name}"),
                move || {
                    let pick = pick.clone();
                    app.act(app.settings, move |state, _| {
                        state.choose_entry(Part::Extensions, pick)
                    });
                },
            ))
            .child(cell(
                app,
                shown.status(),
                if shown.failing() {
                    Token::Fail
                } else if shown.enabled {
                    Token::Ink2
                } else {
                    Token::Ink3
                },
                false,
            ))
            .into_element(),
        cell(
            app,
            format!("{} · {}", shown.shapes, shown.projects),
            Token::Ink3,
            false,
        )
        .into_element(),
    ];
    if let Some(error) = shown
        .health
        .as_ref()
        .and_then(|health| health.last_error.clone())
    {
        lines.push(cell(app, format!("Last error: {error}"), Token::Fail, false).into_element());
    }
    view()
        .test_id(&format!("config-extension-{name}"))
        .min_w(px(0.0))
        .gap(px(2.0))
        .children(lines)
        .into_element()
}

/// Opens the extensions guide outside the app.
fn guide(app: AppState) -> AnyElement {
    view()
        .flex_row()
        .child(button(
            app,
            extensions::GUIDE_LABEL,
            ButtonKind::Quiet,
            "btn-config-extensions-guide",
            || {
                drop(open_url(extensions::GUIDE_URL));
            },
        ))
        .into_element()
}
