//! Where Ask opens: the drawer docked on the right over the current screen on desktop, a pane of
//! the frame in a terminal, a sheet riding on the keyboard on a phone. Each is Ask's head over
//! the session Ask is keyed to (`quick` until its first send is answered, then that chat) in the
//! session page's compact form (screens §11): the transcript, the dock, the composer and the
//! status row; the phone sheet shows the newest reply in the transcript's place. The head names
//! the session and its state and carries End thread (asked once on the confirm sheet), Open
//! session (the same session's page, with the same draft and rows) and Close. A new thread
//! (`quick` with no chat) has no state yet. Without a connection each shows why nothing can be
//! sent and the way to connect.

use nsh_core::prelude::*;

use crate::model::assistant::{self, Action, EndState};
use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::session::{SessionPage, StateWord};
use crate::model::{DataState, Effect, Form, Id, Live};
use crate::screens::agents::{Compact, ask_session, session_view};
use crate::state::AppState;
use crate::ui::ask::{act, unconnected};
use crate::ui::atoms::control_height;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::page_phone::short_window;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

/// The drawer's width (`.drawer`).
const DRAWER_WIDTH: f32 = 500.0;
/// The sheet's largest share of the screen (`.asksheet`).
const SHEET_SHARE: f32 = 0.92;
/// The key that closes Ask, as the terminal's key line names it.
const ESC: &str = "Esc";

/// Whether the chat Ask shows can be ended now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum End {
    /// Nothing to end: no chat yet, or it ended.
    None,
    /// `End thread` ends this chat.
    Ready(String),
    /// Sent; corrald has not said it ended yet.
    Pending,
}

/// Ask's head, as the session model and the chat rows say it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Head {
    title: String,
    word: Option<StateWord>,
    end: End,
    /// corrald refused the last End, with why.
    refused: Option<String>,
}

/// The head of Ask on session `id`, tracked.
fn head(app: AppState, id: Id, page: Memo<SessionPage>) -> Memo<Head> {
    computed(move || {
        let phone = app.form == Form::Phone;
        let (shown, word) = page.with(|p| (p.header.shown_id.clone(), p.header.state.clone()));
        let (end, refused) = app.feed.with(|feed| {
            app.assistant.with(|state| {
                let target = assistant::end_target(feed, id.as_str())
                    .filter(|chat| !assistant::ended(feed, chat));
                let pending = assistant::end_state(state, feed, id.as_str());
                let end = match (&target, &pending) {
                    (Some(_), Some(EndState::Pending)) => End::Pending,
                    (Some(chat), _) => End::Ready(chat.clone()),
                    (None, _) => End::None,
                };
                let refused = match pending {
                    Some(EndState::Failed(why)) => Some(why),
                    _ => None,
                };
                (end, refused)
            })
        });
        // `quick` with no current chat is a new thread: nothing runs yet to have a state.
        let new_thread = id.as_str() == assistant::QUICK
            && app
                .feed
                .with(|feed| assistant::row(feed, assistant::QUICK).is_none());
        Head {
            title: match (phone, new_thread) {
                (true, _) => "Ask".into(),
                (false, true) => "Ask · new thread".into(),
                (false, false) => format!("Ask · {shown}"),
            },
            word: (!phone && !new_thread).then_some(word),
            end,
            refused,
        }
    })
}

/// The session Ask shows, tracked: it changes only when Ask is keyed to another one, or, while
/// Ask is on `quick`, when the chat `quick` names changes (it ended, or a new one started), so
/// the quick page is read again.
fn keyed(app: AppState) -> Memo<(Id, Option<String>)> {
    computed(move || {
        let id = app.ask_id();
        let quick = (id.as_str() == assistant::QUICK)
            .then(|| app.feed.with(assistant::quick_id))
            .flatten();
        (id, quick)
    })
}

/// Ask's head and its session in `form`, made again whenever Ask is keyed to another session;
/// a chat's cold host is asked to start as Ask opens on it.
fn keyed_session(
    app: AppState,
    form: Compact,
    head_of: fn(AppState, Memo<Head>, Id) -> AnyElement,
) -> AnyElement {
    let keyed = keyed(app);
    dynamic(move || {
        let (id, _) = keyed.get();
        untracked(|| {
            let page = session_view(app, id.clone());
            let warmed = id.clone();
            app.reduce(app.assistant, move |state, feed, _, _| {
                state.warm(warmed.as_str(), feed)
            });
            let head = head(app, id.clone(), page);
            view()
                .flex_1()
                .min_h(px(0.0))
                .flex_col()
                .child(dynamic({
                    let id = id.clone();
                    move || {
                        let _ = head.get();
                        untracked(|| head_of(app, head, id.clone()))
                    }
                }))
                .child(refused_end(app, head))
                .child(ask_session(app, id, page, form))
                .into_element()
        })
    })
    .into_element()
}

