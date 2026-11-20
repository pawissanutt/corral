//! The session page's Details panel (`session::Details`): its head (`Details`, ✕), then each
//! section the host serves: a header that folds it (caret, name, summary in its tone, `Open` for
//! the part's full view), and unfolded, why the host cannot serve it now, where its read stands
//! and its rows. Beside the conversation on a wide page (desktop 350 pt, terminal 36 Cells), in
//! the conversation's place on a narrow one, and a bottom sheet on the phone. The Plan ledger and
//! Host events sheets are mounted with the page (`sheets`). test ids `session-details`,
//! `details-<section>`, `btn-details-<section>`, `btn-details-close`, `session-details-sheet`.

mod context;
mod files;

use nsh_core::prelude::*;

use crate::model::hue::Hue;
use crate::model::session::{
    Cx, DetailSection, Details, Facts, ReadState, SectionContent, SessionPage,
};
use crate::model::text::span;
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::notice::notice;
use crate::ui::kit::overlay::{self, Modal};
use crate::ui::kit::text::dim;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::super::run;
use super::{events, hue_words, plan};

/// The panel's width beside the conversation: points on desktop, Cells in a terminal.
pub(super) const PANEL: f32 = 350.0;
pub(super) const PANEL_CELLS: f32 = 36.0;

fn details_of(page: Memo<SessionPage>) -> Memo<Option<Details>> {
    computed(move || page.with(|p| p.details.clone()))
}

/// Words and captions set in from the edge the rows run to, as the form spaces them.
pub(super) fn inset(app: AppState, child: impl IntoElement) -> AnyElement {
    let side = view().min_w(px(0.0));
    match app.form {
        Form::Terminal => side.px(term::cells(1.0)),
        Form::Desktop => side.px(px(14.0)),
        Form::Phone => side.px(px(18.0)),
    }
    .child(child)
    .into_element()
}

/// The Plan ledger and Host events sheets of session `id`, mounted once with its page.
pub(super) fn sheets(app: AppState, id: Id) -> AnyElement {
    view()
        .child(plan::sheet(app, id.clone()))
        .child(events::sheet(app, id))
        .into_element()
}

/// What `read` makes of session `id` as the model reads it now (tracked): for the sheets, which
/// open over the page whether or not Details shows.
pub(super) fn with_cx<T>(app: AppState, id: &Id, read: impl Fn(&Cx<'_>) -> T) -> T {
    let env = app.env();
    app.feed.with(|feed| {
        app.session.with(|state| {
            let facts = Facts::of(feed, id);
            let cx = Cx::new(feed, state.ui(id.as_str()), id, env, &facts);
            read(&cx)
        })
    })
}

/// The panel framed beside the conversation (`beside`), or filling the conversation's place.
pub(super) fn panel(app: AppState, page: Memo<SessionPage>, beside: bool) -> AnyElement {
    let form = app.form;
    let body = content(app, page);
    if form == Form::Terminal {
        let column = view().flex_col().min_h(px(0.0)).child(body);
        return if beside {
            view()
                .flex_row()
                .flex_none()
                .min_h(px(0.0))
                .child(term::vrule())
                .child(column.w(term::cells(PANEL_CELLS)))
                .into_element()
        } else {
            column.flex_1().min_w(px(0.0)).into_element()
        };
    }
    let column = view()
        .flex_col()
        .min_h(px(0.0))
        .bg(color(Token::Surface))
        .child(body);
    if beside {
        column
            .w(px(PANEL))
            .flex_none()
            .border_l(px(1.0))
            .border_l_color(color(Token::Rule))
            .into_element()
    } else {
        column.flex_1().min_w(px(0.0)).into_element()
    }
}

/// The phone's sheet: Details over the page while it is open.
pub(super) fn sheet(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let details = details_of(page);
    let toggle = computed(move || details.with(|d| d.as_ref().map(|d| d.toggle.clone())));
    overlay::modal(
        app,
        move || details.with(|d| d.as_ref().is_some_and(|d| d.shown)),
        move || {
            if let Some(action) = toggle.get_untracked() {
                run(app, action);
            }
        },
        move || Modal {
            test_id: "session-details-sheet".into(),
            title: "Details".into(),
            sub: None,
            body: view()
                .test_id("session-details")
                .child(sections(app, details))
                .into_element(),
            foot: None,
            done: Some("Done".into()),
            confirm: None,
        },
    )
}

/// The head (`Details`, ✕) over the sections, which scroll.
fn content(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let form = app.form;
    let details = details_of(page);
    let toggle = computed(move || details.with(|d| d.as_ref().map(|d| d.toggle.clone())));
    let close = dynamic(move || {
        toggle.get().map(|action| {
            untracked(|| {
                button(
                    app,
                    "✕",
                    ButtonKind::Quiet,
                    "btn-details-close",
                    move || run(app, action.clone()),
                )
            })
        })
    });
    let title = text("Details")
        .role(AccessibilityRole::Header)
        .number_of_lines(1u32)
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    let head = view()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(8.0))
        .child(match form {
            Form::Terminal => title.into_element(),
            _ => title
                .font_family(FONT_TEXT)
                .text_size(px(13.0))
                .into_element(),
        })
        .child(view().flex_1().min_w(px(0.0)))
        .child(close);
    let head = match form {
        Form::Terminal => head.px(term::cells(1.0)),
        _ => head.h(px(38.0)).px(px(14.0)),
    };
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(head)
        .child(
            scroll()
                .test_id("session-details")
                .flex_1()
                .min_h(px(0.0))
                .child(sections(app, details)),
        )
        .into_element()
}

