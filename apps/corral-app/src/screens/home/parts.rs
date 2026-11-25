//! What the dashboard's tiles, cards and panels share: the panel frame with its head, a
//! pressable line that opens the page holding its detail, words at the dashboard's sizes and a
//! bar drawn as a share of its track; Ask about the project in scope, in every form's head; and
//! the watcher that keeps a Blocked task's Pi run state coming while its card or row shows.

use nsh_core::prelude::*;

use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::projects::Scope;
use crate::model::{Form, Id, home};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::hint::hinted;
use crate::ui::kit::marks::hue_fill;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// While task `id`'s card or row shows, watches its Blocked Pi run (`home::watched_run`), so
/// the host's state says whether it asks an owner question Answer can open. Released when the
/// row goes or the run stops waiting, through the source it was opened on.
pub(super) fn watch_run(app: AppState, id: Id) {
    let run = computed(move || app.feed.with(|feed| home::watched_run(feed, &id)));
    let scoped = app.scope();
    effect(move || {
        let Some(session) = run.get() else {
            return;
        };
        untracked(|| {
            scoped.run(vec![Effect::Watch {
                session: session.clone(),
                on: true,
            }]);
        });
        cleanup(move || scoped.run(vec![Effect::Watch { session, on: false }]));
    });
}

/// Ask opened with the project in scope as its context (`btn-ask-project`); none with every
/// project in scope.
pub(super) fn ask_project(app: AppState, scope: &Scope) -> Option<AnyElement> {
    let project = scope.project()?.clone();
    let ask = button_with(
        app,
        home::ASK_PROJECT,
        ButtonKind::Ghost,
        ButtonOptions {
            icon: Some(Icon::Ask),
            ..ButtonOptions::default()
        },
        "btn-ask-project",
        move || app.ask_project(project.clone()),
    );
    Some(hinted(app, ask, home::ASK_PROJECT_HINT))
}

/// Words on one line, cut at their edge: `size` points on the desktop, the terminal's own.
pub(super) fn words(form: Form, content: impl Into<SharedString>, size: f32, ink: Token) -> Text {
    let words = text(content.into())
        .number_of_lines(1u32)
        .truncate()
        .min_w(px(0.0))
        .text_color(color(ink));
    match form {
        Form::Terminal => words,
        Form::Desktop | Form::Phone => words.font_family(FONT_TEXT).text_size(px(size)),
    }
}

/// An id, a figure or a time in the mono font, never cut.
pub(super) fn figure(form: Form, content: impl Into<SharedString>, size: f32, ink: Token) -> Text {
    let words = text(content.into())
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(color(ink));
    match form {
        Form::Terminal => words,
        Form::Desktop | Form::Phone => words
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(size)),
    }
}

/// The page a panel's head links to: its words and where they go.
pub(crate) struct Link {
    pub words: String,
    pub test_id: String,
    pub press: Box<dyn Fn()>,
}

/// One panel (`.panel`): its name and count, a quiet note or link at the trailing edge, then
/// its body. Desktop: surface, a rule line, radius 8. Terminal: a framed region with the name
/// set into its top line and the link, when there is one, after it.
pub(crate) fn panel(
    app: AppState,
    test_id: &str,
    name: String,
    note: Option<String>,
    link: Option<Link>,
    body: AnyElement,
) -> AnyElement {
    let form = app.form;
    let link = link.map(|link| {
        let words = text(link.words)
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3));
        let words = match form {
            Form::Terminal => words,
            Form::Desktop | Form::Phone => words
                .font_family(FONT_TEXT)
                .text_size(px(12.0))
                .underline()
                .text_decoration_color(color(Token::Ink3).opacity(0.5))
                .underline_offset(px(3.0)),
        };
        let press = link.press;
        view()
            .test_id(&link.test_id)
            .role(AccessibilityRole::Link)
            .cursor_pointer()
            .flex_shrink_0()
            .on_press(press)
            .child(words)
            .into_element()
    });
    let title = text(name)
        .role(AccessibilityRole::Header)
        .number_of_lines(1u32)
        .flex_shrink_0()
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    match form {
        // The title sits in the frame's line, which does not cut what runs past it: the note
        // stays out, the link is short.
        Form::Terminal => term::framed(view().px(term::cells(1.0)).child(body))
            .test_id(test_id)
            .child(term::top_title(
                view()
                    .flex_row()
                    .gap(term::cells(2.0))
                    .child(title)
                    .children(link),
            ))
            .into_element(),
        Form::Desktop | Form::Phone => surface(test_id)
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(12.0))
                    .pt(px(8.0))
                    .pb(px(7.0))
                    .child(title.font_family(FONT_TEXT).text_size(px(12.5)))
                    .child(view().flex_1().min_w(px(0.0)).children(note.map(|note| {
                        words(form, note, 12.0, Token::Ink3)
                            .text_right()
                            .into_element()
                    })))
                    .children(link),
            )
            .child(body)
            .into_element(),
    }
}

