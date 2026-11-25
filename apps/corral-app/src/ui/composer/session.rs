//! The session page's composer (`model::session::SessionComposer`): the draft, whose placeholder
//! says who receives it (`Message → parent`), over one bar: the model and effort the host
//! reports, File, the receiver for a build with children that take a message, the receiver's
//! words while Enter gives a reason or an answer and the last send's standing, then the working
//! indicator with Stop while the agent works and the send controls. Enter sends; while the agent
//! works Enter steers the running turn and Queue (Alt+Enter) keeps the text for after it. While
//! the dock asks for the owner's reason (`→ land on main · your reason`, Held) or answer
//! (`→ a2 · answer`), Enter gives the text instead, and an empty one is the model's to refuse
//! with what is missing. Text pasted into the field goes in as it was copied; images and files
//! are staged for the message while the session takes attachments. While the draft is a `/`
//! command the page's command menu matches, Enter runs that command instead. A control the host
//! cannot run now is dimmed with its reason under the bar. The keys are listed in the page's ⋯
//! menu, not under the composer. The draft lives in the session's UI state, so it survives tab
//! switches and leaving the page; the field takes the keys back whenever the page gives them to
//! the composer. Desktop: a rounded box over the page's foot; terminal: the same rows under a
//! rule; phone: Steer now | After this turn over one row of field and button, the button
//! stopping the turn while the agent works and the draft is empty.

use nsh_base::{menu, menu_item};
use nsh_core::element::{color as for_appearance, input};
use nsh_core::keymap::When;
use nsh_core::prelude::*;

use corral_protocol::session_state::SendPolicy;

use crate::model::hue::Hue;
use crate::model::session::{
    Action, ReceiverChoice, SendButton, SendMode, SessionComposer, StopButton,
};
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::overlay::counted;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::{Ask, Attach, PHONE_CHIP, chip_slop};

/// The field's test id.
const SESSION_FIELD: &str = "field-message";

/// The phone's least touch target.
const TARGET: f32 = 44.0;

pub(super) fn act(app: AppState, action: Action) {
    app.session_input(action, false);
}

/// Sends the draft with `policy`, unless the button that sends it is unavailable now. An empty
/// draft goes out only as a reason or an answer, whose emptiness the model answers. A message
/// carries the session's staged attachments, and Ask's also its chips' line.
fn send(
    app: AppState,
    id: &Id,
    composer: Memo<Option<SessionComposer>>,
    policy: SendPolicy,
    ask: Option<Ask>,
    attach: Attach,
) {
    let ready = composer.with_untracked(|composer| {
        let composer = composer.as_ref()?;
        let button = std::iter::once(&composer.send)
            .chain(composer.queue.as_ref())
            .find(|button| button.policy == policy)?;
        let giving = matches!(composer.mode, SendMode::Reason | SendMode::Answer);
        (button.unavailable.is_none() && (giving || !composer.draft.trim().is_empty()))
            .then(|| (composer.draft.clone(), giving))
    });
    let Some((text, giving)) = ready else {
        return;
    };
    let (text, restore) = super::outgoing(app, id, &text, giving);
    if let Some(ask) = ask.filter(|_| !giving) {
        super::send_ask(app, ask, id, text, policy, restore);
        return;
    }
    let attachments = if giving {
        Vec::new()
    } else {
        match super::page_refs(app, id, attach) {
            Ok(refs) => refs,
            Err(why) => {
                app.toast(why);
                return;
            }
        }
    };
    let text = super::dock_message(app, id, text, giving);
    let carried = !attachments.is_empty();
    let action = super::send_action(id, text, policy, attachments, restore);
    let sent = app.session_input(action, ask.is_some());
    if sent {
        app.clear_pastes(id);
        if carried {
            app.clear_staged(id);
        }
    }
}

/// The session's composer. `command` is what Enter runs instead of sending while the draft is a
/// `/` command the page's menu matches; `focused` says the page gives the composer the keys.
pub(crate) fn session_composer(
    app: AppState,
    id: Id,
    composer: Memo<Option<SessionComposer>>,
    command: Memo<Option<Vec<Action>>>,
    focused: Memo<bool>,
) -> AnyElement {
    build(app, id, composer, command, focused, None, true)
}

