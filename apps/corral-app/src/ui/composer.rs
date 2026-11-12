//! The Ask composer and the surface it opens in. One field and one Send: the Assistant | PM switch
//! with the PM's state, the model pill and its list, the "answered by" line after a switch, the
//! removable context chip, the offline or waiting line, the attachments. Enter sends and
//! Shift+Enter breaks the line; Send hands the draft and target to the assistant model, which
//! stores the message with its state at once, and the draft clears after a nonblank send.

use nsh_base::{SheetEvent, sheet, toggle, toggle_group};
use nsh_core::element::{color as for_appearance, input};
use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::model::assistant::{self, Composer, ContextChip, KeyHint, ModelList};
use crate::model::hue::Hue;
use crate::model::text::Rich;
use crate::model::{Form, Overlay};
use crate::state::AppState;
use crate::ui::ask::{act, drawer, models};
use crate::ui::atoms::key_hint;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::text::rich_small;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

/// The mockup's icons for the attachments, in the model's order: Screenshot, Link, File.
const ATTACH_ICONS: [Icon; 3] = [Icon::Review, Icon::Chevron, Icon::Plans];

/// Where a composer's model list opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListAt {
    /// A popup above the control row: the drawer, a thread, a terminal.
    Above,
    /// A popup below the control row: the hotkey window.
    Below,
    /// In the flow under the control row: the phone sheet and a phone thread.
    Inline,
}

/// What a composer is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Host {
    /// A named session's own composer; `None` for the quick thread and Ask.
    pub session: Option<&'static str>,
    /// Where the model list opens.
    pub list: ListAt,
    /// Under a thread page rather than in Ask: it never takes the focus when Ask opens.
    pub docked: bool,
}

/// Ask's composer: the drawer's on desktop, the sheet's on a phone, the pane's in a terminal.
/// Reads and writes `app.ask_draft` and `app.ask_target`; focuses its field whenever
/// `app.ask_focus` changes.
pub fn composer(app: AppState) -> AnyElement {
    let list = if app.form == Form::Phone {
        ListAt::Inline
    } else {
        ListAt::Above
    };
    field(
        app,
        Host {
            session: None,
            list,
            docked: false,
        },
    )
}

/// The surface Ask opens in while `Overlay::Ask` is open: on desktop a drawer docked on the right
/// of its parent over the current screen; in a terminal a pane beside the page (all of a narrow
/// grid); on a phone a sheet with the keyboard up. Closing it (its Close, the backdrop, Esc,
/// back) calls `app.close(Overlay::Ask)` through the model.
pub fn ask_surface(app: AppState) -> AnyElement {
    match app.form {
        Form::Phone => sheet()
            .open(move || Some(app.is_open(Overlay::Ask)))
            .close_on_swipe(true)
            .on_open_change(move |_: &SheetEvent| act(app, assistant::Action::CloseAsk))
            .backdrop_style(|s| s.bg(color(Token::Shell).opacity(0.5)))
            // Made on each opening, so the field and its focus request start fresh. Built
            // untracked: a signal the build reads must not rebuild the open sheet and lose the
            // draft's focus.
            .child(dynamic(move || untracked(|| drawer::phone_panel(app))))
            .into_element(),
        Form::Desktop | Form::Terminal => when(
            move || app.is_open(Overlay::Ask),
            move || {
                untracked(|| {
                    if app.form == Form::Terminal {
                        drawer::pane(app)
                    } else {
                        drawer::desktop(app)
                    }
                })
            },
        )
        .into_element(),
    }
}

