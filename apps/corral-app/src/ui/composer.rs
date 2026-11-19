//! The session composer (`session`), the one composer of every session page and of Ask; the
//! staged attachments with the Screenshot, Link and File controls, the images pasted into its
//! field and the line saying why Send waits on them, which Ask offers always and a session page
//! while its host takes attachments; and what Ask adds: the removable context and project chips
//! and the surface Ask opens in. Ask's draft is its session's own (`draft` keeps it on this
//! device), so the session page shows the same words. A message goes out as `session.send`, Ask's
//! chips' ids as its first line and the session's staged attachments as its refs.

use corral_protocol::assistant::SendAttachment;
use corral_protocol::session_state::SendPolicy;
use nsh_base::{SheetEvent, sheet};
use nsh_core::element::{Input, color as for_appearance};
use nsh_core::prelude::*;

use crate::model::assistant::{self, Composer};
use crate::model::hue::Hue;
use crate::model::session::{Action as SessionAction, SessionComposer};
use crate::model::text::Rich;
use crate::model::{Form, Id, Overlay};
use crate::state::AppState;
use crate::ui::ask::{act, chip_row, drawer, field_row, foot_controls, plus, tray};
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
    let session = id.clone();
    let built = computed(move || {
        // The project brief shows as its own chip, so it is not repeated as the record chip.
        let context = app.composer_record_context();
        let project = app.composer_project();
        let text = app.ask_draft();
        let staged = app.staged(&session);
        app.feed.with(|feed| {
            assistant::composer(feed, &text, &staged, context.as_ref(), project.as_ref())
        })
    });
    Ask { built }
}

/// The attachments a composer offers: Ask's always; a session page's while its host declares
/// them, every control dimmed with the host's reason while it cannot take them now.
#[derive(Clone, Copy)]
pub(crate) struct Attach {
    built: Memo<Composer>,
    /// The controls show: Ask, or a host that declares attachments.
    offered: Memo<bool>,
    /// Why the session takes no attachments now.
    refusal: Memo<Option<String>>,
}

impl Attach {
    fn ask(ask: Ask) -> Attach {
        Attach {
            built: ask.built,
            offered: computed(|| true),
            refusal: computed(|| None),
        }
    }

    /// A session page's: what is staged for session `id`, offered as its host declares.
    fn page(app: AppState, id: &Id, composer: Memo<Option<SessionComposer>>) -> Attach {
        let can =
            computed(move || composer.with(|c| c.as_ref().and_then(|c| c.attachments.clone())));
        let offered = computed(move || can.with(Option::is_some));
        let refusal = computed(move || match can.get() {
            Some(None) => None,
            Some(Some(why)) => Some(why),
            None => Some("this session takes no attachments".to_owned()),
        });
        let session = id.clone();
        let built = computed(move || {
            let text = composer.with(|c| c.as_ref().map(|c| c.draft.clone()).unwrap_or_default());
            let staged = app.staged(&session);
            let mut built = app
                .feed
                .with(|feed| assistant::composer(feed, &text, &staged, None, None));
            if let Some(why) = refusal.get() {
                for button in &mut built.attachments {
                    button.enabled = false;
                }
                built.attach_note = Some(why);
            }
            built
        });
        Attach {
            built,
            offered,
            refusal,
        }
    }
}

/// Stages the images (and files) pasted into session `id`'s composer; the pasted text is the
/// field's own. Refused visibly where the session takes no attachments now.
fn paste(app: AppState, id: &Id, attach: Attach, paste: &PasteEvent) {
    if paste.files.is_empty() && paste.uris.is_empty() {
        return;
    }
    match attach.refusal.get_untracked() {
        Some(why) => app.toast(format!("Not attached: {why}.")),
        None => app.paste(id, paste),
    }
}

/// The refs a session page's message carries: what is staged for session `id`, every upload
/// ready, while its host takes attachments.
fn page_refs(app: AppState, id: &Id, attach: Attach) -> Result<Vec<SendAttachment>, String> {
    let staged = untracked(|| app.staged(id));
    if staged.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(why) = attach.refusal.get_untracked() {
        return Err(format!("Not sent: {why}; remove the attachments to send."));
    }
    if attach.built.with_untracked(|c| c.holds_send.is_some()) {
        return Err(
            "Not sent: an attachment is not ready yet; it is named above the field.".into(),
        );
    }
    app.feed
        .with_untracked(|feed| assistant::send_refs(&staged, feed))
}

/// Sends Ask's draft to session `id` as `session.send`: the chips' line first, the staged
/// attachments as its refs (every upload ready), under a new command id the session model
/// allocates. The draft clears at once; a refusal puts the words back with why.
fn send_ask(app: AppState, ask: Ask, id: &Id, text: String, policy: SendPolicy) {
    if ask.built.with_untracked(|c| c.holds_send.is_some()) {
        app.toast("Not sent: an attachment is not ready yet; it is named above the field.");
        return;
    }
    let staged = untracked(|| app.staged(id));
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
    app.clear_staged(id);
}

/// Over the composer: why Send waits on the attachments, Ask's project and record chips, the
/// staged attachments. A session page shows it only while something is staged.
fn above(app: AppState, id: &Id, ask: Option<Ask>, attach: Attach) -> AnyElement {
    let status = computed(move || attach.built.with(|c| c.status.clone()));
    let session = id.clone();
    let draw = move || {
        view()
            .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
            .child(dynamic(move || {
                status
                    .get()
                    .map(|line| untracked(|| saved_line(app, &line)))
            }))
            .children(ask.map(|ask| chip_row(app, ask.built)))
            .child(tray(app, session.clone(), attach.built))
            .into_element()
    };
    if ask.is_some() {
        return draw();
    }
    let shown = computed(move || {
        attach
            .built
            .with(|c| !c.staged.is_empty() || c.status.is_some())
    });
    when(move || shown.get(), move || untracked(&draw)).into_element()
}

/// Under the composer on desktop and in a terminal while it offers attachments: Screenshot,
/// Link and File, or the open link or path field in their place.
fn attachments(app: AppState, id: &Id, attach: Attach) -> AnyElement {
    let session = id.clone();
    dynamic(move || {
        attach
            .offered
            .get()
            .then(|| untracked(|| foot_controls(app, session.clone(), attach.built)))
    })
    .into_element()
}

/// The phone's "+" while the composer offers attachments.
fn phone_plus(app: AppState, id: &Id, attach: Attach) -> AnyElement {
    let session = id.clone();
    dynamic(move || {
        attach
            .offered
            .get()
            .then(|| untracked(|| plus(app, session.clone(), attach.built)))
    })
    .into_element()
}

/// The phone's open link or path field for session `id`.
fn phone_field(app: AppState, id: &Id) -> AnyElement {
    field_row(app, id.clone())
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
