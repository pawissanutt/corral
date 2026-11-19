//! The session composer (`session`), the one composer of every session page and of Ask, and
//! what Ask adds to it: the removable context and project chips, the staged attachments with the
//! Screenshot, Link and File controls, the line saying why Send waits on them, and the surface
//! Ask opens in. Ask's draft is its session's own (`draft` keeps it on this device), so the
//! session page shows the same words. Its message goes out as `session.send`, its chips' ids as
//! the message's first line and the staged attachments as its refs.

use corral_protocol::session_state::SendPolicy;
use nsh_base::{SheetEvent, sheet};
use nsh_core::element::{Input, color as for_appearance};
use nsh_core::prelude::*;

use crate::model::assistant::{self, Composer};
use crate::model::hue::Hue;
use crate::model::session::Action as SessionAction;
use crate::model::text::Rich;
use crate::model::{Form, Id, Overlay};
use crate::state::AppState;
use crate::ui::ask::{act, chip_row, drawer, foot_controls, tray};
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::text::rich_small;
use crate::ui::tokens::{FONT_TEXT, Token, color};

mod draft;
mod session;

pub(crate) use session::{ask_composer, session_composer};

/// What Ask adds to its session's composer, read through one memo so typing redraws only what
/// the draft changes.
#[derive(Clone, Copy)]
pub(crate) struct Ask {
    pub(crate) built: Memo<Composer>,
}

/// Ask's additions for session `id`, whose draft this device keeps meanwhile.
fn ask(app: AppState, id: &Id) -> Ask {
    draft::keep_draft(app, id.clone());
    let built = computed(move || {
        // The project brief shows as its own chip, so it is not repeated as the record chip.
        let context = app.composer_record_context();
        let project = app.composer_project();
        let text = app.ask_draft();
        app.ask_staged.with(|staged| {
            app.feed.with(|feed| {
                assistant::composer(feed, &text, staged, context.as_ref(), project.as_ref())
            })
        })
    });
    Ask { built }
}

/// Sends Ask's draft to session `id` as `session.send`: the chips' line first, the staged
/// attachments as its refs (every upload ready), under a new command id the session model
/// allocates. The draft clears at once; a refusal puts the words back with why.
fn send_ask(app: AppState, ask: Ask, id: &Id, text: String, policy: SendPolicy) {
    if ask.built.with_untracked(|c| c.holds_send.is_some()) {
        app.toast("Not sent: an attachment is not ready yet; it is named above the field.");
        return;
    }
    let staged = app.ask_staged.get_untracked();
    let attachments = match app
        .feed
        .with_untracked(|feed| assistant::send_refs(&staged, feed))
    {
        Ok(refs) => refs,
        Err(why) => {
            app.toast(why);
            return;
        }
    };
    let (context, project) = untracked(|| (app.composer_record_context(), app.composer_project()));
    let text = assistant::with_context(&text, context.as_ref(), project.as_ref());
    let session = id.clone();
    app.reduce(app.session, move |state, feed, _, _| {
        state.act(
            SessionAction::Send {
                session,
                text,
                policy,
                attachments,
            },
            feed,
        )
    });
    app.ask_staged.set(Vec::new());
}

/// Over Ask's composer: why Send waits on the attachments, the project and record chips, the
/// staged attachments.
fn above(app: AppState, ask: Ask) -> AnyElement {
    let status = computed(move || ask.built.with(|c| c.status.clone()));
    view()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(dynamic(move || {
            status
                .get()
                .map(|line| untracked(|| saved_line(app, &line)))
        }))
        .child(chip_row(app, ask.built))
        .child(tray(app, ask.built))
        .into_element()
}

/// Under Ask's composer on desktop and in a terminal: Screenshot, Link and File, or the open
/// link or path field in their place.
fn attachments(app: AppState, ask: Ask) -> AnyElement {
    foot_controls(app, ask.built)
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

/// Each opening of Ask bumps `app.ask_focus`. The field is made before it is mounted, so a focus
/// sent from this run would reach nothing: `after_answer` waits for the commit after the one that
/// mounts it, which is also after a closing dialog (the palette running Ask) hands its focus
/// back, so this focus lands last. The caret goes after any draft the opening brought.
pub(crate) fn focus_on_open(app: AppState, entry: ElementRef) {
    effect(move || {
        app.ask_focus.with(|_| ());
        after_answer(move || {
            let end = untracked(|| app.ask_draft()).encode_utf16().count();
            let end = u32::try_from(end).unwrap_or(u32::MAX);
            entry.focus();
            entry.set_selection(end, end);
        });
    });
}

/// A status line above the composer (`.savedline`): its first run as a chip in its colour, the
/// sentence after it.
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

/// The phone's least touch target and the tallest its field grows before it scrolls.
const TARGET: f32 = 44.0;
const FIELD_MAX: f32 = 132.0;

/// The drawn height of a chip in the phone composer's control row (model, effort, receiver).
pub(crate) const PHONE_CHIP: f32 = 30.0;

/// What a phone chip adds above and below its drawn height so its touch target is `TARGET`.
pub(crate) fn chip_slop() -> EdgeInsets {
    let slop = px((TARGET - PHONE_CHIP) / 2.0);
    Edges {
        top: slop,
        right: px(0.0),
        bottom: slop,
        left: px(0.0),
    }
}

/// The look of the field Ask keeps while it has no connection: its inks, size and focus ring.
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
            .min_h(px(TARGET))
            .max_h(px(FIELD_MAX))
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
