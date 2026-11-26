//! The chips above the draft and the attachment control in its bar: the removable context chip,
//! the staged tray in the same anatomy (upload progress, ready, failed with the reason,
//! remove), File with why it is dimmed, and the inline path field a terminal opens under the
//! bar while it is open.

use nsh_core::element::color as for_appearance;
use nsh_core::prelude::*;

use crate::model::assistant::{self, AttachField, Composer, ContextChip, PickKind, StagedChip};
use crate::model::hue::Hue;
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::ask::{act, act_on};
use crate::ui::atoms::control_height;
use crate::ui::composer::copyable_input;
use crate::ui::composer::state_mark;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::hint::hinted;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// An attachment control's icon and what a screen reader says for it.
fn face_of(action: &assistant::Action) -> (Icon, &'static str) {
    match action {
        assistant::Action::Attach(PickKind::Image) => (Icon::Review, "Attach an image"),
        assistant::Action::AttachLink => (Icon::Chevron, "Attach a link"),
        _ => (Icon::Plans, "Attach an image or a text file"),
    }
}

/// The chips over the draft (`.achip`), each with its note beside it: the default project the
/// message files under (`btn-unchip-project`), then the record whose brief the assistant gets
/// (`btn-unchip`). Nothing while neither is attached.
pub(crate) fn chip_row(app: AppState, built: Memo<Composer>) -> AnyElement {
    let attached = computed(move || built.with(|c| c.chip.clone()));
    let project = computed(move || built.with(|c| c.project_chip.clone()));
    let gap = if app.form == Form::Terminal { 0.0 } else { 6.0 };
    dynamic(move || {
        let project = project.get();
        let attached = attached.get();
        if project.is_none() && attached.is_none() {
            return None;
        }
        untracked(move || {
            Some(
                view()
                    .gap(px(gap))
                    .children(
                        project.map(|chip| {
                            context_chip(app, &chip, Icon::Project, "btn-unchip-project")
                        }),
                    )
                    .children(
                        attached.map(|chip| context_chip(app, &chip, Icon::Plans, "btn-unchip")),
                    )
                    .into_element(),
            )
        })
    })
}

pub(crate) fn dock_context(app: AppState, session: Id, built: Memo<Composer>) -> AnyElement {
    let chip = computed(move || built.with(|c| c.chip.clone()));
    let available = computed(move || app.current_page_context().is_some());
    let owner = session.clone();
    view()
        .child(when(
            move || available.get(),
            move || {
                let session = owner.clone();
                button(
                    app,
                    crate::model::dock::ATTACH_PAGE,
                    ButtonKind::Quiet,
                    "btn-dock-attach-page",
                    move || {
                        if let Some(context) = untracked(|| app.current_page_context()) {
                            app.set_dock_context(&session, Some(context));
                        }
                    },
                )
            },
        ))
        .child(dynamic(move || {
            let session = session.clone();
            chip.get().map(|chip| {
                untracked(|| {
                    context_chip_with(
                        app,
                        &chip,
                        Icon::Plans,
                        "btn-dock-remove-context",
                        move || app.set_dock_context(&session, None),
                    )
                })
            })
        }))
        .into_element()
}

fn context_chip(app: AppState, context: &ContextChip, glyph: Icon, remove_id: &str) -> AnyElement {
    let remove = context.remove.action.clone();
    context_chip_with(app, context, glyph, remove_id, move || {
        act(app, remove.clone())
    })
}

fn context_chip_with(
    app: AppState,
    context: &ContextChip,
    glyph: Icon,
    remove_id: &str,
    remove: impl Fn() + 'static,
) -> AnyElement {
    let form = app.form;
    let remove_label = context.remove.label;
    let kind = text(context.kind.clone())
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    let name = text(context.label.clone())
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .truncate()
        .text_color(color(Token::Ink));
    let note = note_text(form, context.note).text_color(color(Token::Ink3));
    if form == Form::Terminal {
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
                remove_id,
                remove,
            ))
            .child(note)
            .into_element();
    }
    let phone = form == Form::Phone;
    let size = if phone { 13.0 } else { 12.0 };
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
                .child(icon(glyph, 13.0, color(Token::Ink2)))
                .child(kind.font_family(FONT_TEXT).text_size(px(size)))
                .child(name.font_family(FONT_TEXT).text_size(px(size)))
                .child(close_button(
                    app,
                    remove_id.into(),
                    remove_label.into(),
                    remove,
                )),
        )
        .child(note)
        .into_element()
}

