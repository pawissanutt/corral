//! The keys bar of a daemon pane (`model::agents::keys`): the keys a terminal agent needs most,
//! More keys for the rest, the gathered keys as caps in the order they will be pressed, the text
//! to type after them and one Send. Desktop: a strip directly under the dark screen. Terminal:
//! rows under the screen after a rule, in the page's focus order; no keystroke of the page goes
//! to the pane and Esc keeps its meaning. Phone: docked in the page's bar while the terminal
//! shows, the keys in two rows of fixed share so they never take a third. A session whose keys
//! corrald refuses for a reason worth saying shows that reason in one quiet line instead.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::{Action, KeyBar, KeyButton, KeyPress, Keys, Session, Settled, View};
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::overlay::{self, Modal};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::run;

/// Which of the three the terminal shows; the bar is built once while it stays open, so a
/// change of outcome or a new screen never takes the field's focus.
#[derive(Clone, PartialEq)]
enum Shape {
    Open,
    Closed(String),
    None,
}

fn keys_of(session: Memo<Session>) -> Memo<Option<Keys>> {
    computed(move || session.with(|s| s.terminal.as_ref().and_then(|t| t.keys.clone())))
}

/// What goes directly under the pane's screen: the bar on desktop and in a terminal, or the
/// quiet line saying why keys are not sent. The phone docks the bar instead ([`docked`]).
pub fn under(app: AppState, session: Memo<Session>) -> AnyElement {
    let keys = keys_of(session);
    let shape = computed(move || {
        keys.with(|keys| match keys {
            Some(Keys::Open(_)) => Shape::Open,
            Some(Keys::Closed(words)) => Shape::Closed(words.clone()),
            None => Shape::None,
        })
    });
    dynamic(move || match shape.get() {
        Shape::Open if app.form != Form::Phone => untracked(|| bar(app, keys)),
        Shape::Closed(words) => closed(app, words),
        Shape::Open | Shape::None => view().into_element(),
    })
}

/// The phone's docked bar: the keys while the terminal shows and corrald takes keys, else the
/// page's reply.
pub fn docked(
    app: AppState,
    session: Memo<Session>,
    reply: impl Fn() -> AnyElement + 'static,
) -> AnyElement {
    let keys = keys_of(session);
    let shows = computed(move || {
        session.with(|s| s.view == View::Terminal)
            && keys.with(|keys| matches!(keys, Some(Keys::Open(_))))
    });
    dynamic(move || {
        if shows.get() {
            untracked(|| bar(app, keys))
        } else {
            untracked(&reply)
        }
    })
}

/// Why keys are not sent from here, quiet under the screen.
fn closed(app: AppState, words: String) -> AnyElement {
    let line = view().test_id("session-keys-closed").flex_none();
    match app.form {
        Form::Terminal => line
            .px(term::cells(1.0))
            .child(term::quiet(words).flex_shrink(1.0).min_w(px(0.0)))
            .into_element(),
        Form::Desktop | Form::Phone => line
            .px(px(if app.form == Form::Phone { 16.0 } else { 18.0 }))
            .py(px(7.0))
            .child(
                text(words)
                    .font_family(FONT_TEXT)
                    .text_size(px(if app.form == Form::Phone { 12.5 } else { 11.5 }))
                    .text_color(color(Token::Ink3)),
            )
            .into_element(),
    }
}

/// Once a command settles, the screen is read again and its keys are done with; the field
/// clears only while it still holds the text that was typed.
fn settle(app: AppState, shown: Memo<Option<KeyBar>>, draft: Signal<String>) {
    let settled = computed(move || shown.with(|bar| bar.as_ref().and_then(|b| b.settled.clone())));
    let seen = stored(None::<Option<Settled>>);
    effect(move || {
        let now = settled.get();
        if let Some(before) = seen.replace(Some(now.clone()))
            && before != now
            && let Some(Settled { key, typed }) = now
        {
            if typed.is_some_and(|typed| draft.get_untracked() == typed) {
                draft.set(String::new());
            }
            untracked(|| run(app, Action::Keys(KeyPress::Settled(key))));
        }
    });
}