/// Every section. A section is built again only when it folds or unfolds; its words and rows
/// follow their own facts, so its controls stay put while its read ages.
fn sections(app: AppState, details: Memo<Option<Details>>) -> AnyElement {
    let keys = computed(move || {
        details.with(|d| {
            d.as_ref()
                .map(|d| d.sections.iter().map(|s| s.section).collect::<Vec<_>>())
                .unwrap_or_default()
        })
    });
    dynamic(move || {
        let keys = keys.get();
        untracked(|| {
            view()
                .children(
                    keys.into_iter()
                        .map(|key| {
                            let section = computed(move || {
                                details.with(|d| {
                                    d.as_ref().and_then(|d| {
                                        d.sections.iter().find(|s| s.section == key).cloned()
                                    })
                                })
                            });
                            let folded = computed(move || {
                                section.with(|s| s.as_ref().map(|s| s.folded))
                            });
                            dynamic(move || {
                                folded
                                    .get()
                                    .map(|folded| untracked(|| face(app, section, folded)))
                            })
                        })
                        .collect::<Vec<_>>(),
                )
                .into_element()
        })
    })
}

/// One section: its header, and unfolded its frame and rows.
fn face(app: AppState, shown: Memo<Option<DetailSection>>, folded: bool) -> AnyElement {
    let form = app.form;
    let Some(section) = shown.get_untracked() else {
        return view().into_element();
    };
    let slug = section.section.slug();
    let caret = text(if folded { "▸" } else { "▾" })
        .flex_shrink_0()
        .text_color(color(Token::Ink3));
    let label = text(section.label)
        .number_of_lines(1u32)
        .flex_shrink_0()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(Token::Ink));
    let words = computed(move || {
        shown.with(|s| {
            s.as_ref()
                .map(|s| (s.body.summary.clone(), s.body.hue))
                .unwrap_or_else(|| (String::new(), Hue::Neutral))
        })
    });
    let summary = text(move || words.with(|(said, _)| said.clone()))
        .number_of_lines(1u32)
        .truncate()
        .flex_1()
        .min_w(px(0.0))
        .text_color(move || words.with(|(_, hue)| hue_words(form, *hue)));
    let (caret, label, summary) = match form {
        Form::Terminal => (caret, label, summary),
        _ => (
            caret.font_family(FONT_TEXT).text_size(px(11.0)),
            label.font_family(FONT_TEXT).text_size(px(12.5)),
            summary.font_family(FONT_TEXT).text_size(px(12.0)),
        ),
    };
    let fold = section.fold.clone();
    let name = section.label;
    let spoken = move || {
        words.with(|(said, _)| {
            format!(
                "{name}, {said}, {}",
                if folded { "folded" } else { "open" }
            )
        })
    };
    let header = view()
        .test_id(&section.id)
        .role(AccessibilityRole::Button)
        .label(spoken)
        .accessibility_state(move || AccessibilityState {
            expanded: Some(!folded),
            ..AccessibilityState::default()
        })
        .flex_row()
        .flex_1()
        .min_w(px(0.0))
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .on_press(move || run(app, fold.clone()))
        .child(caret)
        .child(label)
        .child(summary);
    let open = section.view.clone().map(|action| {
        button(
            app,
            "Open",
            ButtonKind::Quiet,
            format!("btn-details-{slug}"),
            move || run(app, action.clone()),
        )
    });
    let line = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(header)
        .children(open);
    let line = match form {
        Form::Terminal => line.px(term::cells(1.0)),
        Form::Desktop => line.min_h(px(30.0)).px(px(14.0)),
        Form::Phone => line.min_h(px(44.0)).px(px(18.0)),
    };
    // The frame follows what it says, not the read's age, which the frame does not show.
    let body = (!folded).then(|| {
        let facts = computed(move || {
            shown.with(|s| {
                s.as_ref().map(|s| {
                    let mut said = s.clone();
                    if matches!(said.body.read, ReadState::Read { .. }) {
                        said.body.read = ReadState::NotRead;
                    }
                    said
                })
            })
        });
        dynamic(move || {
            facts
                .with(|s| s.as_ref().map(|s| untracked(|| frame(app, s))))
        })
    });
    let column = view().child(line).children(body);
    match form {
        Form::Terminal => column.into_element(),
        _ => column
            .pb(px(6.0))
            .border_t(px(1.0))
            .border_color(color(Token::Rule))
            .into_element(),
    }
}