/// The same composer in Ask: its field takes the focus each time Ask opens, and the chips, the
/// staged attachments and the controls that stage them sit around it.
pub(crate) fn ask_composer(
    app: AppState,
    id: Id,
    composer: Memo<Option<SessionComposer>>,
    command: Memo<Option<Vec<Action>>>,
    focused: Memo<bool>,
) -> AnyElement {
    let ask = super::ask(app, &id);
    build(app, id, composer, command, focused, Some(ask), true)
}

/// Ask's composer docked at Home's foot: as [`ask_composer`], but the field takes the keyboard
/// focus only when pressed, so Home's keys walk its cards meanwhile.
pub(crate) fn docked_composer(
    app: AppState,
    id: Id,
    composer: Memo<Option<SessionComposer>>,
    command: Memo<Option<Vec<Action>>>,
) -> AnyElement {
    let ask = super::ask(app, &id);
    build(
        app,
        id,
        composer,
        command,
        computed(|| false),
        Some(ask),
        false,
    )
}

fn build(
    app: AppState,
    id: Id,
    composer: Memo<Option<SessionComposer>>,
    command: Memo<Option<Vec<Action>>>,
    focused: Memo<bool>,
    ask: Option<Ask>,
    take_focus: bool,
) -> AnyElement {
    let form = app.form;
    let entry = element_ref();
    let scope = id.clone();
    let composer = computed(move || {
        let mut composer = composer.get()?;
        if let Some(why) = app.session_input_block(&scope, ask.is_some()) {
            composer.send.unavailable = Some(why.into());
            if let Some(queue) = &mut composer.queue {
                queue.unavailable = Some(why.into());
            }
        }
        Some(composer)
    });
    let attach = match ask {
        Some(ask) => Attach::ask(ask),
        None => Attach::page(app, &id, composer),
    };
    if form != Form::Phone {
        effect(move || {
            if focused.get() {
                after_answer(move || entry.focus());
            }
        });
        let id = id.clone();
        on_key_with(
            "alt+enter",
            KeyOptions {
                when: When::Always,
                ..KeyOptions::default()
            },
            move |_| send(app, &id, composer, SendPolicy::FollowUp, ask, attach),
        );
    }
    if ask.is_some() && take_focus {
        super::focus_on_open(app, entry);
    }
    let docked = ask.is_some() && !take_focus;
    if docked {
        // A field that goes while it has the focus leaves no Escape to take.
        cleanup(move || app.docked_field.set(None));
    }
    let field = field(
        app,
        id.clone(),
        composer,
        command,
        entry,
        ask,
        attach,
        docked,
    );
    match form {
        Form::Phone => phone(app, id, composer, field, ask, attach),
        Form::Desktop | Form::Terminal => frame(app, id, composer, field, ask, attach),
    }
}

/// The draft field: multiline, Enter sends with the send button's policy (or runs the matched
/// `/` command), Shift+Enter breaks the line. A paste is the composer's to place (`super::paste`)
/// except in a terminal, where the field owns bracketed paste and a local clipboard shortcut
/// can stage attachments. Docked on Home, Escape leaves the field.
#[allow(clippy::too_many_arguments)]
fn field(
    app: AppState,
    id: Id,
    composer: Memo<Option<SessionComposer>>,
    command: Memo<Option<Vec<Action>>>,
    entry: ElementRef,
    ask: Option<Ask>,
    attach: Attach,
    docked: bool,
) -> AnyElement {
    let form = app.form;
    let draft = move || composer.with(|c| c.as_ref().map(|c| c.draft.clone()).unwrap_or_default());
    let placeholder = move || {
        composer.with(|c| {
            c.as_ref()
                .map(|c| format!("{} {}", c.title.lead, c.title.to))
                .unwrap_or_default()
        })
    };
    let test_id = if ask.is_some() {
        crate::model::assistant::COMPOSER_FIELD
    } else {
        SESSION_FIELD
    };
    app.focus_when_asked(test_id, entry);
    let field = super::configure_paste(app, &id, attach, entry, composer, input());
    let typed = id.clone();
    let field = field
        .element_ref(&entry)
        .test_id(test_id)
        .label(move || SharedString::from(placeholder()))
        .multiline(true)
        .submit_behavior(SubmitBehavior::Submit)
        .return_key_type(ReturnKeyType::Send)
        .on_submit(move |_: &str| {
            if let Some(actions) = command.get_untracked() {
                app.session_slash(&id, actions, ask.is_some());
                return;
            }
            let policy = composer.with_untracked(|c| c.as_ref().map(|c| c.send.policy));
            if let Some(policy) = policy {
                send(app, &id, composer, policy, ask, attach);
            }
        })
        .placeholder(placeholder)
        .value(draft)
        .on_change(move |text: &str| {
            act(
                app,
                Action::Draft {
                    session: typed.clone(),
                    text: text.to_owned(),
                },
            );
        })
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
        .bg(AdaptiveColor::TRANSPARENT)
        .when(docked, |field| {
            field
                .on_focus(move || app.docked_field.set(Some(entry)))
                .on_blur(move || app.docked_field.set(None))
        });
    match form {
        Form::Desktop => field
            .min_h(px(44.0))
            .max_h(px(160.0))
            .px(px(12.0))
            .py(px(6.0))
            .font_family(FONT_TEXT)
            .text_size(px(14.0))
            .line_height(px(20.0))
            .into_element(),
        Form::Phone => field
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(TARGET))
            .max_h(px(132.0))
            .px(px(12.0))
            .py(px(9.0))
            .rounded(px(10.0))
            .border(px(1.5))
            .border_color(color(Token::Rule2))
            .focus(|s| s.border_color(color(Token::Focus)))
            .font_family(FONT_TEXT)
            .text_size(px(16.0))
            .line_height(px(22.0))
            .into_element(),
        Form::Terminal => field
            .min_h(px(32.0))
            .max_h(px(96.0))
            .px(px(8.0))
            .into_element(),
    }
}