/// A panel's ground on the desktop: surface, a rule line, radius 8.
pub(super) fn surface(test_id: &str) -> View {
    view()
        .test_id(test_id)
        // On the board a panel fills its row's height, so a row's widgets end level.
        .flex_grow(1.0)
        .min_w(px(0.0))
        .bg(color(Token::Surface))
        .border_1()
        .border_color(color(Token::Rule))
        .rounded(px(8.0))
        .overflow_hidden()
}

/// A line of a panel that opens what it names: a row on the desktop with a hover, a row of
/// Cells in a terminal with the focus mark.
pub(super) fn line(
    app: AppState,
    test_id: String,
    spoken: String,
    children: Vec<AnyElement>,
    press: impl Fn() + 'static,
) -> AnyElement {
    let form = app.form;
    let row = view()
        .test_id(&test_id)
        .role(AccessibilityRole::Button)
        .label(spoken)
        .cursor_pointer()
        .flex_row()
        .items_center()
        .min_w(px(0.0))
        .on_press(press);
    match form {
        Form::Terminal => {
            let handle = element_ref();
            let focused = term::focus_of(handle);
            row.element_ref(&handle)
                .gap(term::cells(1.0))
                .child(term::focus_cell(focused))
                .children(children)
                .into_element()
        }
        Form::Desktop | Form::Phone => row
            .gap(px(8.0))
            .min_h(px(26.0))
            .px(px(12.0))
            .py(px(3.0))
            .border_t_1()
            .border_color(color(Token::Rule))
            .hover(|s| s.bg(color(Token::Hover)))
            .children(children)
            .into_element(),
    }
}

/// A panel's quiet line where it has nothing to list.
pub(super) fn empty(app: AppState, words_said: impl Into<SharedString>) -> AnyElement {
    let form = app.form;
    let said = words(form, words_said, 12.5, Token::Ink3);
    match form {
        // A frame around one row of words draws no border: a blank row keeps it.
        Form::Terminal => view().child(said).child(term::blank()).into_element(),
        Form::Desktop | Form::Phone => view()
            .px(px(12.0))
            .pt(px(10.0))
            .pb(px(14.0))
            .border_t_1()
            .border_color(color(Token::Rule))
            .child(said)
            .into_element(),
    }
}

/// A bar `fraction` of its track long in `hue`, the track `width` wide (`.fbar`).
pub(super) fn bar(form: Form, fraction: f32, hue: Hue, width: Points) -> AnyElement {
    let height = match form {
        Form::Terminal => term::rows(1.0),
        Form::Desktop | Form::Phone => px(6.0),
    };
    view()
        .relative()
        .w(width)
        .flex_shrink_0()
        .h(height)
        .rounded(px(3.0))
        .when(form != Form::Terminal, |track| track.bg(color(Token::Rule)))
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .bottom(px(0.0))
                .w(relative(fraction.clamp(0.0, 1.0)))
                .rounded(px(3.0))
                .bg(hue_fill(hue)),
        )
        .into_element()
}