/// A chip's close control: 20 pt, its own hover and focus ring, a phone's larger hit area.
fn close_button(
    app: AppState,
    test_id: String,
    label: String,
    press: impl Fn() + 'static,
) -> AnyElement {
    view()
        .test_id(&test_id)
        .role(AccessibilityRole::Button)
        .label(label)
        .on_press(press)
        .w(px(20.0))
        .h(px(20.0))
        .flex_shrink_0()
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
        .when(app.form == Form::Phone, |v| {
            v.hit_slop(EdgeInsets {
                top: px(12.0),
                right: px(12.0),
                bottom: px(12.0),
                left: px(12.0),
            })
        })
        .child(icon(Icon::Close, 12.0, color(Token::Ink3)))
        .into_element()
}

/// What is staged for `session`, under the context chip: wrapping pills on desktop, one row
/// that scrolls sideways on a phone, a line per item in a terminal. Rows are keyed, so an
/// upload's progress redraws its own pill and a focused Remove keeps its focus.
pub(crate) fn tray(app: AppState, session: Id, built: Memo<Composer>) -> AnyElement {
    let form = app.form;
    let chips = computed(move || built.with(|c| c.staged.clone()));
    let shown = computed(move || chips.with(|chips| !chips.is_empty()));
    when(
        move || shown.get(),
        move || {
            let session = session.clone();
            let rows = each(
                move || chips.get(),
                |chip: &StagedChip| chip.key.clone(),
                move |row: ReadSignal<StagedChip>, _| {
                    let session = session.clone();
                    dynamic(move || row.with(|chip| untracked(|| staged_chip(app, &session, chip))))
                },
            );
            match form {
                Form::Desktop => view()
                    .test_id("ask-attachments")
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .child(rows)
                    .into_element(),
                Form::Phone => scroll()
                    .test_id("ask-attachments")
                    .horizontal()
                    .scrollbar(false)
                    .child(view().flex_row().items_center().gap(px(6.0)).child(rows))
                    .into_element(),
                Form::Terminal => view().test_id("ask-attachments").child(rows).into_element(),
            }
        },
    )
    .into_element()
}

/// A test id part from a staged key (an upload id or a link).
fn slug(key: &str) -> String {
    key.chars()
        .take(48)
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn glyph(kind: &str) -> Icon {
    match kind {
        "image" => Icon::Review,
        "link" => Icon::Chevron,
        _ => Icon::Plans,
    }
}

/// One staged item: its kind, name and size, where its upload stands, and Remove.
fn staged_chip(app: AppState, session: &Id, chip: &StagedChip) -> AnyElement {
    let form = app.form;
    let id = slug(&chip.key);
    let remove = chip.remove.clone();
    let session = session.clone();
    let unattach = move || act_on(app, &session, remove.clone());
    let kind = text(chip.kind)
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    let name = text(chip.label.clone())
        .number_of_lines(1u32)
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .truncate()
        .text_color(color(Token::Ink));
    let meta = (!chip.meta.is_empty()).then(|| {
        text(chip.meta.clone())
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3))
    });
    if form == Form::Terminal {
        let state = match &chip.state {
            Some(state) => state_mark(app, &state.label, state.hue),
            None => text("ready").text_color(hue_ink(Hue::Ok)).into_element(),
        };
        return view()
            .test_id(&format!("chip-attach-{id}"))
            .flex_row()
            .flex_wrap()
            .gap(term::cells(1.0))
            .child(text("▸").text_color(color(Token::Ink3)))
            .child(kind)
            .child(name)
            .children(meta)
            .child(state)
            .child(button_with(
                app,
                "Remove",
                ButtonKind::Quiet,
                ButtonOptions::default(),
                format!("btn-unattach-{id}"),
                unattach,
            ))
            .into_element();
    }
    let phone = form == Form::Phone;
    let size = if phone { 13.0 } else { 12.0 };
    let close = close_button(
        app,
        format!("btn-unattach-{id}"),
        chip.remove_label.clone(),
        unattach,
    );
    view()
        .test_id(&format!("chip-attach-{id}"))
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .flex_shrink_0()
        .h(px(if phone { 30.0 } else { 26.0 }))
        .pl(px(8.0))
        .pr(px(4.0))
        .rounded(px(6.0))
        .bg(color(Token::Surface2))
        .border_1()
        .border_color(color(Token::Rule2))
        .child(icon(glyph(chip.kind), 13.0, color(Token::Ink2)))
        .child(kind.font_family(FONT_TEXT).text_size(px(size)))
        .child(
            name.max_w(px(if phone { 140.0 } else { 180.0 }))
                .font_family(FONT_TEXT)
                .text_size(px(size)),
        )
        .children(meta.map(|meta| meta.font_family(FONT_TEXT).text_size(px(size))))
        .children(
            chip.state
                .as_ref()
                .map(|state| state_mark(app, &state.label, state.hue)),
        )
        .child(close)
        .into_element()
}