/// The bar, built once while it stays open; each part follows only what it shows.
fn bar(app: AppState, keys: Memo<Option<Keys>>) -> AnyElement {
    let form = app.form;
    let shown = computed(move || {
        keys.with(|keys| match keys {
            Some(Keys::Open(bar)) => Some(bar.clone()),
            _ => None,
        })
    });
    let draft = signal(String::new());
    settle(app, shown, draft);
    // Each part follows only its own facts: a key pressed changes the gathered keys, not the
    // key rows, so the focus stays on that key for the next press.
    let rows = computed(move || {
        shown.with(|bar| {
            bar.as_ref()
                .map(|bar| (bar.groups.clone(), bar.more.clone(), bar.enabled))
        })
    });
    let keys_part = dynamic(move || {
        rows.with(|rows| {
            rows.as_ref()
                .map(|(groups, more, enabled)| key_rows(app, groups, more, *enabled))
        })
    });
    let caps = computed(move || shown.with(|bar| bar.as_ref().map(Caps::of)));
    let edits = computed(move || {
        shown.with(|bar| {
            bar.as_ref()
                .map(|bar| [bar.remove.clone(), bar.clear.clone()])
        })
    });
    let sequence = sequence(
        app,
        dynamic(move || caps.with(|caps| caps.as_ref().map(|caps| caps_view(app, caps)))),
        dynamic(move || edits.with(|edits| edits.as_ref().map(|edits| edit_buttons(app, edits)))),
    );
    let (placeholder, label) = untracked(|| {
        shown.with(|bar| {
            bar.as_ref()
                .map_or(("", ""), |bar| (bar.placeholder, bar.field_label))
        })
    });
    let send = computed(move || shown.with(|bar| bar.as_ref().map(|bar| (bar.send, bar.enabled))));
    let send = dynamic(move || {
        send.get().map(|(label, enabled)| {
            button_with(
                app,
                label,
                ButtonKind::Primary,
                ButtonOptions {
                    icon: Some(Icon::Send),
                    disabled: !enabled,
                    ..ButtonOptions::default()
                },
                "btn-key-send",
                move || run(app, Action::Keys(KeyPress::Send(draft.get_untracked()))),
            )
        })
    });
    let entry = entry(app, field(app, placeholder, label, draft), send);
    let notes = computed(move || shown.with(|bar| bar.as_ref().map(Foot::of)));
    let foot = dynamic(move || notes.with(|notes| notes.as_ref().map(|notes| foot(app, notes))));
    let root = view()
        .test_id("session-keys")
        .role(AccessibilityRole::Group)
        .label("Keys for the pane")
        .flex_none();
    // The chooser's host sits beside the parts, never between them, so it adds no gap.
    let chooser = chooser(app, shown);
    match form {
        Form::Terminal => root
            .child(term::hrule())
            .child(keys_part)
            .child(sequence)
            .child(entry)
            .child(foot)
            .child(chooser)
            .into_element(),
        Form::Desktop => root
            .px(px(18.0))
            .py(px(10.0))
            .bg(color(Token::Bg))
            .border_t_1()
            .border_t_color(color(Token::Rule))
            .child(
                view()
                    .gap(px(8.0))
                    .child(keys_part)
                    .child(sequence)
                    .child(entry)
                    .child(foot),
            )
            .child(chooser)
            .into_element(),
        // The gathered keys first, right under the screen they act on, then the keys, so the
        // thumb reaches the field and Send at the bottom.
        Form::Phone => root
            .px(px(14.0))
            .pt(px(10.0))
            .pb_safe_area_plus(px(12.0))
            .bg(color(Token::Bg))
            .border_t(px(1.0))
            .border_color(color(Token::Rule))
            .child(
                view()
                    .gap(px(6.0))
                    .child(sequence)
                    .child(keys_part)
                    .child(entry)
                    .child(foot),
            )
            .child(chooser)
            .into_element(),
    }
}

/// One key: it adds itself to the gathered keys. On the phone it takes an equal share of its
/// row, so the rows never wrap into a third.
fn key(app: AppState, key: &KeyButton, enabled: bool, kind: ButtonKind) -> AnyElement {
    let action = key.action.clone();
    button_with(
        app,
        key.label.clone(),
        kind,
        ButtonOptions {
            disabled: !enabled,
            wide: app.form == Form::Phone,
            ..ButtonOptions::default()
        },
        key.test_id.clone(),
        move || run(app, action.clone()),
    )
}