/// A composer for `host`. Every part reads the model's composer view through its own memo, so
/// typing redraws only what the draft changes (Send's state), never the field itself.
pub(crate) fn field(app: AppState, host: Host) -> AnyElement {
    let form = app.form;
    let Host {
        session,
        list,
        docked,
    } = host;
    let built = computed(move || {
        let env = app.env();
        let target = app.ask_target.get();
        app.ask_draft.with(|text| {
            app.assistant
                .with(|state| assistant::composer(state, target, text, session, env))
        })
    });
    let entry = element_ref();
    let row = element_ref();
    if !docked {
        focus_on_open(app, entry);
    }
    if form != Form::Phone && session.is_none() {
        bind_target_key(app);
    }
    let menu = computed(move || built.with(|c| c.model_menu.clone()));
    let status = computed(move || built.with(|c| c.status.clone()));
    let next = computed(move || built.with(|c| c.next_message.clone()));
    let terminal = form == Form::Terminal;
    let shell = view().gap(px(if terminal { 0.0 } else { 8.0 }));
    let shell = match (form, docked) {
        (Form::Desktop, _) => shell
            .px(px(14.0))
            .pt(px(10.0))
            .pb(px(12.0))
            .bg(color(Token::Bg))
            .border_t(px(1.0))
            .border_color(color(Token::Rule)),
        (Form::Phone, true) => shell
            .px(px(14.0))
            .py(px(8.0))
            .bg(color(Token::Bg))
            .border_t(px(1.0))
            .border_color(color(Token::Rule)),
        (Form::Phone, false) => shell.px(px(14.0)).pt(px(4.0)).pb(px(14.0)),
        (Form::Terminal, _) => shell.px(term::cells(1.0)),
    };
    shell
        .when(terminal, |s| s.child(term::blank()))
        .child(dynamic(move || {
            status
                .get()
                .map(|line| untracked(|| saved_line(app, &line)))
        }))
        .child(dynamic(move || {
            next.get().map(|line| untracked(|| saved_line(app, &line)))
        }))
        .child(controls(app, built, list, row, menu))
        .when(list == ListAt::Inline, |s| {
            s.child(models::inline_list(app, menu))
        })
        .child(chip_row(app, built))
        .when(terminal, |s| s.child(term::blank()))
        .child(entry_field(app, built, entry, session))
        .when(terminal, |s| s.child(term::blank()))
        .child(foot(app, built, session))
        .when(terminal, |s| s.child(term::blank()))
        .into_element()
}

/// Each opening of Ask bumps `app.ask_focus`. The field is made before it is mounted, so a focus
/// sent from this run would reach nothing: `after_answer` waits for the commit after the one that
/// mounts it, which is also after a closing dialog (the palette running Ask) hands its focus
/// back, so this focus lands last. The caret goes after any draft the opening brought.
fn focus_on_open(app: AppState, entry: ElementRef) {
    effect(move || {
        app.ask_focus.with(|_| ());
        after_answer(move || {
            let end = app
                .ask_draft
                .with_untracked(|draft| draft.encode_utf16().count());
            let end = u32::try_from(end).unwrap_or(u32::MAX);
            entry.focus();
            entry.set_selection(end, end);
        });
    });
}

/// `Ctrl ⇧ P` flips the target without leaving the field, while this composer lives.
fn bind_target_key(app: AppState) {
    let anywhere = KeyOptions {
        when: When::Always,
        ..KeyOptions::default()
    };
    on_key_with("mod+shift+p", anywhere, move |_| {
        let target = app.ask_target.get_untracked();
        let mut next = None;
        app.act(app.assistant, |state, env| {
            let outcome = state.key(assistant::Key::ToggleTarget, "", target, None, env);
            next = outcome.target;
            outcome.effects
        });
        if let Some(next) = next {
            app.ask_target.set(next);
        }
    });
}

/// Sends the draft to the composer's session or Ask's target; the model stores it with its state
/// (sent, queued on this device, waiting with its reason) and says whether the draft clears.
fn send(app: AppState, session: Option<&'static str>) {
    let text = app.ask_draft.get_untracked();
    let target = app.ask_target.get_untracked();
    let mut clear = false;
    app.act(app.assistant, |state, env| {
        let outcome = state.key(assistant::Key::Send, &text, target, session, env);
        clear = outcome.clear_draft;
        outcome.effects
    });
    if clear {
        app.ask_draft.set(String::new());
    }
}

/// A status line above the composer (`.savedline`): its first run as a chip in its colour
/// ("offline", "waiting for assistant · host down", the new model's name), the sentence after it.
fn saved_line(app: AppState, line: &Rich) -> AnyElement {
    let Some((lead, rest)) = line.split_first() else {
        return view().into_element();
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(chip(app, lead.text.clone(), lead.hue))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(rich_small(app, &rest.to_vec())),
        )
        .into_element()
}

