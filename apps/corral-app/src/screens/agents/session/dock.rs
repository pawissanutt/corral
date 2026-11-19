//! Above the composer, each only while it has content, in the terminal UI's order
//! (`session::Dock`): the docked choice the owner opened (a build's exit choice, a child's
//! question, a proposed board plan), every other waiting choice as one line (`▲ docs asks: … ·
//! Alt+A to answer`, up to three, then `+n more`) that opens it when pressed; where a chosen exit
//! stands; the owner questions; the queue, one header line that unfolds to its rows with small
//! Edit, ↑, ↓ and Remove controls (a `⋯` menu on the phone); where the read of the waiting cards
//! stands and why they cannot be answered from here; and the sidebar's summary row while the rail
//! is not beside the main view and no choice is open. The dock never takes more than 45 % of the
//! window's height, nor so much that the tab's content keeps less than 240 pt: past that it
//! scrolls. Also the bar standing in the composer's place while the session takes no message,
//! with Resume or Relaunch where the host can start again.
//! test ids `session-dock`, `dock-<card>`, `session-queue`, `btn-queue-toggle`, `queue-<n>`,
//! `btn-queue-<edit|up|down|remove>-<n>`, `btn-cards-retry`.

use nsh_base::{menu, menu_item};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{
    Dock, Focus, HostBar, QuestionDock, QueueControl, QueueDock, QueueItem, SessionPage,
};
use crate::screens::plans::menu_trigger;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::overlay::counted;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::super::run;
use super::header::popup;
use super::{cards, exit, sidebar};

/// The phone's least touch target.
const TARGET: f32 = 44.0;
/// The dock's share of the window's height, at most.
const SHARE: f32 = 0.45;
/// What the tab's content keeps at least, and what the page's head, tabs, composer and status
/// take besides the dock.
const CONTENT_MIN: f32 = 240.0;
const FRAME: f32 = 300.0;
/// The least the dock may scroll within.
const DOCK_MIN: f32 = 96.0;

/// The dock: its items stay mounted and follow their own parts; the whole is hidden while none
/// shows. `rail` says the sidebar is beside the main view, so its summary row is not here.
pub(super) fn dock(app: AppState, page: Memo<SessionPage>, rail: Memo<bool>) -> AnyElement {
    let form = app.form;
    let dock = computed(move || page.with(|p| p.dock.clone()));
    let row = computed(move || {
        !rail.get() && page.with(|p| p.sidebar.is_some()) && dock.with(|d| d.choice.is_none())
    });
    let any = computed(move || {
        row.get()
            || page.with(|p| p.exit.is_some())
            || dock.with(|d| !d.is_empty() || d.read.is_some() || d.inert.is_some())
    });
    let side = dynamic(move || row.get().then(|| untracked(|| sidebar::summary(app, page))));
    let column = view()
        .test_id("session-dock")
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(cards::choice(app, page))
        .child(waiting(app, dock))
        .child(exit::progress(app, page))
        .child(questions(app, dock))
        .child(queue(app, page))
        .child(reading(app, dock))
        .child(inert(app, dock))
        .child(side);
    let column = match form {
        Form::Terminal => column.px(term::cells(1.0)),
        Form::Desktop => column.px(px(18.0)).pt(px(8.0)).pb(px(2.0)),
        Form::Phone => column.px(px(16.0)).pt(px(8.0)),
    };
    // An opened card and the queue scroll within the dock's share of the window, so the tab's
    // content keeps its room.
    let shown = scroll()
        .max_h(move || {
            let height = window().height.0;
            px((height * SHARE)
                .min(height - CONTENT_MIN - FRAME)
                .max(DOCK_MIN))
        })
        .child(column);
    let shell = match form {
        Form::Terminal => view().child(term::hrule()).child(shown),
        Form::Desktop => view()
            .border_t(px(1.0))
            .border_color(color(Token::Rule))
            .child(shown),
        Form::Phone => view().child(shown),
    };
    shell
        .flex_none()
        .style_when(move || !any.get(), |s| s.hidden())
        .into_element()
}

/// Words at the dock's size in `ink`, one line.
fn words(form: Form, content: String, ink: AdaptiveColor) -> Text {
    let words = text(content)
        .number_of_lines(1u32)
        .truncate()
        .text_color(ink);
    match form {
        Form::Terminal => words,
        Form::Desktop => words.font_family(FONT_TEXT).text_size(px(12.5)),
        Form::Phone => words.font_family(FONT_TEXT).text_size(px(14.0)),
    }
}

/// A head in quiet spaced capitals: `Queue · 2 waiting`.
fn head(form: Form, content: String) -> Text {
    let head = text(content)
        .number_of_lines(1u32)
        .truncate()
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink3));
    match form {
        Form::Terminal => head,
        _ => head
            .font_family(FONT_TEXT)
            .text_size(px(11.0))
            .letter_spacing(px(0.4)),
    }
}