/// The attachment controls in the composer's bar (File), each dimmed with why as its hint.
/// Desktop and terminal; a phone keeps File as its "+" ([`plus`]).
pub(crate) fn controls(app: AppState, session: Id, built: Memo<Composer>) -> AnyElement {
    let form = app.form;
    let count = built.with_untracked(|c| c.attachments.len());
    view()
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .gap(px(if form == Form::Terminal { 16.0 } else { 4.0 }))
        .children((0..count).map(|index| control(app, &session, built, index)))
        .into_element()
}

/// The phone's "+" at the start of the composer's row: a full touch target that runs File (the
/// system picker). Dimmed while it cannot, it says why to screen readers and, as the phone has
/// no hover, in a toast when pressed. Nothing when the composer offers no attachments.
pub(crate) fn plus(app: AppState, session: Id, built: Memo<Composer>) -> AnyElement {
    let state = computed(move || {
        built.with(|c| {
            c.attachments
                .first()
                .map(|b| (b.action.clone(), b.enabled, c.attach_note.clone()))
        })
    });
    dynamic(move || {
        state.get().map(|(action, enabled, note)| {
            let session = session.clone();
            untracked(|| {
                let (_, spoken) = face_of(&action);
                let why = format!(
                    "Not attached: {}.",
                    note.as_deref().unwrap_or("unavailable")
                );
                let spoken = match (enabled, note) {
                    (true, _) => spoken.to_owned(),
                    (false, Some(note)) => format!("{spoken}, unavailable: {note}"),
                    (false, None) => format!("{spoken}, unavailable"),
                };
                view()
                    .test_id("btn-attach")
                    .role(AccessibilityRole::Button)
                    .label(spoken)
                    .size(px(TARGET))
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(color(Token::Rule2))
                    .bg(color(Token::Surface))
                    .focus_visible(|s| {
                        s.outline(px(2.0))
                            .outline_color(color(Token::Focus))
                            .outline_offset(px(2.0))
                    })
                    .active(|s| s.bg(color(Token::Sel)))
                    .on_press(move || {
                        if enabled {
                            act_on(app, &session, action.clone());
                        } else {
                            app.toast(why.clone());
                        }
                    })
                    .when(!enabled, |v| v.opacity(0.45))
                    .child(icon(Icon::Plus, 20.0, color(Token::Ink2)))
                    .into_element()
            })
        })
    })
    .into_element()
}

/// The phone's touch target.
const TARGET: f32 = 44.0;

/// One control, redrawn when it is enabled or dimmed. A dimmed one takes no press and says why,
/// as its hint and to screen readers.
fn control(app: AppState, session: &Id, built: Memo<Composer>, index: usize) -> AnyElement {
    let state = computed(move || {
        built.with(|c| {
            c.attachments
                .get(index)
                .map(|b| (b.label, b.action.clone(), b.enabled, c.attach_note.clone()))
        })
    });
    let session = session.clone();
    dynamic(move || {
        state.get().map(|(label, action, enabled, note)| {
            let session = session.clone();
            untracked(|| {
                let form = app.form;
                let (glyph, spoken) = face_of(&action);
                let spoken = if enabled {
                    spoken.to_owned()
                } else {
                    match &note {
                        Some(note) => format!("{spoken}, unavailable: {note}"),
                        None => format!("{spoken}, unavailable"),
                    }
                };
                let face = view()
                    .test_id(&format!("btn-attach-{index}"))
                    .role(AccessibilityRole::Button)
                    .label(spoken)
                    .flex_row()
                    .items_center()
                    .when(enabled, |v| {
                        v.on_press(move || act_on(app, &session, action.clone()))
                            .cursor_pointer()
                    })
                    .when(!enabled, |v| v.opacity(0.45));
                let face = if form == Form::Terminal {
                    let ink = if enabled { Token::Ink } else { Token::Ink3 };
                    face.child(text(label).text_color(color(ink)))
                        .into_element()
                } else {
                    face.when(enabled, |v| v.hover(|s| s.bg(color(Token::Hover))))
                        .focus_visible(|s| {
                            s.outline(px(2.0))
                                .outline_color(color(Token::Focus))
                                .outline_offset(px(1.0))
                        })
                        .gap(px(5.0))
                        .h(px(control_height(26.0)))
                        .px(px(8.0))
                        .rounded(px(6.0))
                        .child(icon(glyph, 13.0, color(Token::Ink3)))
                        .child(
                            text(label)
                                .number_of_lines(1u32)
                                .font_family(FONT_TEXT)
                                .text_size(px(12.0))
                                .text_color(color(Token::Ink3)),
                        )
                        .into_element()
                };
                match note.filter(|_| !enabled) {
                    Some(note) => hinted(app, face, note),
                    None => face,
                }
            })
        })
    })
}