/// The control keys, then the cursor keys and Enter, then More keys. Desktop and terminal: one
/// row, the groups set apart, wrapping when narrow. Phone: the control keys and More, then the
/// cursor keys and Enter, each row shared equally.
fn key_rows(
    app: AppState,
    [control, cursor]: &[Vec<KeyButton>; 2],
    more: &KeyButton,
    enabled: bool,
) -> AnyElement {
    let control = control
        .iter()
        .map(|k| key(app, k, enabled, ButtonKind::Ghost));
    let cursor = cursor
        .iter()
        .map(|k| key(app, k, enabled, ButtonKind::Ghost));
    let more = key(app, more, enabled, ButtonKind::Quiet);
    match app.form {
        Form::Phone => view()
            .gap(px(6.0))
            .child(view().flex_row().gap(px(6.0)).children(control).child(more))
            .child(view().flex_row().gap(px(6.0)).children(cursor))
            .into_element(),
        Form::Terminal => {
            let spaced = |faces: Vec<AnyElement>| {
                faces.into_iter().enumerate().flat_map(|(at, face)| {
                    (at > 0).then(|| term::space(1.0)).into_iter().chain([face])
                })
            };
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .px(term::cells(1.0))
                .children(spaced(control.collect()))
                .child(term::space(3.0))
                .children(spaced(cursor.collect()))
                .child(term::space(3.0))
                .child(more)
                .into_element()
        }
        Form::Desktop => view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .children(control)
            .child(view().w(px(10.0)))
            .children(cursor)
            .child(view().w(px(10.0)))
            .child(more)
            .into_element(),
    }
}

/// One gathered key as a cap: a terminal's own cap, else the mono key on a sunk tile.
fn cap(app: AppState, label: &str) -> AnyElement {
    if app.form == Form::Terminal {
        return term::key_cap(label.to_owned());
    }
    let phone = app.form == Form::Phone;
    view()
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .h(px(if phone { 26.0 } else { 20.0 }))
        .px(px(6.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(color(Token::Rule2))
        .bg(color(Token::BgSunk))
        .accessibility_elements_hidden(true)
        .child(
            text(label.to_owned())
                .number_of_lines(1u32)
                .font_family(FONT_MONO)
                .text_size(px(if phone { 13.0 } else { 11.5 }))
                .text_color(color(Token::Ink)),
        )
        .into_element()
}

/// What the gathered keys show: redrawn when a key is added or removed, apart from Remove last
/// and Clear, which keep their focus while there is something to remove.
#[derive(Clone, PartialEq)]
struct Caps {
    label: &'static str,
    keys: Vec<String>,
    empty: &'static str,
    spoken: String,
}

impl Caps {
    fn of(bar: &KeyBar) -> Self {
        Self {
            label: bar.sequence_label,
            keys: bar.sequence.clone(),
            empty: bar.empty,
            spoken: bar.spoken.clone(),
        }
    }
}

/// The gathered keys as caps in order, or that none are added, named for screen readers as one
/// line. The phone pans a long sequence rather than growing its bar; elsewhere it wraps.
fn caps_view(app: AppState, caps: &Caps) -> AnyElement {
    let form = app.form;
    let faces: Vec<AnyElement> = if caps.keys.is_empty() {
        vec![match form {
            Form::Terminal => term::quiet(caps.empty).into_element(),
            Form::Desktop | Form::Phone => text(caps.empty)
                .accessibility_elements_hidden(true)
                .font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone { 13.0 } else { 12.0 }))
                .text_color(color(Token::Ink3))
                .into_element(),
        }]
    } else {
        caps.keys.iter().map(|label| cap(app, label)).collect()
    };
    let row = view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .children(faces.into_iter().enumerate().flat_map(|(at, face)| {
            (at > 0 && form == Form::Terminal)
                .then(|| term::space(1.0))
                .into_iter()
                .chain([face])
        }));
    view()
        .test_id("session-keys-sequence")
        .label(caps.spoken.clone())
        .flex_1()
        .min_w(px(0.0))
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .child(match form {
            Form::Terminal => term::quiet(caps.label).into_element(),
            Form::Desktop | Form::Phone => text(caps.label)
                .accessibility_elements_hidden(true)
                .font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone { 12.5 } else { 11.5 }))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink3))
                .into_element(),
        })
        .when(form == Form::Terminal, |line| line.child(term::space(1.0)))
        .child(if form == Form::Phone {
            scroll()
                .horizontal()
                .flex_1()
                .min_w(px(0.0))
                .child(row)
                .into_element()
        } else {
            row.flex_wrap().flex_1().min_w(px(0.0)).into_element()
        })
        .into_element()
}