/// The docked choices not opened, one line each (`▲ docs asks: … · Alt+A to answer`), then
/// `+n more`; pressing a line opens its card.
fn waiting(app: AppState, dock: Memo<Dock>) -> AnyElement {
    let form = app.form;
    let lines = computed(move || dock.with(|d| (d.waiting.clone(), d.more.clone())));
    dynamic(move || {
        let (lines, more) = lines.get();
        (!lines.is_empty()).then(|| {
            untracked(|| {
                let rows: Vec<AnyElement> = lines
                    .into_iter()
                    .map(|line| {
                        let action = line.action;
                        view()
                            .test_id(&line.id)
                            .role(AccessibilityRole::Button)
                            .label(line.text.clone())
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .cursor_pointer()
                            .on_press(move || run(app, action.clone()))
                            .when(form == Form::Phone, |row| row.min_h(px(TARGET)))
                            .when(form == Form::Desktop, |row| {
                                row.px(px(10.0))
                                    .py(px(4.0))
                                    .rounded(px(6.0))
                                    .bg(color(Token::YouBg))
                                    .hover(|s| s.bg(color(Token::Hover)))
                            })
                            .child(
                                words(form, line.text, hue_ink(Hue::You))
                                    .flex_1()
                                    .min_w(px(0.0)),
                            )
                            .child(words(form, "▴".to_owned(), color(Token::Ink3)))
                            .into_element()
                    })
                    .collect();
                view()
                    .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
                    .children(rows)
                    .children(more.map(|more| words(form, more, color(Token::Ink3))))
                    .into_element()
            })
        })
    })
}

/// The owner questions: the head with `Alt+A to answer`, then up to three questions, each its
/// title (and `answer recorded; delivery pending` once it is); pressing one opens its answer card.
/// The question whose card is open is not listed again.
fn questions(app: AppState, dock: Memo<Dock>) -> AnyElement {
    let shown = computed(move || {
        dock.with(|d| {
            let open = d.answer.as_ref().map(|answer| answer.card.as_str());
            d.questions.clone().map(|mut questions| {
                questions
                    .items
                    .retain(|item| Some(item.card.as_str()) != open);
                questions
            })
        })
    });
    dynamic(move || {
        let questions = shown.get().filter(|q| !q.items.is_empty())?;
        Some(untracked(|| question_dock(app, questions)))
    })
}

fn question_dock(app: AppState, questions: QuestionDock) -> AnyElement {
    let form = app.form;
    let top = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(
            words(form, format!("▲ {}", questions.header), hue_ink(Hue::You))
                .font_weight(FontWeight::SEMIBOLD)
                .flex_1()
                .min_w(px(0.0)),
        )
        .children(
            questions
                .hint
                .map(|hint| words(form, hint.to_owned(), color(Token::Ink3))),
        );
    let items: Vec<AnyElement> = questions
        .items
        .into_iter()
        .map(|item| {
            let spoken = match &item.note {
                Some(note) => format!("{}, {note}", item.label),
                None => item.label.clone(),
            };
            let line = view()
                .test_id(&item.id)
                .gap(px(1.0))
                .child(words(form, item.label.clone(), color(Token::Ink)))
                .children(
                    item.note
                        .clone()
                        .map(|note| words(form, note, color(Token::Ink3))),
                );
            let line = match item.action {
                Some(action) => line
                    .role(AccessibilityRole::Button)
                    .label(spoken)
                    .cursor_pointer()
                    .on_press(move || run(app, action.clone())),
                None => line,
            };
            match form {
                Form::Terminal => line.pl(term::cells(2.0)).into_element(),
                Form::Desktop => line
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .hover(|s| s.bg(color(Token::Hover)))
                    .into_element(),
                Form::Phone => line
                    .min_h(px(TARGET))
                    .justify_center()
                    .px(px(4.0))
                    .active(|s| s.bg(color(Token::Hover)))
                    .into_element(),
            }
        })
        .collect();
    let card = view()
        .test_id("session-questions")
        .gap(px(if form == Form::Terminal { 0.0 } else { 2.0 }))
        .child(top)
        .children(items)
        .children(
            questions
                .more
                .map(|more| words(form, more, color(Token::Ink3)).into_element()),
        );
    match form {
        Form::Terminal => card.into_element(),
        Form::Desktop | Form::Phone => card
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(8.0))
            .bg(color(Token::YouBg))
            .border_1()
            .border_color(color(Token::Rule2))
            .into_element(),
    }
}

