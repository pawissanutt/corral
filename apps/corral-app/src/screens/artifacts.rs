//! The artifact finder on every form: the filter, which keeps the keyboard focus (↑ ↓ and the
//! page keys walk the results from it, Enter opens the selected one full size), the results
//! newest first with each one's kind, title, task, project and date, and the selected
//! artifact's preview beside them, under them where the page is narrow (always on a phone).
//! A press on a result selects it; a press on the selected one opens it full size. Walking to
//! the last result asks corrald for the next page; Load older artifacts does the same for the
//! pointer. Every word, number and branch is `model::artifacts`'.
//!
//! test ids: "field-artifacts", "artifacts" (the results' scroll), "artifact-<reference>" (a
//! result), "artifacts-count", "btn-load-more", "btn-retry-artifacts", and those of `shown`.

mod browser;
mod save;
mod shown;

use nsh_core::element::color as for_appearance;
use nsh_core::keymap::When;
use nsh_core::prelude::*;

use corral_protocol::detail::DetailRef;

use crate::client::requests::{self, ClientRequest};
use crate::model::artifacts::{
    self, Action, FIELD_LABEL, HINT, LOAD_MORE, LOADING, LOADING_MORE, More, PLACEHOLDER, Page,
    RETRY, Row, SHOW, SUBTITLE, Status, TITLE, Walk,
};
use crate::model::{Effect, Form, Hue};
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, button, seg};
use crate::ui::kit::notice::notice;
use crate::ui::kit::page::{Width, columns, context, measure, measure_fill, measured_head, pane};
use crate::ui::kit::rows::{list_row_tall, revealing, titled, walked_row};
use crate::ui::kit::text::{dim, mono, plain};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, MEASURE_FORM, MEASURE_PAGE, Token, color, text_size};

/// The results column's width on desktop: a share of the page between 440 pt (`.split`'s first
/// column) and 640, so a wide window widens the results rather than only the preview; in a
/// terminal, where a Cell stands for 12 points, room for a result's columns and its title.
const LIST: Width = Width::Between {
    share: 0.30,
    min: 440.0,
    max: 640.0,
};
const TERMINAL_LIST: f32 = 680.0;
/// Terminal Cells of a result's kind, task, run, project and date columns.
const KIND_CELLS: f32 = 10.0;
const TASK_CELLS: f32 = 5.0;
const RUN_CELLS: f32 = 3.0;
const PROJECT_CELLS: f32 = 7.0;
const DATE_CELLS: f32 = 6.0;

/// Activity › Artifacts.
pub fn screen(app: AppState) -> AnyElement {
    let page = computed(move || {
        let state = app.artifacts.get();
        app.feed
            .with(|feed| artifacts::page(&state, feed, app.form))
    });
    keep_loaded(app, page);
    let full = computed(move || page.with(|page| page.full.is_some()));
    let downloads = save::downloads();
    let opens = browser::opens();
    dynamic(move || {
        if full.get() {
            untracked(|| shown::full(app, page, downloads, opens))
        } else {
            untracked(|| finder(app, page, downloads, opens))
        }
    })
    .into_element()
}