/// Ask's composer alone (the hotkey window), on the session Ask is keyed to.
pub(crate) fn composer_only(app: AppState) -> AnyElement {
    let keyed = keyed(app);
    dynamic(move || {
        let (id, _) = keyed.get();
        untracked(|| {
            let page = session_view(app, id.clone());
            ask_session(app, id, page, Compact::Composer)
        })
    })
    .into_element()
}

/// The state word: the running mark or its glyph in its hue, then its words.
fn state_word(app: AppState, word: &StateWord) -> AnyElement {
    let form = app.form;
    let ink = if word.hue == Hue::Neutral {
        color(Token::Ink3)
    } else {
        hue_ink(word.hue)
    };
    let lead = if word.spinner {
        Some(mark(app, Mark::Active, word.hue))
    } else {
        word.glyph
            .map(|glyph| text(glyph).text_color(hue_ink(word.hue)).into_element())
    };
    let words = text(word.words.clone())
        .number_of_lines(1u32)
        .truncate()
        .min_w(px(0.0))
        .text_color(ink);
    view()
        .flex_row()
        .items_center()
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .gap(px(if form == Form::Terminal { 8.0 } else { 5.0 }))
        .children(lead)
        .child(if form == Form::Terminal {
            words.into_element()
        } else {
            words
                .font_family(FONT_TEXT)
                .text_size(px(12.0))
                .into_element()
        })
        .into_element()
}

/// Open session ↗: the same session's page; Ask closes as it opens.
fn open_session(app: AppState, id: Id, kind: ButtonKind) -> AnyElement {
    button(
        app,
        "Open session ↗",
        kind,
        "btn-open-session",
        move || act(app, Action::OpenLive(Live::Session(id.clone()))),
    )
}

/// End thread, while there is a chat to end: it asks once below the head first.
fn end_button(app: AppState, end: &End, kind: ButtonKind) -> Option<AnyElement> {
    match end {
        End::None => None,
        End::Pending => Some(button_with(
            app,
            "Ending…",
            kind,
            ButtonOptions {
                disabled: true,
                ..ButtonOptions::default()
            },
            "btn-end-thread",
            || {},
        )),
        End::Ready(chat) => {
            let chat = chat.clone();
            Some(button(
                app,
                "End thread",
                kind,
                "btn-end-thread",
                move || {
                    app.run(vec![Effect::EndThread(chat.clone())]);
                },
            ))
        }
    }
}

/// Under the head after corrald refused End thread: why, and that the thread goes on. Ending
/// itself asks once on the confirm sheet, as from the session page's ⋯ menu.
fn refused_end(app: AppState, head: Memo<Head>) -> AnyElement {
    let refused = computed(move || head.with(|h| h.refused.clone()));
    dynamic(move || {
        refused.get().map(|why| {
            untracked(|| {
                let form = app.form;
                let line = text(format!("Not ended: {why}. The thread goes on."))
                    .text_color(color(Token::Ink2));
                let line = if form == Form::Terminal {
                    line.into_element()
                } else {
                    line.font_family(FONT_TEXT)
                        .text_size(px(12.5))
                        .line_height(px(18.0))
                        .into_element()
                };
                let row = view().test_id("ask-end").child(line);
                match form {
                    Form::Terminal => row.px(term::cells(1.0)).into_element(),
                    _ => row
                        .px(px(16.0))
                        .py(px(10.0))
                        .bg(color(Token::Surface2))
                        .border_b(px(1.0))
                        .border_color(color(Token::Rule))
                        .into_element(),
                }
            })
        })
    })
    .into_element()
}