/// Remove last and Clear, quiet; dimmed while there is nothing to remove.
fn edit_buttons(app: AppState, edits: &[(&'static str, Option<Action>); 2]) -> AnyElement {
    let [remove, clear] = edits;
    let edit = |(label, action): &(&'static str, Option<Action>), test_id: &'static str| {
        let action = action.clone();
        button_with(
            app,
            *label,
            ButtonKind::Quiet,
            ButtonOptions {
                disabled: action.is_none(),
                ..ButtonOptions::default()
            },
            test_id,
            move || {
                if let Some(action) = action.clone() {
                    run(app, action);
                }
            },
        )
    };
    let terminal = app.form == Form::Terminal;
    view()
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .gap(px(if terminal { 0.0 } else { 6.0 }))
        .child(edit(remove, "btn-keys-remove"))
        .when(terminal, |row| row.child(term::space(1.0)))
        .child(edit(clear, "btn-keys-clear"))
        .into_element()
}

/// The gathered keys' row: the caps taking the room, Remove last and Clear at its end.
fn sequence(app: AppState, caps: AnyElement, edits: AnyElement) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    view()
        .flex_row()
        .items_center()
        .gap(px(if terminal { 0.0 } else { 6.0 }))
        .when(terminal, |row| row.px(term::cells(1.0)))
        .child(view().flex_1().min_w(px(0.0)).flex_row().child(caps))
        .when(terminal, |row| row.child(term::space(2.0)))
        .child(edits)
        .into_element()
}

/// The text field: the words typed into the pane after the keys, as written, in the pane's mono
/// face; it may hold several lines.
fn field(
    app: AppState,
    placeholder: &'static str,
    label: &'static str,
    draft: Signal<String>,
) -> AnyElement {
    let form = app.form;
    let entry = input()
        .test_id("session-keys-text")
        .label(label)
        .multiline(true)
        .placeholder(placeholder)
        .value(move || draft.get())
        .on_change(move |text: &str| draft.set(text.to_owned()))
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(AdaptiveColor::TRANSPARENT);
    match form {
        Form::Desktop => entry
            .h(px(38.0))
            .font_family(FONT_MONO)
            .text_size(px(12.5))
            .line_height(px(19.0))
            .into_element(),
        Form::Phone => entry
            .h(px(40.0))
            .py(px(9.0))
            .font_family(FONT_MONO)
            .text_size(px(15.0))
            .line_height(px(21.0))
            .into_element(),
        Form::Terminal => entry.h(term::rows(2.0)).into_element(),
    }
}

/// The field and Send in one raised box, as the reply box sets them; a terminal sets them on
/// one row a Cell in.
fn entry(app: AppState, field: AnyElement, send: AnyElement) -> AnyElement {
    match app.form {
        Form::Terminal => view()
            .flex_row()
            .items_end()
            .px(term::cells(1.0))
            .child(field)
            .child(term::space(2.0))
            .child(send)
            .into_element(),
        form => view()
            .flex_row()
            .items_end()
            .gap(px(8.0))
            .pl(px(12.0))
            .pr(px(if form == Form::Phone { 6.0 } else { 8.0 }))
            .py(px(if form == Form::Phone { 6.0 } else { 8.0 }))
            .rounded(px(if form == Form::Phone { 12.0 } else { 8.0 }))
            .bg(color(Token::Surface))
            .border_1()
            .border_color(color(Token::Rule2))
            .child(field)
            .child(send)
            .into_element(),
    }
}

/// Small words under the field in `ink`, at the form's note size.
fn small(app: AppState, words: impl Into<SharedString>, ink: AdaptiveColor) -> Text {
    let words = text(words.into())
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(ink);
    match app.form {
        Form::Terminal => words.number_of_lines(1u32),
        Form::Phone => words.font_family(FONT_TEXT).text_size(px(12.0)),
        Form::Desktop => words.font_family(FONT_TEXT).text_size(px(11.5)),
    }
}

/// The words under the field, redrawn only when they change.
#[derive(Clone, PartialEq)]
struct Foot {
    hint: &'static str,
    note: Option<String>,
    outcome: Option<(Hue, String)>,
    recover: Option<Action>,
}

impl Foot {
    fn of(bar: &KeyBar) -> Self {
        Self {
            hint: bar.hint,
            note: bar.note.clone(),
            outcome: bar.outcome.clone(),
            recover: bar.recover.clone(),
        }
    }
}

/// How the keys and the text go, in the quiet ink the reader still reads; why nothing is sent;
/// and the latest command's outcome in its tone, with "Check result" when it is unknown.
fn foot(app: AppState, foot: &Foot) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let outcome = foot.outcome.clone().map(|(hue, words)| {
        let ink = match hue {
            Hue::Neutral => color(Token::Ink3),
            hue => hue_ink(hue),
        };
        view()
            .flex_row()
            .items_center()
            .gap(px(if terminal { 0.0 } else { 8.0 }))
            .child(small(app, words, ink).test_id("session-keys-status"))
            .children(foot.recover.clone().map(|action| {
                view()
                    .flex_row()
                    .when(terminal, |row| row.child(term::space(2.0)))
                    .child(button(
                        app,
                        "Check result",
                        ButtonKind::Quiet,
                        "btn-recover-keys",
                        move || run(app, action.clone()),
                    ))
            }))
    });
    view()
        .gap(px(if terminal { 0.0 } else { 4.0 }))
        .when(terminal, |words| words.px(term::cells(1.0)))
        .child(small(app, foot.hint, color(Token::Ink3)))
        .children(
            foot.note
                .clone()
                .map(|note| small(app, note, color(Token::Ink2))),
        )
        .children(outcome)
        .into_element()
}

/// More keys: the keys the bar does not show, by kind. A key adds itself and closes it; Close,
/// Escape and the scrim close it with nothing added.
fn chooser(app: AppState, shown: Memo<Option<KeyBar>>) -> AnyElement {
    let open = computed(move || shown.with(|bar| bar.as_ref().and_then(|b| b.chooser.clone())));
    overlay::modal(
        app,
        move || open.with(Option::is_some),
        move || {
            if let Some(chooser) = open.get_untracked() {
                run(app, chooser.close);
            }
        },
        move || {
            let chooser = open.get_untracked();
            let body = view()
                .gap(px(if app.form == Form::Terminal {
                    0.0
                } else {
                    14.0
                }))
                .children(chooser.iter().flat_map(|chooser| {
                    chooser
                        .groups
                        .iter()
                        .map(move |(title, keys)| group(app, *title, keys))
                }))
                .into_element();
            Modal {
                test_id: "session-keys-more".into(),
                title: chooser
                    .as_ref()
                    .map_or_else(String::new, |c| c.title.to_owned()),
                sub: chooser.as_ref().map(|c| c.sub.to_owned()),
                body,
                done: Some("Close".into()),
                confirm: None,
            }
        },
    )
}

/// One kind of key in the chooser: its name, then its keys wrapping.
fn group(app: AppState, title: &'static str, keys: &[KeyButton]) -> AnyElement {
    let form = app.form;
    let faces = keys.iter().enumerate().flat_map(|(at, k)| {
        (at > 0 && form == Form::Terminal)
            .then(|| term::space(1.0))
            .into_iter()
            .chain([key_face(app, k)])
    });
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(match form {
            Form::Terminal => term::quiet(title).into_element(),
            Form::Desktop | Form::Phone => text(title)
                .role(AccessibilityRole::Header)
                .font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone { 13.0 } else { 12.0 }))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink2))
                .into_element(),
        })
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
                .children(faces),
        )
        .into_element()
}

/// A chooser key at its own width: the sheet wraps them, so none shares a row equally.
fn key_face(app: AppState, key: &KeyButton) -> AnyElement {
    let action = key.action.clone();
    button(
        app,
        key.label.clone(),
        ButtonKind::Ghost,
        key.test_id.clone(),
        move || run(app, action.clone()),
    )
}