/// Keeps the filter's list, the shown artifact's content and, as the list is walked, its next
/// page requested.
fn keep_loaded(app: AppState, page: Memo<Page>) {
    let list = computed(move || {
        app.daemon().then(|| {
            app.artifacts
                .with(|state| app.feed.with(|feed| state.request(feed)))
        })
    });
    effect(move || {
        if let Some(list) = list.get() {
            untracked(|| app.load(list));
        }
    });
    // A cited artifact shown full size may be outside the filter's rows: its task's detail,
    // which lists it, stays loaded while it shows.
    let owner = computed(move || {
        app.artifacts.with(|state| {
            state
                .full
                .as_deref()
                .and_then(corral_types::Reference::parse)
                .map(|cited| vec![crate::model::work::task_ref(cited.task())])
                .unwrap_or_default()
        })
    });
    crate::screens::task::keep::keep_details(app, owner, None);
    let shown = computed(move || {
        page.with(|page| {
            page.full
                .as_ref()
                .or(page.preview.as_ref())
                .map(|shown| shown.read.clone())
        })
    });
    // The shown artifact is read only to its head; one no longer shown gives its head up, so a
    // view that needs all of it (a review's gate log) reads it whole.
    let headed: std::rc::Rc<std::cell::RefCell<Option<DetailRef>>> = Default::default();
    let head = move |target: DetailRef, on: bool| {
        _ = requests::send(ClientRequest::ArtifactHead { target, on });
    };
    {
        let headed = headed.clone();
        effect(move || {
            let target = shown
                .get()
                .map(|reference| artifacts::content_target(&reference));
            let previous = headed.replace(target.clone());
            untracked(|| {
                if let Some(previous) =
                    previous.filter(|previous| Some(previous) != target.as_ref())
                {
                    head(previous, false);
                }
                if let Some(target) = target {
                    head(target.clone(), true);
                    app.load(target);
                }
            });
        });
    }
    cleanup(move || {
        if let Some(target) = headed.take() {
            head(target, false);
        }
    });
    let more = computed(move || {
        let state = app.artifacts.get();
        app.feed.with(|feed| artifacts::more(&state, feed))
    });
    effect(move || {
        if let Some(next) = more.get() {
            untracked(|| app.run(vec![Effect::Detail(next)]));
        }
    });
}

/// The finder: the head, the filter, then the results and the preview.
fn finder(
    app: AppState,
    page: Memo<Page>,
    downloads: save::Downloads,
    opens: browser::Opens,
) -> AnyElement {
    let form = app.form;
    if form == Form::Phone {
        let content = view()
            .child(field_bar(app, page))
            .child(results(app, page))
            .child(context(
                app,
                "artifact-side",
                shown::preview(app, page, downloads, opens),
            ))
            .into_element();
        return pane(app, "artifacts", content);
    }
    let cols = columns(
        app,
        vec![
            (
                if form == Form::Terminal {
                    Width::Fixed(TERMINAL_LIST)
                } else {
                    LIST
                },
                pane(app, "artifacts", results(app, page)),
            ),
            (
                Width::Fill,
                context(
                    app,
                    "artifact-side",
                    shown::preview(app, page, downloads, opens),
                ),
            ),
        ],
        1,
    );
    // On a desktop the filter and the columns keep to the page's measure under the head, as
    // every Activity page does; the filter's rule runs edge to edge as the head's does.
    let filter = match form {
        Form::Desktop => view()
            .flex_none()
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(measure(app, MEASURE_PAGE, field_bar(app, page)))
            .into_element(),
        Form::Phone | Form::Terminal => field_bar(app, page),
    };
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(measured_head(
            app,
            MEASURE_PAGE,
            TITLE,
            Some(SUBTITLE.to_owned()),
            Vec::new(),
        ))
        .child(filter)
        .when(form == Form::Terminal, |page| page.child(term::hrule()))
        .child(measure_fill(app, MEASURE_PAGE, cols))
        .into_element()
}

/// The filter's row: the field, then how many results show; on desktop and phone the tokens'
/// hint under it.
fn field_bar(app: AppState, page: Memo<Page>) -> AnyElement {
    let form = app.form;
    let count = text(move || page.with(|page| page.results.count.clone()))
        .test_id("artifacts-count")
        .number_of_lines(1u32)
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(color(Token::Ink3));
    match form {
        Form::Terminal => view()
            .flex_row()
            .items_center()
            .h(term::rows(1.0))
            .px(term::cells(1.0))
            .child(
                view()
                    .w(term::cells(2.0))
                    .flex_shrink_0()
                    .child(text("›").text_color(color(Token::Ink3))),
            )
            .child(field(app))
            .child(term::space(2.0))
            .child(count)
            .child(term::space(2.0))
            .child(show(app, page))
            .into_element(),
        Form::Desktop | Form::Phone => {
            let small = if form == Form::Phone { 12.5 } else { 11.5 };
            view()
                .flex_none()
                .gap(px(6.0))
                .px(px(18.0))
                .py(px(10.0))
                // The field keeps a form's width, so the count stays beside what was typed.
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .max_w(px(MEASURE_FORM))
                        .gap(px(10.0))
                        .child(field(app))
                        .child(count.font_family(FONT_TEXT).text_size(px(small)))
                        .child(show(app, page)),
                )
                .child(
                    text(HINT)
                        .max_w(px(MEASURE_FORM))
                        .font_family(FONT_TEXT)
                        .text_size(px(small))
                        .text_color(color(Token::Ink3)),
                )
                .into_element()
        }
    }
}