/// The open link or path field: `link › [Paste a link] Attach Cancel`, Enter attaches and Esc
/// cancels; the reason under it when what was typed is not attached. Nothing while closed or
/// open in another session's composer. Leaving the composer closes its open field, so a field
/// out of sight never takes the next Esc.
pub(crate) fn field_row(app: AppState, session: Id) -> AnyElement {
    let owner = session.clone();
    cleanup(move || {
        // App signals outlive the page; a disposed one is let go of quietly.
        let open = app.ask_field.try_get().flatten();
        if open.is_some_and(|(at, _)| at == owner) {
            app.close_attach_field();
        }
    });
    let field = computed(move || {
        app.ask_field.with(|open| {
            open.as_ref()
                .filter(|(owner, _)| *owner == session)
                .map(|(_, field)| *field)
        })
    });
    dynamic(move || {
        field
            .get()
            .map(|field| untracked(|| field_view(app, field)))
    })
}

fn field_view(app: AppState, field: AttachField) -> AnyElement {
    let form = app.form;
    let phone = form == Form::Phone;
    let terminal = form == Form::Terminal;
    // In a side pane set aside the field is hidden: it gives the keyboard back, keeping what
    // was typed, and its Enter attaches nothing until the pane comes back.
    let handle = element_ref();
    app.release_while_aside(handle);
    let entry = copyable_input(app, Some(handle), move || app.ask_field_text.get())
        .test_id(match field {
            AttachField::Link => "attach-link-input",
            AttachField::Path(_) => "attach-path-input",
        })
        .label(field.placeholder())
        .placeholder(field.placeholder().to_owned())
        .on_change(move |text: &str| {
            app.ask_field_text.set(text.to_owned());
            app.ask_field_error.set(None);
        })
        .on_submit(move |_: &str| {
            if app.field_acts() {
                app.submit_attach_field();
            }
        })
        .return_key_type(ReturnKeyType::Done)
        .keyboard_type(if field == AttachField::Link {
            KeyboardType::Url
        } else {
            KeyboardType::Default
        })
        .auto_capitalize(AutoCapitalize::None)
        .auto_correct(false)
        .auto_focus(true)
        .text_color(color(Token::Ink))
        .placeholder_color(move || {
            for_appearance(color(if terminal { Token::Ink3 } else { Token::Ink4 }))
        })
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .flex_1()
        .min_w(px(0.0));
    let entry = if terminal {
        entry.h(px(16.0)).bg(AdaptiveColor::TRANSPARENT)
    } else {
        entry
            .h(px(if phone { TARGET } else { 26.0 }))
            .px(px(8.0))
            .font_family(FONT_TEXT)
            .text_size(px(if phone { 16.0 } else { 12.5 }))
            .rounded(px(6.0))
            .bg(color(Token::Surface))
            .border_1()
            .border_color(color(Token::Focus))
    };
    let attach = button_with(
        app,
        "Attach",
        ButtonKind::Ghost,
        ButtonOptions::default(),
        "btn-attach-field",
        move || app.submit_attach_field(),
    );
    let cancel = button_with(
        app,
        "Cancel",
        ButtonKind::Quiet,
        ButtonOptions {
            key_hint: Some("Esc"),
            ..ButtonOptions::default()
        },
        "btn-attach-cancel",
        move || app.close_attach_field(),
    );
    let row = view()
        .flex_row()
        .items_center()
        .gap(if terminal { term::cells(1.0) } else { px(6.0) })
        .when(terminal, |r| {
            r.child(text(format!("{} ›", field.lead())).text_color(color(Token::Ink3)))
        })
        .child(entry)
        .child(attach)
        .child(cancel);
    view()
        .gap(px(if terminal { 0.0 } else { 4.0 }))
        .child(row)
        .child(dynamic(move || {
            app.ask_field_error.get().map(|why| {
                note_text(form, why)
                    .number_of_lines(2u32)
                    .text_color(hue_ink(Hue::Fail))
                    .into_element()
            })
        }))
        .into_element()
}

/// Secondary words at the composer's note size (11.5 pt; 13 on a phone; the terminal's own).
fn note_text(form: Form, words: impl Into<String>) -> Text {
    let words = text(words.into()).number_of_lines(1u32);
    match form {
        Form::Terminal => words,
        Form::Desktop => words.font_family(FONT_TEXT).text_size(px(11.5)),
        Form::Phone => words.font_family(FONT_TEXT).text_size(px(13.0)),
    }
}