/// The control row (`.crow`): the target switch, the model pill or the PM's profile note, and on
/// desktop the keys, right-aligned, wrapping under the rest when the row is narrow. Absent for a
/// pane session's composer, which has none of them.
fn controls(
    app: AppState,
    built: Memo<Composer>,
    list: ListAt,
    row: ElementRef,
    menu: Memo<Option<ModelList>>,
) -> AnyElement {
    let form = app.form;
    let has_targets = computed(move || built.with(|c| !c.targets.is_empty()));
    let pill =
        computed(move || built.with(|c| c.model_pill.as_ref().map(|b| b.label).zip(c.model_name)));
    let note = computed(move || built.with(|c| c.profile_note));
    let keys = computed(move || built.with(|c| c.key_hints));
    // Fixed for a composer's life: Ask always has its switch, a daemon session its pill, a pane
    // session nothing; only what is inside the row follows the target.
    let shown =
        untracked(|| has_targets.get() || pill.with(Option::is_some) || note.with(Option::is_some));
    if !shown {
        return view().into_element();
    }
    view()
        .element_ref(&row)
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .child(when(
            move || has_targets.get(),
            move || untracked(|| target_switch(app, built)),
        ))
        .child(dynamic(move || {
            pill.get()
                .map(|(label, name)| untracked(|| models::pill(app, label, name, list, row, menu)))
        }))
        .child(dynamic(move || {
            note.get().map(|note| {
                small(form, note)
                    .text_color(color(Token::Ink3))
                    .into_element()
            })
        }))
        .child(dynamic(move || {
            keys.get().map(|keys| untracked(|| key_hints(app, keys)))
        }))
        .into_element()
}

/// Assistant | PM: one toggle pressed at a time, one stop of the keyboard focus with the arrow keys
/// moving between them; PM carries its state dot and words. Choosing one closes the model list.
fn target_switch(app: AppState, built: Memo<Composer>) -> AnyElement {
    let form = app.form;
    let options = built.with(|c| c.targets.clone());
    let group = toggle_group().flex_row().items_center();
    let group = if form == Form::Terminal {
        group.gap(px(8.0))
    } else {
        group
            .gap(px(2.0))
            .p(px(2.0))
            .rounded(px(if form == Form::Phone { 9.0 } else { 7.0 }))
            .bg(color(Token::BgSunk))
            .border_1()
            .border_color(color(Token::Rule))
    };
    group
        .children(
            options
                .iter()
                .enumerate()
                .map(|(index, option)| target_item(app, built, index, option.target, option.label)),
        )
        .into_element()
}

