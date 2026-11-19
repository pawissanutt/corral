//! The composers: Ask's and the session page's (`session`), and the surface Ask opens in.
//! Ask's is one field and one Send: the model pill and its list, the "answered by" line after a
//! switch, the removable context chip and the staged attachments, the offline or waiting line,
//! the Screenshot, Link and File controls. Enter sends and Shift+Enter breaks the line; Send
//! hands the draft and attachment refs to the assistant model, which stores the message with its
//! state at once; the draft and the attachments clear once the device outbox accepted it. A
//! phone folds all of it into one row and one chip above it (`phone`).

use nsh_base::{SheetEvent, sheet};
use nsh_core::element::{Input, color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::assistant::{self, Composer, KeyHint, ModelList, ThreadRef};
use crate::model::hue::Hue;
use crate::model::text::Rich;
use crate::model::{Form, Overlay};
use crate::state::AppState;
use crate::ui::ask::{act, chip_row, drawer, foot_controls, models, tray};
use crate::ui::atoms::key_hint;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::text::rich_small;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

mod draft;
mod pending;
mod phone;
mod send;
mod session;

use draft::keep_draft;
use send::send;

pub(crate) use session::session_composer;

/// Where a composer's model list opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListAt {
    /// A popup above the control row: the drawer, a thread, a terminal.
    Above,
    /// A popup below the control row: the hotkey window.
    Below,
    /// In the flow, in the chooser under the who chip: the phone sheet and a phone thread.
    Inline,
}

/// What a composer is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Host {
    /// The thread this composer sends to: Ask's quick thread or a named session's own.
    pub thread: ThreadRef,
    /// Where the model list opens.
    pub list: ListAt,
    /// Under a thread page rather than in Ask: it never takes the focus when Ask opens.
    pub docked: bool,
}

/// Ask's composer: the drawer's on desktop, the sheet's on a phone, the pane's in a terminal.
/// Reads and writes `app.ask_draft`; focuses its field whenever
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
            thread: ThreadRef::quick(),
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
        thread,
        list,
        docked,
    } = host;
    let named = !thread.is_quick();
    // Ask and the quick thread's docked composer share the app's draft; a named session's
    // composer keeps its own, owned by the root so its words outlive the composer. Words typed
    // for one thread are never saved under another.
    let ThreadRef::Live(scope) = &thread;
    let draft = if named {
        pending::named_draft(app, scope)
    } else {
        app.ask_draft
    };
    let source = thread.clone();
    let built = computed(move || {
        let env = app.env();
        let ThreadRef::Live(scope) = &source;
        // The project brief shows as its own chip, so it is not repeated as the record chip.
        let context = app.composer_record_context();
        let project = app.composer_project();
        draft.with(|text| {
            app.ask_staged.with(|staged| {
                app.feed.with(|feed| {
                    app.assistant.with(|state| {
                        assistant::composer_with_project(
                            state,
                            feed,
                            assistant::Draft {
                                scope: scope.as_str(),
                                text,
                                context: context.as_ref(),
                                staged,
                            },
                            project.as_ref(),
                            env,
                        )
                    })
                })
            })
        })
    });
    let entry = element_ref();
    let row = element_ref();
    if !docked {
        focus_on_open(app, entry);
    }
    keep_draft(app, scope.clone(), draft);
    let menu = computed(move || built.with(|c| c.model_menu.clone()));
    let status = computed(move || built.with(|c| c.status.clone()));
    let next = computed(move || built.with(|c| c.next_message.clone()));
    let terminal = form == Form::Terminal;
    let to = To {
        thread,
        docked,
        draft,
    };
    let status = dynamic(move || {
        status
            .get()
            .map(|line| untracked(|| saved_line(app, &line)))
    });
    if form == Form::Phone {
        return phone::composer(app, built, entry, to, status, next, menu);
    }
    let shell = view().gap(px(if terminal { 0.0 } else { 8.0 }));
    let shell = if terminal {
        shell.px(term::cells(1.0))
    } else {
        shell
            .px(px(14.0))
            .pt(px(10.0))
            .pb(px(12.0))
            .bg(color(Token::Bg))
            .border_t(px(1.0))
            .border_color(color(Token::Rule))
    };
    shell
        .when(terminal, |s| s.child(term::blank()))
        .child(status)
        .child(dynamic(move || {
            next.get().map(|line| untracked(|| saved_line(app, &line)))
        }))
        .child(controls(app, built, list, row, menu, docked))
        .child(chip_row(app, built))
        .child(tray(app, built))
        .when(terminal, |s| s.child(term::blank()))
        .child(entry_field(app, built, entry, to.clone()))
        .when(terminal, |s| s.child(term::blank()))
        .child(foot(app, built, to))
        .when(terminal, |s| s.child(term::blank()))
        .into_element()
}