/// Words at the composer's note size in `ink`.
fn small(form: Form, words: impl Into<String>, ink: AdaptiveColor) -> Text {
    let words = text(words.into()).number_of_lines(1u32).text_color(ink);
    match form {
        Form::Terminal => words,
        Form::Desktop => words.font_family(FONT_TEXT).text_size(px(11.5)),
        Form::Phone => words.font_family(FONT_TEXT).text_size(px(13.0)),
    }
}

/// What the field's placeholder does not say: the receiver while Enter gives a reason or an
/// answer, in its tone, and the newest send's standing while it is current (`last input
/// queued`). Nothing otherwise.
fn standing(app: AppState, composer: Memo<Option<SessionComposer>>) -> AnyElement {
    let form = app.form;
    let shown = computed(move || {
        composer.with(|c| {
            let title = &c.as_ref()?.title;
            let to = (title.hue != Hue::Neutral).then(|| (title.to.clone(), title.hue));
            (to.is_some() || title.last.is_some()).then(|| (to, title.last.clone()))
        })
    });
    dynamic(move || {
        shown.get().map(|(to, last)| {
            untracked(|| {
                view()
                    .flex_row()
                    .items_center()
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .gap(px(6.0))
                    .children(to.map(|(to, hue)| {
                        small(form, to, tint_or_ink(hue))
                            .font_weight(FontWeight::SEMIBOLD)
                            .flex_shrink_0()
                    }))
                    .children(last.map(|(hue, words)| {
                        small(form, words, tint(hue))
                            .truncate()
                            .flex_shrink(1.0)
                            .min_w(px(0.0))
                    }))
                    .into_element()
            })
        })
    })
    .into_element()
}

/// While the agent works: the working indicator (`● working · 12s`) and Stop. Nothing otherwise.
fn working(app: AppState, composer: Memo<Option<SessionComposer>>) -> AnyElement {
    let form = app.form;
    let shown = computed(move || {
        composer.with(|c| {
            let c = c.as_ref()?;
            (c.activity.is_some() || c.stop.is_some()).then(|| (c.activity.clone(), c.stop.clone()))
        })
    });
    dynamic(move || {
        shown.get().map(|(activity, stop)| {
            untracked(|| {
                let activity = activity.map(|line| {
                    let words = match &line.elapsed {
                        Some(time) => format!("{} · {time}", line.phase),
                        None => line.phase.clone(),
                    };
                    view()
                        .flex_row()
                        .items_center()
                        .flex_shrink_0()
                        .gap(px(if form == Form::Terminal { 8.0 } else { 5.0 }))
                        .children(if line.spinner {
                            Some(mark(app, crate::model::board::Mark::Active, line.hue))
                        } else {
                            line.glyph
                                .map(|glyph| small(form, glyph, hue_ink(line.hue)).into_element())
                        })
                        .child(small(form, words, tint(line.hue)))
                        .into_element()
                });
                view()
                    .flex_row()
                    .items_center()
                    .flex_shrink_0()
                    .gap(px(8.0))
                    .children(activity)
                    .children(stop.map(|stop| stop_button(app, stop)))
                    .into_element()
            })
        })
    })
    .into_element()
}