/// The queue: one header line (`Queue · 2 waiting`, its clause and `+k more`, `▸`/`▾`) that
/// folds and unfolds it; unfolded, each owner item in delivery order (steers first) with its
/// controls, then input from other sources as one read-only line. While the page gives the queue
/// the keys, its first control has them.
fn queue(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || page.with(|p| p.dock.queue.clone()));
    let keyed = computed(move || page.with(|p| p.focus == Focus::Queue));
    dynamic(move || {
        let queue = shown.get()?;
        Some(untracked(|| queue_face(app, queue, keyed)))
    })
}

fn queue_face(app: AppState, queue: QueueDock, keyed: Memo<bool>) -> AnyElement {
    let form = app.form;
    let first = element_ref();
    effect(move || {
        if keyed.get() {
            after_answer(move || first.focus());
        }
    });
    let mut said = queue.header.clone();
    said.extend(queue.clause.clone());
    let toggle = queue.toggle.clone();
    let caret = if queue.open { "▾" } else { "▸" };
    let top = view()
        .test_id("btn-queue-toggle")
        .role(AccessibilityRole::Button)
        .label(format!(
            "{said}, {}",
            if queue.open { "fold" } else { "show" }
        ))
        .accessibility_state(AccessibilityState {
            expanded: Some(queue.open),
            ..AccessibilityState::default()
        })
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .on_press(move || run(app, toggle.clone()))
        .when(form == Form::Phone, |row| row.min_h(px(TARGET)))
        .child(head(form, caret.to_owned()).flex_shrink_0())
        .child(head(form, said).flex_1().min_w(px(0.0)))
        .children(queue.more.clone().map(|more| head(form, more)));
    let rows: Vec<AnyElement> = if queue.open {
        queue
            .items
            .into_iter()
            .enumerate()
            .map(|(at, item)| row(app, item, (at == 0).then_some(first)))
            .collect()
    } else {
        Vec::new()
    };
    let foreign = queue
        .foreign
        .filter(|_| queue.open)
        .map(|foreign| words(form, foreign, color(Token::Ink3)).into_element());
    view()
        .test_id("session-queue")
        .role(AccessibilityRole::List)
        .label("Queue")
        .gap(px(if form == Form::Terminal { 0.0 } else { 1.0 }))
        .child(top)
        .children(rows)
        .children(foreign)
        .into_element()
}

/// One queued item: `▌`, its place, its receiver, `steer ·` before a steer, the text cut to a
/// line, `…` while a change to it is in flight, then its controls (a `⋯` menu on the phone).
fn row(app: AppState, item: QueueItem, first: Option<ElementRef>) -> AnyElement {
    let form = app.form;
    let mut lead = format!("▌{} {}", item.index, item.to);
    if item.steer {
        lead.push_str("  steer ·");
    }
    let busy = item
        .busy
        .then(|| words(form, "…".to_owned(), color(Token::Ink3)));
    let controls = if form == Form::Phone {
        (!item.controls.is_empty()).then(|| row_menu(app, &item))
    } else {
        let mut handle = first;
        let pressed: Vec<AnyElement> = item
            .controls
            .iter()
            .map(|control| {
                // The first control that runs takes the keys when the queue is given them.
                let focus = control.action.is_some().then(|| handle.take()).flatten();
                control_button(app, control, focus)
            })
            .collect();
        (!pressed.is_empty()).then(|| {
            view()
                .flex_row()
                .flex_shrink_0()
                .items_center()
                .gap(px(if form == Form::Terminal { 8.0 } else { 4.0 }))
                .children(pressed)
                .into_element()
        })
    };
    view()
        .test_id(&item.id)
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .when(form == Form::Phone, |row| row.min_h(px(TARGET)))
        .when(form == Form::Desktop, |row| row.min_h(px(22.0)))
        .child(words(form, lead, color(Token::Ink3)).flex_shrink_0())
        .child(
            words(form, item.text, color(Token::Ink))
                .flex_1()
                .min_w(px(0.0)),
        )
        .children(busy)
        .children(controls)
        .into_element()
}