fn target_item(
    app: AppState,
    built: Memo<Composer>,
    index: usize,
    target: crate::model::AskTarget,
    label: &'static str,
) -> AnyElement {
    let form = app.form;
    let on = computed(move || built.with(|c| c.targets.get(index).is_some_and(|t| t.selected)));
    let state =
        computed(move || built.with(|c| c.targets.get(index).and_then(|t| t.state.clone())));
    let (height, size, radius) = match form {
        Form::Desktop => (24.0, 12.0, 5.0),
        Form::Phone => (32.0, 14.0, 7.0),
        Form::Terminal => (16.0, text_size(form), 0.0),
    };
    let words = text(label)
        .number_of_lines(1u32)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(move || {
            if on.get() || form == Form::Terminal {
                color(Token::Ink)
            } else {
                color(Token::Ink3)
            }
        });
    let words = if form == Form::Terminal {
        words
    } else {
        words.font_family(FONT_TEXT).text_size(px(size))
    };
    let face = view()
        .test_id(&format!("seg-ask-{}", label.to_lowercase()))
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .child(words)
        .child(dynamic(move || {
            state
                .get()
                .map(|chip| untracked(|| state_mark(app, &chip.label, chip.hue)))
        }));
    toggle()
        .pressed(move || on.get())
        // Choosing the pressed one again keeps it: the switch always names one target.
        .on_change(move |_: &bool| {
            app.ask_target.set(target);
            app.assistant.update(|state| {
                state.close_menu();
            });
        })
        .h(px(height))
        .px(px(if form == Form::Phone { 12.0 } else { 10.0 }))
        .rounded(px(radius))
        .items_center()
        .justify_center()
        .cursor_pointer()
        // In a terminal NSH bands a pressed toggle itself; a fill of the app's would be a second.
        .when(form != Form::Terminal, |t| {
            t.border_1()
                .bg(move || {
                    if on.get() {
                        color(Token::Surface)
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .border_color(move || {
                    if on.get() {
                        color(Token::Rule2)
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .focus_visible(|s| s.border_color(color(Token::Focus)))
        })
        .when(form == Form::Phone, |t| {
            t.hit_slop(EdgeInsets {
                top: px(6.0),
                right: px(0.0),
                bottom: px(6.0),
                left: px(0.0),
            })
        })
        .child(face)
        .into_element()
}

/// The PM's state beside its name (`.pmdot` and its words): a dot in the state's colour, the words
/// quiet. A terminal draws the dot as a glyph.
pub(crate) fn state_mark(app: AppState, label: &str, hue: Hue) -> AnyElement {
    let form = app.form;
    let words = text(label.to_owned())
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    if form == Form::Terminal {
        return view()
            .flex_row()
            .child(text("● ").text_color(hue_ink(hue)))
            .child(words)
            .into_element();
    }
    view()
        .flex_row()
        .items_center()
        .gap(px(3.0))
        .child(
            view()
                .w(px(6.0))
                .h(px(6.0))
                .rounded(px(3.0))
                .bg(hue_ink(hue))
                .accessibility_elements_hidden(true),
        )
        .child(
            words
                .font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone { 13.0 } else { 12.0 })),
        )
        .into_element()
}

/// The desktop keys (`.ckeys`): each key as a cap and what it does, quiet, at the row's end.
fn key_hints(app: AppState, keys: &'static [KeyHint]) -> AnyElement {
    let form = app.form;
    let hints = view()
        .flex_auto()
        .flex_row()
        .flex_wrap()
        .items_center()
        .justify_end()
        .gap(px(8.0));
    if form == Form::Terminal {
        return hints
            .children(keys.iter().map(|hint| term::legend(hint.keys, hint.action)))
            .into_element();
    }
    hints
        .children(keys.iter().map(|hint| {
            view()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .child(key_hint(hint.keys))
                .child(
                    text(hint.action)
                        .number_of_lines(1u32)
                        .font_family(FONT_TEXT)
                        .text_size(px(11.0))
                        .text_color(color(Token::Ink3)),
                )
        }))
        .into_element()
}

/// The context chip (`.achip`) and the note beside it, while a context is attached.
fn chip_row(app: AppState, built: Memo<Composer>) -> AnyElement {
    let attached = computed(move || built.with(|c| c.chip.clone()));
    dynamic(move || {
        attached
            .get()
            .map(|context| untracked(|| context_chip(app, &context)))
    })
}

fn context_chip(app: AppState, context: &ContextChip) -> AnyElement {
    let form = app.form;
    let remove = context.remove.action.clone();
    let remove_label = context.remove.label;
    let kind = text(context.kind)
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    let name = text(context.label.clone())
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .truncate()
        .text_color(color(Token::Ink));
    let note = small(form, context.note).text_color(color(Token::Ink3));
    if form == Form::Terminal {
        let unchip = remove.clone();
        return view()
            .flex_row()
            .flex_wrap()
            .gap(term::cells(1.0))
            .child(
                view()
                    .flex_row()
                    .gap(term::cells(1.0))
                    .px(term::cells(1.0))
                    .bg(color(Token::Surface))
                    .child(kind)
                    .child(name),
            )
            .child(button_with(
                app,
                remove_label,
                ButtonKind::Quiet,
                ButtonOptions::default(),
                "btn-unchip",
                move || act(app, unchip.clone()),
            ))
            .child(note)
            .into_element();
    }
    let phone = form == Form::Phone;
    let size = if phone { 13.0 } else { 12.0 };
    let close = view()
        .test_id("btn-unchip")
        .role(AccessibilityRole::Button)
        .label(remove_label)
        .on_press(move || act(app, remove.clone()))
        .w(px(20.0))
        .h(px(20.0))
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::Focus))
                .outline_offset(px(1.0))
        })
        .when(phone, |v| {
            v.hit_slop(EdgeInsets {
                top: px(12.0),
                right: px(12.0),
                bottom: px(12.0),
                left: px(12.0),
            })
        })
        .child(icon(Icon::Close, 12.0, color(Token::Ink3)));
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .max_w(relative(1.0))
                .h(px(if phone { 30.0 } else { 26.0 }))
                .pl(px(8.0))
                .pr(px(4.0))
                .rounded(px(6.0))
                .bg(color(Token::Surface2))
                .border_1()
                .border_color(color(Token::Rule2))
                .child(icon(Icon::Plans, 13.0, color(Token::Ink2)))
                .child(kind.font_family(FONT_TEXT).text_size(px(size)))
                .child(name.font_family(FONT_TEXT).text_size(px(size)))
                .child(close),
        )
        .child(note)
        .into_element()
}

