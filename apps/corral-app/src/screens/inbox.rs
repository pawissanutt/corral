//! The Inbox destination and the focused question: the fixture preview's `Page::Question`, and
//! a served question's live page by its key. The model resolves every word, branch and order
//! (`model::inbox`); this module lays it out per form and sends each control's [`Action`]
//! back through the reducer (`State::act` for the fixture, `State::act_live` for corrald).
//!
//! Views read the model through memos built from a copy of the state without the typed text
//! (the alternate answer, the follow-up): typing changes only the decide bar, so the fields
//! keep their focus and the bands around them are not made again.

mod decide;
mod evidence;
mod graph;
mod list;
mod options;
mod sheet;
mod thread;

use nsh_core::prelude::*;

use crate::model::inbox::{self, Action, Button, ButtonKind as ModelKind, Src, State};
use crate::model::{Destination, Env, Form, Hue, Id, Rich};
use crate::screens::{PhoneBar, PhonePage};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::page::{Width, columns};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// The list's width beside the handover, and on a desktop window too small for both at full
/// width (the list keeps the room its rows need, the handover stacks its parts).
const LIST: f32 = 400.0;
const LIST_NARROW: f32 = 280.0;
/// The least room the handover takes beside the list, and the focus page's centre beside its
/// 340 pt context column, before they switch to their narrow layouts.
const SHEET_MIN: f32 = 560.0;
const SIDE: f32 = 340.0;

/// Which question a handover shows: a served focus page's key, or None for the fixture and
/// for the inline handover of the selected row.
type Key = Option<Id>;

/// The Inbox destination. Desktop: the 400 pt list beside the selection's pane (the full
/// handover with its decide bar pinned for a question); in a window narrower than both need,
/// a 280 pt list beside the handover in its narrow layout. Phone: the large title over the
/// needs-you cards and the changes feed, scrolling itself. Terminal: the list and the pane side
/// by side on a wide grid, stacked on a narrow one with the pane taking two thirds.
pub fn screen(app: AppState) -> AnyElement {
    list::load_changes(app);
    match app.form {
        Form::Desktop => by_width(app, LIST + SHEET_MIN, desk_inbox),
        Form::Phone => list::phone(app),
        Form::Terminal => terminal_panes(app, list::column, terminal_preview),
    }
}

/// The fixture preview's focused question.
pub fn question(app: AppState) -> AnyElement {
    focus_page(app, None)
}

/// A served question's focus page on desktop and terminal.
pub fn live_question(app: AppState, key: Id) -> AnyElement {
    focus_page(app, Some(key))
}

/// The fixture preview's question pushed on the phone.
pub fn question_phone(app: AppState) -> PhonePage {
    phone_page(app, None)
}

/// A served question pushed on the phone.
pub fn live_question_phone(app: AppState, key: Id) -> PhonePage {
    phone_page(app, Some(key))
}

/// The focused question. Desktop: the page head (back, pager, keys, Ask about this, Next), S A R
/// in the centre with the decide bar pinned under them, B Thread History in the 340 pt context
/// column; in a window too narrow for both, the context bands under S A R in one scroll.
/// Terminal: the same head; on a wide grid the context column beside the bands, on a narrow one
/// under them in the same scroll. A served question loads first, and says so when it cannot.
fn focus_page(app: AppState, key: Key) -> AnyElement {
    sheet::load(app, key.clone());
    let head_key = key.clone();
    let body = move |app: AppState| match app.form {
        Form::Terminal => {
            let key = head_key.clone();
            view()
                .flex_1()
                .min_h(px(0.0))
                .min_w(px(0.0))
                .child(dynamic(move || {
                    let wide = term::wide();
                    let key = key.clone();
                    Some(untracked(move || {
                        if wide {
                            focus_columns(app, key)
                        } else {
                            sheet::focus_main(app, key, true, true)
                        }
                    }))
                }))
                .into_element()
        }
        Form::Desktop | Form::Phone => {
            let key = head_key.clone();
            by_width_with(app, SHEET_MIN + SIDE, move |app, narrow| {
                if narrow {
                    sheet::focus_main(app, key.clone(), true, true)
                } else {
                    focus_columns(app, key.clone())
                }
            })
        }
    };
    let page_key = key.clone();
    view()
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .bg(color(Token::Bg))
        .child(sheet::focus_head(app, page_key))
        .child(sheet::gate(app, key, body))
        .into_element()
}

/// The question pushed on the phone: S R A B Thread History under the pager and head, the decide
/// bar in place of the tab bar, the Ask-about icon in the header, the thread as a sheet.
fn phone_page(app: AppState, key: Key) -> PhonePage {
    sheet::load(app, key.clone());
    let title = untracked(|| {
        let env = app.env();
        with_source(app, |state, src| {
            inbox::question(state, src, key.as_ref(), env).title
        })
    });
    let body_key = key.clone();
    PhonePage {
        title: title.into(),
        body: sheet::gate(app, key.clone(), move |app| {
            sheet::phone_body(app, body_key.clone())
        }),
        trailing: Some(sheet::ask_icon(app, key.clone())),
        bar: Some(PhoneBar::always(decide::decide(app, key, false))),
        ask: false,
        scroll: true,
    }
}