/// A queue control, one text line high: quiet words (Remove in the failure colour), muted and
/// inert while the host would refuse it or a change is in flight.
fn control_button(app: AppState, control: &QueueControl, focus: Option<ElementRef>) -> AnyElement {
    let form = app.form;
    let action = control.action.clone();
    let enabled = action.is_some();
    let ink = match (enabled, control.danger) {
        (false, _) => color(Token::Ink4),
        (true, true) => hue_ink(Hue::Fail),
        (true, false) => color(Token::Ink2),
    };
    let handle = focus.unwrap_or_else(element_ref);
    let face = text(control.label).number_of_lines(1u32).text_color(ink);
    let face = match form {
        Form::Terminal => face,
        _ => face.font_family(FONT_TEXT).text_size(px(11.5)),
    };
    let pressed = view()
        .element_ref(&handle)
        .test_id(&control.id)
        .role(AccessibilityRole::Button)
        .label(control.words)
        .accessibility_state(AccessibilityState {
            disabled: !enabled,
            ..AccessibilityState::default()
        })
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .focusable(enabled)
        .when(enabled, |pressed| {
            pressed.cursor_pointer().on_press(move || {
                if let Some(action) = &action {
                    run(app, action.clone());
                }
            })
        });
    match form {
        Form::Terminal => pressed
            .child(term::focus_cell(term::focus_of(handle)))
            .child(face)
            .into_element(),
        _ => pressed
            .px(px(4.0))
            .h(px(18.0))
            .rounded(px(4.0))
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(2.0))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(1.0))
            })
            .child(face)
            .into_element(),
    }
}

/// The phone's `⋯` menu of a queued item: Edit, Move up, Move down, Remove.
fn row_menu(app: AppState, item: &QueueItem) -> AnyElement {
    let entries: Vec<AnyElement> = item
        .controls
        .iter()
        .map(|control| {
            let action = control.action.clone();
            let enabled = action.is_some();
            let words = text(control.words)
                .font_family(FONT_TEXT)
                .text_size(px(16.0))
                .text_color(if control.danger && enabled {
                    hue_ink(Hue::Fail)
                } else if enabled {
                    color(Token::Ink)
                } else {
                    color(Token::Ink3)
                });
            menu_item()
                .accessibility_label(control.words)
                .disabled(!enabled)
                .on_select(move || {
                    if let Some(action) = &action {
                        run(app, action.clone());
                    }
                })
                .child(
                    view()
                        .test_id(&control.id)
                        .min_h(px(TARGET + 4.0))
                        .justify_center()
                        .px(px(18.0))
                        .child(words),
                )
                .into_element()
        })
        .collect();
    menu()
        .popup_style(popup(app.form))
        .backdrop_style(|s| s.bg(color(Token::Shell).opacity(0.5)))
        .on_open_change(counted(app))
        .trigger(menu_trigger(
            app,
            "⋯",
            "Queued input actions",
            &format!("btn-queue-menu-{}", item.index - 1),
        ))
        .child(view().children(entries))
        .into_element()
}

/// Where the read of the waiting cards stands while one is not read, with Retry after a
/// failure.
fn reading(app: AppState, dock: Memo<Dock>) -> AnyElement {
    let form = app.form;
    let read = computed(move || dock.with(|d| d.read.clone()));
    dynamic(move || {
        read.get().map(|read| {
            untracked(|| {
                let retry = read.retry.map(|action| {
                    button(
                        app,
                        "Retry",
                        ButtonKind::Quiet,
                        "btn-cards-retry",
                        move || run(app, action.clone()),
                    )
                });
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        words(form, read.words, cards::tint(read.hue))
                            .flex_1()
                            .min_w(px(0.0)),
                    )
                    .children(retry)
                    .into_element()
            })
        })
    })
}

/// Why the cards cannot be answered from here now (`answer when corrald is back`).
fn inert(app: AppState, dock: Memo<Dock>) -> AnyElement {
    let form = app.form;
    let why = computed(move || dock.with(|d| d.inert.clone()));
    dynamic(move || {
        why.get()
            .map(|why| untracked(|| words(form, why, color(Token::Ink3)).into_element()))
    })
}

/// The bar in the composer's place: why the session takes no message, and Resume or Relaunch
/// where its host can be started again here.
pub(super) fn host_bar(app: AppState, bar: Memo<Option<HostBar>>) -> AnyElement {
    let form = app.form;
    dynamic(move || {
        bar.get().map(|bar| {
            untracked(|| {
                let action = bar.action.map(|(label, action)| {
                    button(
                        app,
                        label,
                        ButtonKind::Primary,
                        format!("btn-{}", label.to_lowercase()),
                        move || run(app, action.clone()),
                    )
                });
                let line = view()
                    .test_id("session-host-bar")
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.0))
                    .child(view().flex_1().min_w(px(0.0)).child(words(
                        form,
                        bar.text,
                        hue_ink(Hue::Neutral),
                    )))
                    .children(action);
                match form {
                    Form::Terminal => view()
                        .flex_none()
                        .child(term::hrule())
                        .child(line.px(term::cells(1.0)))
                        .into_element(),
                    Form::Desktop => line
                        .flex_none()
                        .px(px(18.0))
                        .py(px(10.0))
                        .border_t(px(1.0))
                        .border_color(color(Token::Rule))
                        .bg(color(Token::Surface))
                        .into_element(),
                    Form::Phone => line.flex_none().px(px(16.0)).py(px(10.0)).into_element(),
                }
            })
        })
    })
    .into_element()
}
