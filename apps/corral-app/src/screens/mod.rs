//! The screens over the model, one module per destination, and the routing between them: a
//! destination's screen, or on desktop and terminal the page above it. With no daemon at all
//! (`DataState::Disconnected`) every route shows one frame instead: the title and the
//! snapshot's line saying corrald cannot be reached.

pub mod agents;
pub mod assistant;
pub mod changes;
pub mod inbox;
pub mod now;
pub mod plans;
pub mod review;
pub mod settings;
pub mod timeline;
pub mod usage;

use nsh_core::prelude::*;

use crate::model::{DataState, Destination, Form, Page};
use crate::state::AppState;
use crate::ui::term;
use crate::ui::tokens::{Token, color, row_height, text_size};

/// What a phone page gives the phone shell, which draws the header, the scroll, the tab bar
/// or the page's own bar, and the floating Ask button.
pub struct PhonePage {
    pub title: String,
    /// The back control's label: "Inbox", "Now", "Agents", "Plans", "More", "Settings".
    pub back: &'static str,
    pub body: AnyElement,
    /// A control at the header's trailing edge (the "Ask about this" icon).
    pub trailing: Option<AnyElement>,
    /// A pinned bar that replaces the tab bar (the decide bar, a docked composer).
    pub bar: Option<AnyElement>,
    /// Whether the floating Ask button shows over this page (false where a composer is docked).
    pub ask: bool,
    /// True: the shell scrolls `body` and adds the Ask clearance. False: the page scrolls itself.
    pub scroll: bool,
}

/// The destination's screen; the disconnected frame while no daemon answers. Made again only
/// when the data state crosses that line, so a screen keeps its own state otherwise.
pub fn destination(app: AppState, destination: Destination) -> AnyElement {
    let data = computed(move || app.data.get());
    dynamic(move || {
        let data = data.get();
        untracked(move || {
            if unavailable(data, destination) {
                frame(app, destination)
            } else {
                screen(app, destination)
            }
        })
    })
}

/// The content container's page on desktop and terminal: the top page when there is one
/// (the focused question, a session, a thread), else the destination's screen. Tasks and
/// plans are selected in place there, so their destination's screen shows them. Made again
/// only when the route itself changes; the screens track their own state.
pub fn content(app: AppState) -> AnyElement {
    let route = computed(move || (app.destination(), app.page(), app.data.get()));
    dynamic(move || {
        let (destination, page, data) = route.get();
        untracked(move || {
            if unavailable(data, destination)
                || (data == DataState::Daemon
                    && matches!(
                        page,
                        Some(Page::Question | Page::Session(_) | Page::Thread(_))
                    ))
            {
                return frame(app, destination);
            }
            match page {
                Some(Page::Question) => inbox::question(app),
                Some(Page::Session(_)) => agents::session(app),
                Some(Page::Thread(_)) => assistant::thread(app),
                Some(Page::Task(_) | Page::Plan(_)) | None => screen(app, destination),
            }
        })
    })
}

fn unavailable(data: DataState, destination: Destination) -> bool {
    data == DataState::Disconnected
        || (data == DataState::Daemon
            && !matches!(destination, Destination::Now | Destination::Settings))
}

/// Each destination's own screen.
fn screen(app: AppState, destination: Destination) -> AnyElement {
    match destination {
        Destination::Inbox => inbox::screen(app),
        Destination::Now => now::screen(app),
        Destination::Changes => changes::screen(app),
        Destination::Plans => plans::screen(app),
        Destination::Agents => agents::screen(app),
        Destination::Review => review::screen(app),
        Destination::Timeline => timeline::screen(app),
        Destination::Usage => usage::screen(app),
        Destination::Settings => settings::screen(app),
    }
}

/// How the disconnected frame sits on each form: the desktop's page head, the phone's large
/// title, and whole Cells in a terminal.
struct Frame {
    pad_x: f32,
    head_height: Option<f32>,
    head_pad_y: f32,
    title_scale: f32,
    body_pad_y: f32,
    /// How the head is set off from the body: a border under it on the desktop; in a terminal,
    /// where a one-row box draws no border, a line of its own that joins the frame around the
    /// page (`├───┤`).
    rule: Rule,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rule {
    None,
    Border,
    Line,
}

impl Frame {
    fn of(form: Form) -> Frame {
        match form {
            Form::Desktop => Frame {
                pad_x: 18.0,
                head_height: Some(row_height(form) + 16.0),
                head_pad_y: 0.0,
                title_scale: 15.0 / 13.0,
                body_pad_y: 16.0,
                rule: Rule::Border,
            },
            Form::Phone => Frame {
                pad_x: 18.0,
                head_height: None,
                head_pad_y: 8.0,
                title_scale: 30.0 / 16.0,
                body_pad_y: 8.0,
                rule: Rule::None,
            },
            // A blank row between the line and the body: a box around one text keeps no
            // padding rows there.
            Form::Terminal => Frame {
                pad_x: 8.0,
                head_height: Some(row_height(form)),
                head_pad_y: 0.0,
                title_scale: 1.0,
                body_pad_y: 0.0,
                rule: Rule::Line,
            },
        }
    }
}

/// The disconnected frame: the destination's label as the title (test_id "page-title"), then
/// the snapshot's line (test_id "page-body").
fn frame(app: AppState, destination: Destination) -> AnyElement {
    let frame = Frame::of(app.form);
    let head = view()
        .flex_row()
        .flex_none()
        .items_center()
        .px(px(frame.pad_x))
        .py(px(frame.head_pad_y))
        .when_some(frame.head_height, |head, height| head.h(px(height)))
        .when(frame.rule == Rule::Border, |head| {
            head.border_b_1().border_color(color(Token::Rule))
        })
        .child(
            text(destination.label())
                .test_id("page-title")
                .role(AccessibilityRole::Header)
                .text_size(px(text_size(app.form) * frame.title_scale))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink))
                .number_of_lines(1),
        );
    let body = view().px(px(frame.pad_x)).pt(px(frame.body_pad_y)).child(
        text(move || app.snapshot().body_line(destination).into_owned())
            .test_id("page-body")
            .text_size(px(text_size(app.form)))
            .text_color(color(if app.form == Form::Terminal {
                Token::Ink
            } else {
                Token::Ink3
            })),
    );
    view()
        .size_full()
        .flex_col()
        .bg(color(Token::Bg))
        .child(head)
        .when(frame.rule == Rule::Line, |page| {
            page.child(term::hrule()).child(term::blank())
        })
        .child(body)
        .into_element()
}