/// The All switch: the owner's artifacts, or submissions, records and replaced ones too
/// (test ids "seg-artifacts-0", "seg-artifacts-1").
fn show(app: AppState, page: Memo<Page>) -> AnyElement {
    seg(
        app,
        &SHOW,
        move || page.with(|page| page.all),
        move |all| app.artifacts_act(Action::All(all)),
        "seg-artifacts",
    )
}

/// The filter field. While it has the keyboard focus ↑ ↓ and Page Up/Down walk the results and
/// Enter opens the selected one full size; it takes the focus back whenever the finder asks
/// (a press on a result, the way back from full size, Find in all artifacts).
fn field(app: AppState) -> AnyElement {
    let form = app.form;
    let handle = element_ref();
    let focused = signal(false);
    effect(move || {
        if !focused.get() {
            return;
        }
        let walking = KeyOptions {
            when: When::Always,
            repeat: true,
            ..KeyOptions::default()
        };
        for (keys, walk) in [
            ("up", Walk::Up),
            ("down", Walk::Down),
            ("pageup", Walk::PageUp),
            ("pagedown", Walk::PageDown),
        ] {
            on_key_with(keys, walking, move |_| {
                app.artifacts_act(Action::Walk(walk))
            });
        }
        let once = KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        };
        on_key_with("enter", once, move |_| {
            app.artifacts_act(Action::OpenSelected)
        });
    });
    if form != Form::Phone {
        let asked = computed(move || app.artifacts.with(|state| state.focus));
        effect(move || {
            let _ = asked.get();
            after_answer(move || handle.focus());
        });
    }
    let displayed = move || app.artifacts.with(|state| state.query.clone());
    let entry = copyable_input(app, Some(handle), displayed)
        .test_id("field-artifacts")
        .label(FIELD_LABEL)
        .placeholder(PLACEHOLDER)
        .on_change(move |typed: &str| app.artifacts_act(Action::Query(typed.to_owned())))
        .on_focus(move || {
            focused.set(true);
            app.artifacts.update(|state| state.typing = true);
        })
        .on_blur(move || {
            focused.set(false);
            app.artifacts.update(|state| state.typing = false);
        })
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || {
            for_appearance(color(if form == Form::Terminal {
                Token::Ink3
            } else {
                Token::Ink4
            }))
        })
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(AdaptiveColor::TRANSPARENT);
    match form {
        Form::Terminal => entry.h(term::rows(1.0)).into_element(),
        Form::Desktop | Form::Phone => {
            let (height, size) = if form == Form::Phone {
                (44.0, 16.0)
            } else {
                (32.0, text_size(form))
            };
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .h(px(height))
                .px(px(10.0))
                .rounded(px(if form == Form::Phone { 10.0 } else { 6.0 }))
                .border_1()
                .border_color(color(Token::Rule2))
                .bg(color(Token::Surface))
                .focus_within(|s| s.border_color(color(Token::Focus)))
                .child(icon(Icon::Search, 14.0, color(Token::Ink3)))
                .child(
                    entry
                        .h(px(height))
                        .font_family(FONT_TEXT)
                        .text_size(px(size)),
                )
                .into_element()
        }
    }
}

/// The results, then the list's state under them: loading, why it failed with Retry, that
/// nothing matches, or the next page.
fn results(app: AppState, page: Memo<Page>) -> AnyElement {
    let rows = computed(move || page.with(|page| page.results.rows.clone()));
    let selected = computed(move || page.with(|page| page.results.selected.clone()));
    let state = computed(move || {
        page.with(|page| {
            (
                page.results.status.clone(),
                page.results.empty.clone(),
                page.results.more,
            )
        })
    });
    view()
        .role(AccessibilityRole::List)
        .label(TITLE)
        .child(dynamic(move || {
            let rows = rows.get();
            untracked(|| {
                view()
                    .children(rows.iter().map(|row| result(app, row, selected)))
                    .into_element()
            })
        }))
        .child(dynamic(move || {
            let (status, empty, more) = state.get();
            untracked(|| below(app, status, empty, more))
        }))
        .into_element()
}