fn desk_inbox(app: AppState, narrow: bool) -> AnyElement {
    columns(
        app,
        vec![
            (
                Width::Fixed(if narrow { LIST_NARROW } else { LIST }),
                list::column(app),
            ),
            (Width::Fill, list::preview(app, narrow)),
        ],
        1,
    )
}

fn terminal_preview(app: AppState) -> AnyElement {
    list::preview(app, true)
}

fn focus_columns(app: AppState, key: Key) -> AnyElement {
    columns(
        app,
        vec![
            (
                Width::Fill,
                sheet::focus_main(app, key.clone(), false, app.form == Form::Terminal),
            ),
            (Width::Fixed(SIDE), sheet::side(app, key)),
        ],
        0,
    )
}

/// A screen laid out by the width it is given: `build(app, narrow)`, narrow below `min` points,
/// made again only when the width crosses `min`. It grows into its parent, so its measure is the
/// room and not its content.
fn by_width(app: AppState, min: f32, build: fn(AppState, bool) -> AnyElement) -> AnyElement {
    by_width_with(app, min, build)
}

fn by_width_with(
    app: AppState,
    min: f32,
    build: impl Fn(AppState, bool) -> AnyElement + 'static,
) -> AnyElement {
    let width = signal(None::<f32>);
    let narrow = computed(move || width.get().map(|room| room < min));
    view()
        .flex_1()
        .flex_col()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .on_layout(move |frame: &Bounds<Points>| {
            let room = frame.size.width.0;
            if width.get_untracked() != Some(room) {
                width.set(Some(room));
            }
        })
        .child(dynamic(move || {
            let narrow = narrow.get()?;
            Some(untracked(|| build(app, narrow)))
        }))
        .into_element()
}

/// Two panes of a terminal screen: side by side (the first 400 pt wide) on a wide grid; on a
/// narrow one stacked, a joined line between them, the second taking two thirds of the height
/// so the handover keeps its rows.
fn terminal_panes(
    app: AppState,
    first: fn(AppState) -> AnyElement,
    second: fn(AppState) -> AnyElement,
) -> AnyElement {
    view()
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .child(dynamic(move || {
            let wide = term::wide();
            Some(untracked(move || {
                if wide {
                    return columns(
                        app,
                        vec![(Width::Fixed(LIST), first(app)), (Width::Fill, second(app))],
                        1,
                    );
                }
                let part = |pane: AnyElement, share: f32| {
                    view()
                        .flex_1()
                        .flex_grow(share)
                        .flex_col()
                        .min_h(px(0.0))
                        .min_w(px(0.0))
                        .child(pane)
                };
                view()
                    .flex_1()
                    .flex_col()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .child(part(first(app), 1.0))
                    .child(term::hrule())
                    .child(part(second(app), 2.0))
                    .into_element()
            }))
        }))
        .into_element()
}

/// Runs `action` through the source's reducer, then the effects it returns.
fn act(app: AppState, action: Action) {
    if untracked(|| app.daemon()) {
        app.reduce(app.inbox, move |state, feed, _selection, env| {
            state.act_live(action, feed, env)
        });
    } else {
        app.act(app.inbox, move |state, env| state.act(action, env));
    }
}

/// A press handler dispatching `action`.
fn press(app: AppState, action: Action) -> impl Fn() + 'static {
    move || act(app, action.clone())
}

/// Tracked read of the state and the source: the served feed in daemon mode, else the fixture.
fn with_source<R>(app: AppState, read: impl FnOnce(&State, Src) -> R) -> R {
    if app.daemon() {
        app.feed
            .with(|feed| app.inbox.with(|state| read(state, Src::Live(feed))))
    } else {
        app.inbox.with(|state| read(state, Src::Fixture))
    }
}

/// A memo of a view builder over the state without its typed text (see the module docs).
fn blind<T: PartialEq + 'static>(
    app: AppState,
    build: impl Fn(&State, Src, Env) -> T + 'static,
) -> Memo<T> {
    computed(move || {
        let env = app.env();
        with_source(app, |state, src| build(&state.blind(), src, env))
    })
}

/// A part of the view made again whenever its memo's value changes.
fn live<T: PartialEq + Clone + 'static>(
    memo: Memo<T>,
    draw: impl Fn(T) -> AnyElement + 'static,
) -> AnyElement {
    dynamic(move || {
        let value = memo.get();
        Some(untracked(|| draw(value)))
    })
}

/// A model button as the kit draws it, its test id named by its action.
fn button(app: AppState, model: &Button) -> AnyElement {
    button_as(app, model, ButtonOptions::default())
}

fn button_as(app: AppState, model: &Button, options: ButtonOptions) -> AnyElement {
    button_with(
        app,
        model.label.clone(),
        kind(model.kind),
        ButtonOptions {
            disabled: options.disabled || !model.enabled,
            ..options
        },
        format!("btn-{}", action_id(&model.action, &model.label)),
        press(app, model.action.clone()),
    )
}