/// The desktop drawer: absolute on the right of its parent, 500 pt wide, the page beside it still
/// visible and usable; the head, then the session.
pub(crate) fn desktop(app: AppState) -> AnyElement {
    // One short slide from the right edge, eased out; reduced motion skips it.
    let entrance = motion().entering(
        MotionSpec::new(KeyframeTiming::new(200.0).easing(Easing::Bezier(0.16, 1.0, 0.3, 1.0)))
            .values(MotionChannel::TranslateX, [24.0, 0.0])
            .values(MotionChannel::Opacity, [0.5, 1.0]),
    );
    view()
        .absolute()
        .top(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .w(px(DRAWER_WIDTH))
        .max_w(relative(1.0))
        .role(AccessibilityRole::Region)
        .label("Ask")
        .bg(color(Token::Bg))
        .border_l(px(1.0))
        .border_l_color(color(Token::Rule2))
        .shadow_2xl()
        .shadow_color(color(Token::Shell))
        .bind_motion(&entrance)
        .child(body(app, Compact::Drawer, desktop_head))
        .into_element()
}

/// Ask's session in `form` under its head, or without a connection the head with Close and why
/// nothing can be sent.
fn body(
    app: AppState,
    form: Compact,
    head_of: fn(AppState, Memo<Head>, Id) -> AnyElement,
) -> AnyElement {
    when(
        move || unconnected::unconnected(app),
        move || {
            untracked(|| {
                let head = computed(|| Head {
                    title: "Ask".into(),
                    word: None,
                    end: End::None,
                    refused: None,
                });
                view()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex_col()
                    .child(head_of(app, head, Id::from(assistant::QUICK)))
                    .child(unconnected::body(app))
                    .into_element()
            })
        },
    )
    .otherwise(move || untracked(|| keyed_session(app, form, head_of)))
    .into_element()
}

/// Whether Ask has a session to open: not without a connection.
fn connected(app: AppState) -> bool {
    untracked(|| app.data.get()) != DataState::Disconnected
}

/// `.dhead`: the title, the state word, End thread, Open session and the close control, 44 pt
/// high over a rule.
fn desktop_head(app: AppState, head: Memo<Head>, id: Id) -> AnyElement {
    let h = head.get_untracked();
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(8.0))
        .h(px(44.0))
        .pl(px(16.0))
        .pr(px(10.0))
        .border_b(px(1.0))
        .border_color(color(Token::Rule))
        .child(
            text(h.title.clone())
                .role(AccessibilityRole::Header)
                .number_of_lines(1u32)
                .flex_shrink_0()
                .font_family(FONT_TEXT)
                .text_size(px(13.5))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .children(h.word.as_ref().map(|word| state_word(app, word)))
        .child(view().flex_1())
        .children(end_button(app, &h.end, ButtonKind::Quiet))
        .children(connected(app).then(|| open_session(app, id, ButtonKind::Quiet)))
        .child(close_icon(app))
        .into_element()
}

/// The drawer's close control: the close icon alone, named "Close (Esc)" for screen readers.
fn close_icon(app: AppState) -> AnyElement {
    view()
        .test_id("ask-close")
        .role(AccessibilityRole::Button)
        .label("Close (Esc)")
        .on_press(move || act(app, Action::CloseAsk))
        .size(px(control_height(30.0)))
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .active(|s| s.bg(color(Token::Sel)))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::Focus))
                .outline_offset(px(2.0))
        })
        .child(icon(Icon::Close, 14.0, color(Token::Ink2)))
        .into_element()
}

/// Ask in a terminal: a pane of the frame around the content, its title row and the session
/// ruled apart by lines that join the frame. On a wide grid it takes half the content beside the
/// page, split from it by a line of its own, so nothing of the page is cut under it; on a narrow
/// grid it takes the whole frame while the shell sets the page aside.
pub(crate) fn pane(app: AppState) -> AnyElement {
    let body = view()
        .flex_1()
        .min_w(px(0.0))
        .child(body(app, Compact::Drawer, terminal_head));
    let pane = view()
        .role(AccessibilityRole::Region)
        .label("Ask")
        .flex_row();
    if term::wide() {
        pane.w(relative(0.5))
            .flex_shrink_0()
            .child(term::vrule())
            .child(body)
            .into_element()
    } else {
        pane.flex_1().min_w(px(0.0)).child(body).into_element()
    }
}

/// Cells between two parts of the terminal head.
const HEAD_GAP: usize = 2;

/// The terminal title row over a rule: the title in bold, the state, End thread, Open session,
/// then Esc Close as the key line reads. The title and Esc Close always show whole; the state,
/// then End thread, then Open session show only when they fit whole in the room the row
/// measures.
fn terminal_head(app: AppState, head: Memo<Head>, id: Id) -> AnyElement {
    let room = signal(None::<f32>);
    let row = view()
        .flex_row()
        .items_center()
        .h(term::rows(1.0))
        .px(term::cells(1.0))
        .overflow_hidden()
        .on_layout(move |frame: &Bounds<Points>| room.set(Some(frame.size.width.0)))
        .child(dynamic(move || {
            let h = head.get_untracked();
            let column = term::cells(1.0).0.max(1.0);
            // The row's own width less its padding of a Cell each side.
            let room = room.get().map_or(0, |width| {
                ((width / column + 0.01).floor() as usize).saturating_sub(2)
            });
            let id = id.clone();
            untracked(|| terminal_parts(app, &h, id, room))
        }));
    view().child(row).child(term::hrule()).into_element()
}