/// An unfolded section's body: why the host cannot serve it now, where its read stands, then
/// its rows.
fn frame(app: AppState, section: &DetailSection) -> AnyElement {
    let body = &section.body;
    let unavailable = body.unavailable.clone().map(|why| {
        inset(
            app,
            notice(
                app,
                Hue::Held,
                &vec![span(format!("unavailable: {why}"))],
                None,
            ),
        )
    });
    let read = match &body.read {
        ReadState::Reading | ReadState::ReadingKept | ReadState::Stale { .. }
        | ReadState::Failed(_) => {
            let retry = body.reread.clone().map(|action| {
                button(
                    app,
                    "Retry",
                    ButtonKind::Quiet,
                    format!("btn-reread-details-{}", section.section.slug()),
                    move || run(app, action.clone()),
                )
            });
            Some(inset(
                app,
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        dim(app, body.read.words())
                            .text_color(hue_words(app.form, body.read.hue()))
                            .flex_1()
                            .min_w(px(0.0)),
                    )
                    .children(
                        retry.filter(|_| matches!(body.read, ReadState::Failed(_))),
                    ),
            ))
        }
        ReadState::NotRead | ReadState::Read { .. } | ReadState::DoesNotApply(_) => None,
    };
    let rows = section.content.as_ref().map(|content| match content {
        SectionContent::Files(tab) => files::files(app, tab),
        SectionContent::Agents(tab) => context::agents(app, tab),
        SectionContent::Context(tab) => context::context(app, tab),
        SectionContent::Plan(tab) => plan::tab(app, tab),
        SectionContent::Task(words, action) => {
            let action = action.clone();
            inset(
                app,
                button(
                    app,
                    words.clone(),
                    ButtonKind::Ghost,
                    "btn-details-task-page",
                    move || run(app, action.clone()),
                ),
            )
        }
    });
    view()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 4.0 }))
        .children(unavailable)
        .children(read)
        .children(rows)
        .into_element()
}