fn kind(kind: ModelKind) -> ButtonKind {
    match kind {
        ModelKind::Primary => ButtonKind::Primary,
        ModelKind::Ghost => ButtonKind::Ghost,
        ModelKind::Quiet => ButtonKind::Quiet,
    }
}

/// The stable part of a control's test id: what it does, not what it says, except for the
/// toast-only controls, which say what they are.
fn action_id(action: &Action, label: &str) -> String {
    match action {
        Action::Answer | Action::Submit(_) => "answer".into(),
        Action::Thread => "ask-worker".into(),
        Action::FocusReply => "ask-asker".into(),
        Action::SendThread | Action::Followup(_) => "send".into(),
        Action::CloseThread => "done".into(),
        Action::NextQ | Action::Next(_) => "next".into(),
        Action::Ask | Action::AskAbout(_, false) => "ask-about".into(),
        Action::AskAbout(_, true) => "discuss-pm".into(),
        Action::OpenQuestion => "open-question".into(),
        Action::PlanOpen(_) => "open-plan".into(),
        Action::Go(destination) => format!("go-{}", destination.slug()),
        Action::Open(_) | Action::Live(_) | Action::OpenRow(_) => "open".into(),
        Action::Evidence(_) | Action::Proof(..) => "proof".into(),
        Action::Recover(_) => "check-result".into(),
        Action::Refetch(_) => "reload".into(),
        Action::Select(_)
        | Action::Row(_)
        | Action::Choose(_)
        | Action::Pick(..)
        | Action::Draft(_)
        | Action::Text(..)
        | Action::Reply(..)
        | Action::Toast(_) => slug(label),
    }
}

/// Lower-case words joined by hyphens.
fn slug(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// Text at `size` (desktop and phone points) in `ink`; a terminal keeps its own font and size.
fn words(app: AppState, content: impl Into<SharedString>, size: f32, ink: Token) -> Text {
    let t = text(content.into()).text_color(color(ink));
    if app.form == Form::Terminal {
        t
    } else {
        t.font_family(FONT_TEXT).text_size(px(size))
    }
}

/// [`words`] in the mono font with tabular figures.
fn code(app: AppState, content: impl Into<SharedString>, size: f32, ink: Token) -> Text {
    let t = text(content.into()).text_color(color(ink));
    if app.form == Form::Terminal {
        t
    } else {
        t.font_family(FONT_MONO).tabular_nums().text_size(px(size))
    }
}

/// The model's [`Rich`] at `size` in `ink`, line height 1.5: the kit's `rich` has two sizes
/// only, and the sheet's metadata, consequences and attributions sit at others. Mono runs are
/// 0.93 of the size; dim runs ink-3; hued runs their hue. Fixture links open their page, live
/// links their record.
fn rich_at(app: AppState, content: &Rich, size: f32, ink: AdaptiveColor) -> Text {
    let terminal = app.form == Form::Terminal;
    let root = text("").flex_shrink(1.0).min_w(px(0.0)).text_color(ink);
    let root = if terminal {
        root
    } else {
        root.font_family(FONT_TEXT)
            .text_size(px(size))
            .line_height(px((size * 1.5).round()))
    };
    root.children(content.iter().map(|run| {
        let mut piece = text(run.text.clone());
        if run.bold {
            piece = piece.font_weight(FontWeight::BOLD);
        }
        if run.mono && !terminal {
            piece = piece.font_family(FONT_MONO).text_size(px(size * 0.93));
        }
        if run.hue != Hue::Neutral {
            piece = piece.text_color(hue_ink(run.hue));
        } else if run.dim {
            piece = piece.text_color(color(Token::Ink3));
        }
        if run.strike {
            piece = piece.line_through();
        }
        if let Some(page) = run.link {
            piece = piece
                .underline()
                .cursor_pointer()
                .role(AccessibilityRole::Link)
                .on_press(move || app.open(page));
        } else if let Some(record) = run.live.clone() {
            piece = piece
                .underline()
                .cursor_pointer()
                .role(AccessibilityRole::Link)
                .on_press(move || app.open_live(record.clone()));
        }
        piece
    }))
}

/// A leading icon for a button, by what it does: the chat bubble for asking, the back chevron
/// for going back.
fn icon_for(action: &Action) -> Option<Icon> {
    match action {
        Action::Thread | Action::Ask | Action::AskAbout(..) | Action::FocusReply => Some(Icon::Ask),
        Action::SendThread | Action::Followup(_) => Some(Icon::Send),
        Action::Go(Destination::Inbox) => Some(Icon::Back),
        _ => None,
    }
}

/// The keyboard focus ring of a pressable view (`:focus-visible`).
fn ring(s: nsh_core::element::StyleRefinement) -> nsh_core::element::StyleRefinement {
    s.outline(px(2.0))
        .outline_color(color(Token::Focus))
        .outline_offset(px(-2.0))
}