/// The field: multiline, Enter sends, Shift+Enter breaks the line; always ringed in the focus
/// colour as the mockup draws it, the placeholder naming what it talks to.
fn entry_field(
    app: AppState,
    built: Memo<Composer>,
    entry: ElementRef,
    session: Option<&'static str>,
) -> AnyElement {
    let form = app.form;
    let placeholder = computed(move || built.with(|c| c.placeholder.clone()));
    let field = input()
        .element_ref(&entry)
        .test_id("ask-input")
        .label(move || SharedString::from(placeholder.get()))
        .multiline(true)
        .submit_behavior(SubmitBehavior::Submit)
        .return_key_type(ReturnKeyType::Send)
        .on_submit(move |_: &str| send(app, session))
        .placeholder(move || placeholder.get())
        .value(move || app.ask_draft.get())
        .on_change(move |text: &str| app.ask_draft.set(text.to_owned()))
        .text_color(color(Token::Ink))
        .placeholder_color(move || {
            for_appearance(color(if form == Form::Terminal {
                Token::Ink3
            } else {
                Token::Ink4
            }))
        })
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)));
    match form {
        Form::Desktop => field
            .h(px(64.0))
            .px(px(12.0))
            .py(px(9.0))
            .font_family(FONT_TEXT)
            .text_size(px(14.0))
            .line_height(px(20.0))
            .rounded(px(8.0))
            .bg(color(Token::Surface))
            .border(px(1.5))
            .border_color(color(Token::Focus))
            .into_element(),
        Form::Phone => field
            .h(px(92.0))
            .px(px(12.0))
            .py(px(10.0))
            .font_family(FONT_TEXT)
            .text_size(px(16.0))
            .line_height(px(23.0))
            .rounded(px(10.0))
            .bg(color(Token::Surface))
            .border(px(1.5))
            .border_color(color(Token::Focus))
            .into_element(),
        // NSH draws a field without a fill of its own as a raised band, and underlines it where
        // the terminal has no bands.
        Form::Terminal => field
            .h(px(48.0))
            .px(px(8.0))
            .bg(AdaptiveColor::TRANSPARENT)
            .into_element(),
    }
}

/// The attachments (`.att`) and Send, enabled once the draft has words.
fn foot(app: AppState, built: Memo<Composer>, session: Option<&'static str>) -> AnyElement {
    let form = app.form;
    let attachments = built.with_untracked(|c| c.attachments.clone());
    let send_state = computed(move || built.with(|c| (c.send.label, c.send.enabled)));
    view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 16.0 } else { 4.0 }))
        .children(
            attachments
                .into_iter()
                .enumerate()
                .map(|(index, button)| attachment(app, index, button)),
        )
        .child(view().flex_1())
        .child(dynamic(move || {
            let (label, enabled) = send_state.get();
            untracked(|| {
                button_with(
                    app,
                    label,
                    ButtonKind::Primary,
                    ButtonOptions {
                        disabled: !enabled,
                        ..ButtonOptions::default()
                    },
                    "btn-send",
                    move || send(app, session),
                )
            })
        }))
        .into_element()
}

fn attachment(app: AppState, index: usize, button: assistant::Button) -> AnyElement {
    let form = app.form;
    let action = button.action.clone();
    let face = view()
        .test_id(&format!("btn-attach-{index}"))
        .role(AccessibilityRole::Button)
        .label(button.label)
        .on_press(move || act(app, action.clone()))
        .flex_row()
        .items_center()
        .cursor_pointer();
    if form == Form::Terminal {
        return face
            .child(text(button.label).text_color(color(Token::Ink)))
            .into_element();
    }
    let phone = form == Form::Phone;
    face.gap(px(5.0))
        .h(px(26.0))
        .px(px(8.0))
        .rounded(px(6.0))
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::Focus))
                .outline_offset(px(1.0))
        })
        .when(phone, |v| {
            v.hit_slop(EdgeInsets {
                top: px(9.0),
                right: px(0.0),
                bottom: px(9.0),
                left: px(0.0),
            })
        })
        .children(
            ATTACH_ICONS
                .get(index)
                .map(|glyph| icon(*glyph, 13.0, color(Token::Ink3))),
        )
        .child(
            text(button.label)
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(if phone { 13.0 } else { 12.0 }))
                .text_color(color(Token::Ink3)),
        )
        .into_element()
}

/// Secondary words at the composer's note size (`11.5px`; 13 on a phone; the terminal's own).
fn small(form: Form, words: &'static str) -> Text {
    let words = text(words).number_of_lines(1u32);
    match form {
        Form::Terminal => words,
        Form::Desktop => words.font_family(FONT_TEXT).text_size(px(11.5)),
        Form::Phone => words.font_family(FONT_TEXT).text_size(px(13.0)),
    }
}