/// The head's parts that fit `room` Cells, in order.
fn terminal_parts(app: AppState, head: &Head, id: Id, room: usize) -> Vec<AnyElement> {
    const CLOSE: &str = "Close";
    let close_width = ESC.chars().count() + 2 + 1 + CLOSE.chars().count();
    let mut used = head.title.chars().count() + HEAD_GAP + close_width;
    let mut fits = |width: usize| {
        let fit = used + HEAD_GAP + width <= room;
        if fit {
            used += HEAD_GAP + width;
        }
        fit
    };
    let word = head
        .word
        .as_ref()
        .filter(|word| fits(word.words.chars().count() + 2));
    let end = match &head.end {
        End::Ready(_) => fits("End thread".len() + 2),
        End::Pending => fits("Ending…".chars().count() + 2),
        End::None => false,
    };
    let open = connected(app) && fits("Open session ↗".chars().count() + 2);
    let mut parts = vec![
        text(head.title.clone())
            .role(AccessibilityRole::Header)
            .number_of_lines(1u32)
            .flex_shrink_0()
            .font_weight(FontWeight::BOLD)
            .text_color(color(Token::Ink))
            .into_element(),
    ];
    if let Some(word) = word {
        parts.push(term::space(HEAD_GAP as f32));
        parts.push(state_word(app, word));
    }
    parts.push(
        view()
            .flex_1()
            .min_w(term::cells(HEAD_GAP as f32))
            .into_element(),
    );
    if end && let Some(control) = end_button(app, &head.end, ButtonKind::Ghost) {
        parts.push(control);
        parts.push(term::space(HEAD_GAP as f32));
    }
    if open {
        parts.push(open_session(app, id, ButtonKind::Ghost));
        parts.push(term::space(HEAD_GAP as f32));
    }
    parts.push(
        view()
            .test_id("ask-close")
            .role(AccessibilityRole::Button)
            .label("Close (Esc)")
            .on_press(move || act(app, Action::CloseAsk))
            .flex_row()
            .flex_shrink_0()
            .child(term::key_cap(ESC))
            .child(term::space(1.0))
            .child(
                text(CLOSE)
                    .number_of_lines(1u32)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color(Token::Ink)),
            )
            .into_element(),
    );
    parts
}

/// The phone sheet's panel (`.asksheet`): a grab handle, "Ask" with Open session and Close, the
/// newest reply (pressing it opens the session page), the dock and the composer; riding on the
/// software keyboard, which its field raises as it opens, and clear of the home indicator while
/// the keyboard is down. Held sideways, the little height goes to the field: the grab handle
/// gives way (the sheet still swipes down) and the sides keep clear of the notch.
pub(crate) fn phone_panel(app: AppState) -> AnyElement {
    view()
        .pb_keyboard()
        .max_h(move || px(window().height.0 * SHEET_SHARE))
        .bg(color(Token::Surface))
        .rounded_t(px(14.0))
        .border_t(px(1.0))
        .border_color(color(Token::Rule2))
        .shadow_2xl()
        .shadow_color(color(Token::Shell))
        .role(AccessibilityRole::Region)
        .label("Ask")
        .child(
            view()
                .pl_safe_area()
                .pr_safe_area()
                .style_when(short_window, |s| s.pt(px(6.0)))
                .pb(move || {
                    if keyboard_height() > px(0.0) {
                        px(0.0)
                    } else {
                        safe_area().bottom
                    }
                })
                .child(
                    view()
                        .self_center()
                        .w(px(36.0))
                        .h(px(5.0))
                        .mt(px(8.0))
                        .mb(px(4.0))
                        .rounded(px(3.0))
                        .bg(color(Token::Rule2))
                        .accessibility_elements_hidden(true)
                        .style_when(short_window, |s| s.hidden()),
                )
                .child(body(app, Compact::Sheet, phone_head)),
        )
        .into_element()
}

/// `.sh-h`: "Ask", then End thread, Open session and Close at the trailing edge.
fn phone_head(app: AppState, head: Memo<Head>, id: Id) -> AnyElement {
    let h = head.get_untracked();
    view()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .pl(px(18.0))
        .pr(px(6.0))
        .pb(px(2.0))
        .child(
            text(h.title.clone())
                .role(AccessibilityRole::Header)
                .flex_1()
                .min_w(px(0.0))
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(text_size(Form::Phone)))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .children(end_button(app, &h.end, ButtonKind::Quiet))
        .children(connected(app).then(|| open_session(app, id, ButtonKind::Quiet)))
        .child(button(
            app,
            "Close",
            ButtonKind::Quiet,
            "ask-close",
            move || act(app, Action::CloseAsk),
        ))
        .into_element()
}