/// Each opening of Ask bumps `app.ask_focus`. The field is made before it is mounted, so a focus
/// sent from this run would reach nothing: `after_answer` waits for the commit after the one that
/// mounts it, which is also after a closing dialog (the palette running Ask) hands its focus
/// back, so this focus lands last. The caret goes after any draft the opening brought.
pub(crate) fn focus_on_open(app: AppState, entry: ElementRef) {
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

/// Where Send delivers: the composer's thread, whether it is docked under a thread page, and the
/// draft it sends from.
#[derive(Debug, Clone)]
struct To {
    thread: ThreadRef,
    docked: bool,
    draft: Signal<String>,
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

/// The control row (`.crow`): the model pill or the session's profile note, and on
/// desktop the keys, right-aligned, wrapping under the rest when the row is narrow. Absent for a
/// pane session's composer, which has none of them. Ask's own pane in a terminal shows no keys:
/// the shell's key line names them while its field has the focus.
fn controls(
    app: AppState,
    built: Memo<Composer>,
    list: ListAt,
    row: ElementRef,
    menu: Memo<Option<ModelList>>,
    docked: bool,
) -> AnyElement {
    let form = app.form;
    let pill = computed(move || {
        built.with(|c| {
            c.model_pill
                .as_ref()
                .map(|b| b.label)
                .zip(c.model_name.clone())
        })
    });
    let note = computed(move || built.with(|c| c.profile_note.clone()));
    let own_keys = form != Form::Terminal || docked;
    let keys = computed(move || built.with(|c| c.key_hints.filter(|_| own_keys)));
    // Ask always has its pill or profile note, a pane session neither. A thread page can mount
    // before its row or the configured profiles arrive, so the row follows them instead of
    // being fixed at mount.
    let shown = move || pill.with(Option::is_some) || note.with(Option::is_some);
    when(shown, move || {
        untracked(|| {
            view()
                .element_ref(&row)
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(8.0))
                .child(dynamic(move || {
                    pill.get().map(|(label, name)| {
                        untracked(|| models::pill(app, label, name, list, row, menu))
                    })
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
        })
    })
    .into_element()
}

/// A state beside its name: a dot in the state's colour, the words quiet. A terminal draws the
/// dot as a glyph.
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

/// The field: multiline, Enter sends, Shift+Enter breaks the line; always ringed in the focus
/// colour as the mockup draws it, the placeholder naming what it talks to. A phone's starts one
/// line high and grows with the draft to five lines, then scrolls.
fn entry_field(app: AppState, built: Memo<Composer>, entry: ElementRef, to: To) -> AnyElement {
    let form = app.form;
    let placeholder = computed(move || built.with(|c| c.placeholder.clone()));
    let draft = to.draft;
    app.focus_when_asked(assistant::COMPOSER_FIELD, entry);
    let field = input()
        .element_ref(&entry)
        .test_id(assistant::COMPOSER_FIELD)
        .label(move || SharedString::from(placeholder.get()))
        .multiline(true)
        .submit_behavior(SubmitBehavior::Submit)
        .return_key_type(ReturnKeyType::Send)
        .on_submit(move |_: &str| {
            // Enter does what Send does: a served thread that takes no message (ended, ending,
            // over the limit, uploads not ready) sends nothing; the status line says why.
            if built.with_untracked(|c| c.send.enabled) {
                send(app, &to);
            }
        })
        .placeholder(move || placeholder.get())
        .value(move || draft.get())
        .on_change(move |text: &str| draft.set(text.to_owned()));
    field_look(form, field)
}

/// The composer field's look for the form: its inks, size and focus ring. Shared with the field
/// Ask keeps while it has no connection.
pub(crate) fn field_look(form: Form, field: Input) -> AnyElement {
    let field = field
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
            .min_h(px(phone::TARGET))
            .max_h(px(phone::FIELD_MAX))
            .px(px(12.0))
            .py(px(9.0))
            .font_family(FONT_TEXT)
            .text_size(px(16.0))
            .line_height(px(22.0))
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

/// The attachment controls, or the open link or path field, and Send, enabled once the draft
/// has words and every staged attachment is ready.
fn foot(app: AppState, built: Memo<Composer>, to: To) -> AnyElement {
    let form = app.form;
    let send_state = computed(move || built.with(|c| (c.send.label, c.send.enabled)));
    view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 16.0 } else { 4.0 }))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(foot_controls(app, built)),
        )
        .child(dynamic(move || {
            let (label, enabled) = send_state.get();
            let to = to.clone();
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
                    move || send(app, &to),
                )
            })
        }))
        .into_element()
}

/// Secondary words at the composer's note size (`11.5px`; 13 on a phone; the terminal's own).
fn small(form: Form, words: impl Into<String>) -> Text {
    let words = text(words.into()).number_of_lines(1u32);
    match form {
        Form::Terminal => words,
        Form::Desktop => words.font_family(FONT_TEXT).text_size(px(11.5)),
        Form::Phone => words.font_family(FONT_TEXT).text_size(px(13.0)),
    }
}
