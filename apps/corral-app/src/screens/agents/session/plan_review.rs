//! Plan review (`session::PlanDoc`, `session::PlanReview`): the plan a session in plan mode
//! submitted, opened from its line in the dock. The document takes the conversation's place,
//! drawn as Markdown at the prose measure under its title and facts, with where the read of the
//! whole document stands; the dock holds its card: the model an execute option runs on with
//! Change (the session's model list), the host's options, the one field for feedback or a note,
//! and Not now. Its keys work while the dock has them, as a docked choice's do.
//! test ids `plan-review`, `btn-plan-retry`, `dock-<card>`, `opt-<card>-<n>`, `btn-plan-model`,
//! `field-answer-<card>-0`, `btn-plan-send`, `btn-card-not-now`.

use std::cell::Cell;
use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::session::{Body, PlanDoc, PlanReview, SessionPage};
use crate::screens::markdown::{markdown, provide_styles};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button};
use crate::ui::kit::page::measure;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, MEASURE_PROSE, Token, color};

use super::super::run;
use super::cards::{foot_row, frame, hint, options_list, press, quiet, standing, tint, words};
use super::decision::text_row;

/// The plan document in the conversation's place, in its own scroll.
pub(super) fn document(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let doc = computed(move || {
        page.with(|p| match &p.body {
            Body::PlanReview(doc) => Some(doc.clone()),
            _ => None,
        })
    });
    let shown =
        dynamic(move || doc.with(|doc| doc.as_ref().map(|doc| untracked(|| content(app, doc)))));
    scroll()
        .test_id("plan-review")
        .flex_1()
        .min_h(px(0.0))
        .child(shown)
        .into_element()
}

fn content(app: AppState, doc: &PlanDoc) -> AnyElement {
    let form = app.form;
    let title = text(doc.title.clone())
        .role(AccessibilityRole::Header)
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    let title = match form {
        Form::Terminal => title,
        _ => title
            .font_family(FONT_TEXT)
            .text_size(px(17.0))
            .line_height(px(24.0)),
    };
    let facts = (!doc.facts.is_empty()).then(|| quiet(form, doc.facts.clone()));
    let note = doc.note.clone().map(|(hue, said)| {
        let retry = doc.retry.clone().map(|action| {
            button(
                app,
                "Retry",
                ButtonKind::Quiet,
                "btn-plan-retry",
                move || run(app, action.clone()),
            )
        });
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(if form == Form::Terminal { 16.0 } else { 8.0 }))
            .child(words(form, said, tint(hue)).flex_1().min_w(px(0.0)))
            .children(retry)
    });
    provide_styles(app);
    let body = doc.body.as_ref().map(|body| markdown(app, body, false));
    let column = view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .child(title)
        .children(facts)
        .children(note)
        .children((form == Form::Terminal).then(term::blank))
        .children(body);
    match form {
        Form::Terminal => column.px(term::cells(1.0)).into_element(),
        Form::Desktop => measure(
            app,
            MEASURE_PROSE,
            column.px(px(18.0)).py(px(14.0)).into_element(),
        ),
        Form::Phone => column.px(px(16.0)).py(px(12.0)).into_element(),
    }
}

/// The opened review's card in the dock (`dock.plan`), built once per card.
pub(super) fn choices(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let card = computed(move || page.with(|p| p.dock.plan.clone()));
    let id = computed(move || card.with(|c| c.as_ref().map(|c| c.card.clone())));
    dynamic(move || id.get().map(|_| untracked(|| review_card(app, page, card))))
}

fn review_card(
    app: AppState,
    page: Memo<SessionPage>,
    card: Memo<Option<PlanReview>>,
) -> AnyElement {
    let form = app.form;
    let test_id = untracked(|| card.with(|c| c.as_ref().map(|c| c.id.clone()).unwrap_or_default()));
    let model =
        computed(move || card.with(|c| c.as_ref().map(|c| (c.model.clone(), c.change.clone()))));
    let model = dynamic(move || {
        let (model, change) = model.get()?;
        Some(untracked(|| {
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(if form == Form::Terminal { 16.0 } else { 8.0 }))
                .child(words(form, model, color(Token::Ink2)))
                .child(button(
                    app,
                    "Change",
                    ButtonKind::Quiet,
                    "btn-plan-model",
                    move || run(app, change.clone()),
                ))
                .into_element()
        }))
    });
    let options =
        computed(move || card.with(|c| c.as_ref().map(|c| c.options.clone()).unwrap_or_default()));
    let keyed = computed(move || page.with(SessionPage::plan_option_keys));
    let armed = Rc::new(Cell::new(true));
    let rows = options_list(app, options, armed.clone(), Some(keyed), true, None);
    let row = computed(move || card.with(|c| c.as_ref().map(|c| c.words.clone())));
    let submit = computed(move || card.with(|c| c.as_ref().and_then(|c| c.submit.clone())));
    let asking = computed(move || card.with(|c| c.as_ref().is_some_and(|c| c.asking)));
    // Keep planning asks for the feedback: the field is drawn again with the keys on it.
    let field = dynamic(move || {
        let asking = asking.get();
        let armed = armed.clone();
        untracked(|| {
            row.get()
                .map(|first| text_row(app, first, row, armed, submit, asking))
        })
    });
    let state = computed(move || {
        card.with(|c| {
            c.as_ref()
                .map(|c| (c.read.clone(), c.sending, c.inert.clone()))
        })
    });
    let state = dynamic(move || {
        let (read, sending, inert) = state.get()?;
        untracked(|| standing(app, read, sending, inert.as_deref()))
    });
    let foot = computed(move || {
        card.with(|c| {
            c.as_ref().map(|c| {
                (
                    c.submit.clone().filter(|_| c.asking),
                    c.not_now.clone(),
                    c.hint.clone(),
                )
            })
        })
    });
    let foot = dynamic(move || {
        let (send, not_now, keys) = foot.get()?;
        let show_keys = keyed.get();
        Some(untracked(|| {
            let not_now = press(
                app,
                "Not now",
                ButtonKind::Quiet,
                "btn-card-not-now",
                not_now,
                ButtonOptions {
                    key_hint: Some("Esc"),
                    ..ButtonOptions::default()
                },
            );
            let send = send.map(|action| {
                press(
                    app,
                    "Send feedback",
                    ButtonKind::Primary,
                    "btn-plan-send",
                    action,
                    ButtonOptions::default(),
                )
            });
            foot_row(
                app,
                if show_keys { hint(app, &keys) } else { None },
                [not_now].into_iter().chain(send).collect(),
            )
        }))
    });
    frame(
        app,
        &test_id,
        move || card.with(|c| c.as_ref().map(|c| c.title.clone()).unwrap_or_default()),
        None,
        vec![model, rows, field, state, foot],
        false,
    )
}