fn tint(hue: Hue) -> AdaptiveColor {
    if hue == Hue::Neutral {
        color(Token::Ink3)
    } else {
        hue_ink(hue)
    }
}

/// The receiver's words: ink when neutral, else its tone (Held for a reason, You for an answer).
fn tint_or_ink(hue: Hue) -> AdaptiveColor {
    if hue == Hue::Neutral {
        color(Token::Ink)
    } else {
        hue_ink(hue)
    }
}

fn stop_button(app: AppState, stop: StopButton) -> AnyElement {
    let action = stop.action;
    button_with(
        app,
        stop.label,
        ButtonKind::Ghost,
        ButtonOptions {
            danger: true,
            ..ButtonOptions::default()
        },
        "btn-stop",
        move || act(app, action.clone()),
    )
}

/// The one bar under the field: the model and effort as the picker's pill, File, the receiver,
/// what the placeholder does not say, then (except on the phone, whose one button sits beside
/// the field) the working indicator, Stop, Queue and Send or Steer. It wraps only where the page
/// is too narrow for one row. On a phone the model, the effort and the receiver are compact
/// chips on one row, which scrolls sideways rather than wrapping when it does not fit.
fn bar(
    app: AppState,
    id: Id,
    composer: Memo<Option<SessionComposer>>,
    ask: Option<Ask>,
    attach: Attach,
) -> AnyElement {
    let form = app.form;
    let buttons =
        computed(move || composer.with(|c| c.as_ref().map(|c| (c.send.clone(), c.queue.clone()))));
    let receiver = computed(move || composer.with(|c| c.as_ref().and_then(|c| c.receiver.clone())));
    let sends = id.clone();
    // Ask on a new thread may know no model yet: it shows nothing rather than an absent model.
    let pill = computed(move || {
        ask.is_none() || composer.with(|c| c.as_ref().is_some_and(|c| c.model.is_some()))
    });
    let picks = id.clone();
    let chips = view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 16.0 } else { 8.0 }))
        .child(dynamic(move || {
            pill.get().then(|| {
                let id = picks.clone();
                untracked(|| crate::ui::ask::models::session_pill(app, id, composer))
            })
        }))
        .when(form != Form::Phone, |chips| {
            chips.child(super::attachments(app, &id, attach))
        })
        .child(dynamic(move || {
            receiver.get().map(|pill| {
                untracked(|| receiver_menu(app, pill.label, pill.choices, pill.unavailable))
            })
        }));
    if form == Form::Phone {
        return scroll()
            .horizontal()
            .flex_shrink_0()
            .child(chips.flex_shrink_0().py(px(7.0)))
            .into_element();
    }
    chips
        .flex_wrap()
        .child(standing(app, composer))
        .child(view().flex_1().min_w(px(0.0)))
        .child(working(app, composer))
        .child(dynamic(move || {
            buttons
                .get()
                .filter(|_| form != Form::Phone)
                .map(|(send, queue)| {
                    let id = sends.clone();
                    untracked(|| {
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .children(queue.map(|queue| {
                                send_button(
                                    app,
                                    id.clone(),
                                    composer,
                                    queue,
                                    ButtonKind::Ghost,
                                    ask,
                                    attach,
                                )
                            }))
                            .child(send_button(
                                app,
                                id,
                                composer,
                                send,
                                ButtonKind::Primary,
                                ask,
                                attach,
                            ))
                            .into_element()
                    })
                })
        }))
        .into_element()
}

fn send_button(
    app: AppState,
    id: Id,
    composer: Memo<Option<SessionComposer>>,
    button: SendButton,
    kind: ButtonKind,
    ask: Option<Ask>,
    attach: Attach,
) -> AnyElement {
    let policy = button.policy;
    button_with(
        app,
        button.label,
        kind,
        ButtonOptions {
            disabled: button.unavailable.is_some(),
            ..ButtonOptions::default()
        },
        button.id,
        move || send(app, &id, composer, policy, ask, attach),
    )
}