/// What stands under the results.
fn below(app: AppState, status: Status, empty: Option<String>, more: More) -> AnyElement {
    let pad = |part: AnyElement| match app.form {
        Form::Terminal => view()
            .flex_row()
            .px(term::cells(1.0))
            .child(part)
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .flex_row()
            .px(px(16.0))
            .py(px(10.0))
            .child(part)
            .into_element(),
    };
    let line = match status {
        Status::Loading => Some(dim(app, LOADING).into_element()),
        Status::Failed(why) => {
            let retry = button(
                app,
                RETRY,
                ButtonKind::Ghost,
                "btn-retry-artifacts",
                move || {
                    let list = app.artifacts.with_untracked(|state| {
                        app.feed.with_untracked(|feed| state.request(feed))
                    });
                    app.refresh(list);
                },
            );
            Some(notice(
                app,
                Hue::Fail,
                &vec![crate::model::span(why)],
                Some(retry),
            ))
        }
        Status::Ready => empty.map(|empty| dim(app, empty).into_element()),
    };
    let more = match more {
        More::Done => None,
        More::Loading => Some(dim(app, LOADING_MORE).into_element()),
        More::Offer => Some(button(
            app,
            LOAD_MORE,
            ButtonKind::Quiet,
            "btn-load-more",
            move || app.artifacts_act(Action::LoadMore),
        )),
    };
    view()
        .children(line.into_iter().chain(more).map(pad))
        .into_element()
}

/// One result: on desktop and in a terminal its kind, title (with the project's chip), task and
/// date on one line; on a phone the title over the rest.
fn result(app: AppState, row: &Row, selected: Memo<Option<String>>) -> AnyElement {
    let form = app.form;
    let me = row.reference.clone();
    let is = computed(move || selected.with(|selected| selected.as_deref() == Some(me.as_str())));
    let reference = row.reference.clone();
    let press = move || app.artifacts_act(Action::Press(reference.clone()));
    let test_id = format!("artifact-{}", row.reference);
    if form == Form::Phone {
        let facts = [
            Some(row.kind.as_str()),
            Some(row.task.as_str()),
            row.run.as_deref(),
            row.project.as_deref(),
            Some(row.at.as_str()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let content = vec![
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(plain(app, row.shown_title.clone()).truncate())
                .child(dim(app, facts).truncate())
                .into_element(),
        ];
        return list_row_tall(
            app,
            row.label.clone(),
            test_id,
            move || is.get(),
            press,
            content,
        );
    }
    let terminal = form == Form::Terminal;
    let fixed = |words: Text, cells: f32, points: f32| {
        let words = words.number_of_lines(1u32).flex_shrink_0();
        if terminal {
            words.w(term::cells(cells)).into_element()
        } else {
            words.w(px(points)).into_element()
        }
    };
    let content = vec![
        fixed(dim(app, row.kind.clone()), KIND_CELLS, 74.0),
        titled(app, plain(app, row.shown_title.clone()).truncate(), None),
        fixed(
            mono(app, row.task.to_string()).text_color(color(Token::Ink2)),
            TASK_CELLS,
            44.0,
        ),
        fixed(
            dim(app, row.run.clone().unwrap_or_default()),
            RUN_CELLS,
            28.0,
        ),
        fixed(
            dim(app, row.project.clone().unwrap_or_default()).truncate(),
            PROJECT_CELLS,
            64.0,
        ),
        fixed(dim(app, row.at.clone()).text_right(), DATE_CELLS, 52.0),
    ];
    revealing(
        is,
        walked_row(
            app,
            row.label.clone(),
            test_id,
            move || is.get(),
            press,
            content,
        ),
    )
}