/// `→ parent ▾`: the parent and every child that takes a message, the current one marked. While
/// the host cannot take a message for its children it is muted with the reason and does not
/// open.
fn receiver_menu(
    app: AppState,
    label: String,
    choices: Vec<ReceiverChoice>,
    unavailable: Option<String>,
) -> AnyElement {
    let form = app.form;
    if let Some(why) = unavailable {
        return small(
            form,
            format!("{label} · unavailable: {why}"),
            color(Token::Ink3),
        )
        .test_id("btn-receiver")
        .truncate()
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .into_element();
    }
    // On a phone the chip drops the disclosure mark and keeps a full touch target by hit area.
    let shown = if form == Form::Phone {
        label
            .trim_end_matches(" ▾")
            .trim_end_matches('▾')
            .trim_end()
            .to_owned()
    } else {
        label.clone()
    };
    let trigger = view()
        .test_id("btn-receiver")
        .role(AccessibilityRole::Button)
        .label(format!("Receiver {label}"))
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .cursor_pointer()
        .when(form != Form::Terminal, |t| {
            t.px(px(8.0))
                .h(px(if form == Form::Phone {
                    PHONE_CHIP
                } else {
                    24.0
                }))
                .rounded(px(6.0))
                .border_1()
                .border_color(color(Token::Rule2))
                .hover(|s| s.bg(color(Token::Hover)))
        })
        .when(form == Form::Phone, |t| t.hit_slop(chip_slop()))
        .child(small(form, shown, color(Token::Ink)));
    menu()
        .popup_style(move |s| {
            let s = s
                .bg(color(Token::Surface))
                .border_1()
                .border_color(color(Token::Rule2));
            match form {
                Form::Desktop => s.min_w(px(200.0)).py(px(4.0)).rounded(px(8.0)).shadow_lg(),
                Form::Phone => s.rounded_t(px(14.0)).pt(px(8.0)).pb(px(24.0)),
                Form::Terminal => s.min_w(term::cells(24.0)),
            }
        })
        .backdrop_style(|s| s.bg(color(Token::Shell).opacity(0.5)))
        .on_open_change(counted(app))
        .trigger(trigger)
        .child(
            view().children(
                choices
                    .into_iter()
                    .map(|choice| {
                        let action = choice.action.clone();
                        let test_id = format!(
                            "seg-receiver-{}",
                            choice.child.as_deref().unwrap_or("parent")
                        );
                        menu_item()
                            .accessibility_label(choice.label.clone())
                            .on_select(move || act(app, action.clone()))
                            .cursor_pointer()
                            .hover(|s| s.bg(color(Token::Hover)))
                            .focus(|s| s.bg(color(Token::Hover)))
                            .child(
                                view()
                                    .test_id(&test_id)
                                    .px(if form == Form::Terminal {
                                        term::cells(1.0)
                                    } else {
                                        px(12.0)
                                    })
                                    .py(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
                                    .when(choice.selected, |row| row.bg(color(Token::Sel)))
                                    .child(small(form, choice.label, color(Token::Ink))),
                            )
                            .into_element()
                    })
                    .collect::<Vec<_>>(),
            ),
        )
        .into_element()
}

/// Why the send controls are dimmed, under the bar, once per reason.
fn reasons(app: AppState, composer: Memo<Option<SessionComposer>>) -> AnyElement {
    let form = app.form;
    let why = computed(move || {
        composer.with(|c| {
            let c = c.as_ref()?;
            let mut reasons: Vec<String> = Vec::new();
            for button in [Some(&c.send), c.queue.as_ref()].into_iter().flatten() {
                if let Some(why) = &button.unavailable
                    && !reasons.contains(why)
                {
                    reasons.push(why.clone());
                }
            }
            (!reasons.is_empty()).then(|| reasons.join(" · "))
        })
    });
    dynamic(move || {
        why.get()
            .map(|why| small(form, why, hue_ink(Hue::Held)).into_element())
    })
    .into_element()
}

/// Desktop and terminal: the box (a rule over it in a terminal) with the field over the one
/// bar; Ask's chips and the staged attachments over the box, and only while open, the path
/// field under the bar; why Send is dimmed under the box only while it is.
fn frame(
    app: AppState,
    id: Id,
    composer: Memo<Option<SessionComposer>>,
    field: AnyElement,
    ask: Option<Ask>,
    attach: Attach,
) -> AnyElement {
    if app.form == Form::Terminal {
        return view()
            .test_id("session-composer")
            .flex_none()
            .child(term::hrule())
            .child(
                view()
                    .px(term::cells(1.0))
                    .child(super::above(app, &id, ask, attach))
                    .child(field)
                    .child(bar(app, id.clone(), composer, ask, attach))
                    .child(super::attach_field(app, &id))
                    .child(reasons(app, composer)),
            )
            .into_element();
    }
    view()
        .test_id("session-composer")
        .flex_none()
        .px(px(14.0))
        .pt(px(8.0))
        .pb(px(8.0))
        .gap(px(6.0))
        .child(super::above(app, &id, ask, attach))
        .child(
            view()
                .rounded(px(10.0))
                .border_1()
                .border_color(color(Token::Rule2))
                .bg(color(Token::Surface))
                .child(view().pt(px(4.0)).child(field))
                .child(
                    view()
                        .px(px(12.0))
                        .pb(px(8.0))
                        .gap(px(6.0))
                        .child(bar(app, id.clone(), composer, ask, attach))
                        .child(super::attach_field(app, &id)),
                ),
        )
        .child(reasons(app, composer))
        .into_element()
}

/// The phone's line over the field, only while it has something to say: what the placeholder
/// does not, then the working indicator and Stop.
fn phone_line(app: AppState, composer: Memo<Option<SessionComposer>>) -> AnyElement {
    let noted = computed(move || {
        composer.with(|c| {
            c.as_ref().is_some_and(|c| {
                c.title.hue != Hue::Neutral
                    || c.title.last.is_some()
                    || c.activity.is_some()
                    || c.stop.is_some()
            })
        })
    });
    when(
        move || noted.get(),
        move || {
            untracked(|| {
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(standing(app, composer))
                    .child(view().flex_1().min_w(px(0.0)))
                    .child(working(app, composer))
                    .into_element()
            })
        },
    )
    .into_element()
}

/// The phone: while the agent works, Steer now | After this turn decides what the button does;
/// the field and the button in one row; the button stops the turn while the agent works and the
/// draft is empty.
fn phone(
    app: AppState,
    id: Id,
    composer: Memo<Option<SessionComposer>>,
    field: AnyElement,
    ask: Option<Ask>,
    attach: Attach,
) -> AnyElement {
    let after = signal(false);
    let working = computed(move || {
        composer.with(|c| c.as_ref().is_some_and(|c| c.mode == SendMode::Working))
    });
    let queues = computed(move || composer.with(|c| c.as_ref().is_some_and(|c| c.queue.is_some())));
    let choice = dynamic(move || {
        (working.get() && queues.get()).then(|| {
            untracked(|| {
                crate::ui::kit::button::seg(
                    app,
                    &[(false, "Steer now"), (true, "After this turn")],
                    move || after.get(),
                    move |value| after.set(value),
                    "seg-send-when",
                )
            })
        })
    });
    let action = computed(move || {
        composer.with(|c| {
            let c = c.as_ref()?;
            if c.mode == SendMode::Working && c.draft.trim().is_empty() {
                return c.stop.clone().map(Err);
            }
            Some(Ok(c.send.unavailable.is_some()))
        })
    });
    let id_for_button = id.clone();
    let button = dynamic(move || {
        action.get().map(|action| {
            let id = id_for_button.clone();
            untracked(|| match action {
                Err(stop) => {
                    let press = stop.action.clone();
                    button_with(
                        app,
                        "■",
                        ButtonKind::Ghost,
                        ButtonOptions {
                            danger: true,
                            ..ButtonOptions::default()
                        },
                        "btn-stop",
                        move || act(app, press.clone()),
                    )
                }
                Ok(dimmed) => button_with(
                    app,
                    "↑",
                    ButtonKind::Primary,
                    ButtonOptions {
                        disabled: dimmed,
                        ..ButtonOptions::default()
                    },
                    "btn-send",
                    move || {
                        let policy = if after.get_untracked() && working.get_untracked() {
                            SendPolicy::FollowUp
                        } else {
                            composer
                                .with_untracked(|c| c.as_ref().map(|c| c.send.policy))
                                .unwrap_or(SendPolicy::Steer)
                        };
                        send(app, &id, composer, policy, ask, attach);
                    },
                ),
            })
        })
    });
    view()
        .test_id("session-composer")
        .flex_none()
        .px(px(12.0))
        .pt(px(6.0))
        .gap(px(6.0))
        .child(super::above(app, &id, ask, attach))
        .child(phone_line(app, composer))
        .child(choice)
        .child(
            view()
                .flex_row()
                .items_end()
                .gap(px(8.0))
                .child(super::phone_plus(app, &id, attach))
                .child(field)
                .child(button),
        )
        .child(bar(app, id, composer, ask, attach))
        .child(reasons(app, composer))
        .into_element()
}
