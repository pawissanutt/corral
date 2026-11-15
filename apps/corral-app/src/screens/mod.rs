//! The screens over the model, one module per destination, and the routing between them: a
//! destination's screen, or on desktop and terminal the page above it. Routes corrald does not
//! serve show a frame that says why; browser Settings keeps Connection available for pairing.

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

use crate::model::{Availability, DataState, Destination, Form, Live, Page, Route};
use crate::state::AppState;
use crate::ui::kit::page;
use crate::ui::term;
use crate::ui::tokens::{Token, color, row_height, text_size};

/// What a phone page gives the phone shell, which draws the header (its back control names the
/// page under it in the shell's stack), the scroll, the tab bar or the page's own bar, and the
/// floating Ask button.
pub struct PhonePage {
    pub title: String,
    pub body: AnyElement,
    /// A control at the header's trailing edge (the "Ask about this" icon).
    pub trailing: Option<AnyElement>,
    /// A pinned bar that replaces the tab bar while it shows (the decide bar, a docked composer).
    pub bar: Option<PhoneBar>,
    /// Whether the floating Ask button shows over this page: false where a composer is docked
    /// or the header already asks about the page, so a page offers one Ask.
    pub ask: bool,
    /// True: the shell scrolls `body` and adds the Ask clearance. False: the page scrolls itself.
    pub scroll: bool,
}

/// A phone page's own bar and whether it shows now. A page whose bar comes and goes with its
/// state (a plan that awaits acceptance once its detail arrives) keeps the tab bar while it
/// has none.
pub struct PhoneBar {
    pub shown: Memo<bool>,
    pub view: AnyElement,
}

impl PhoneBar {
    /// A bar that shows for as long as its page does.
    pub fn always(view: AnyElement) -> PhoneBar {
        PhoneBar {
            shown: computed(|| true),
            view,
        }
    }
}

/// The destination's screen; the unavailable frame while no daemon serves it. Made again only
/// when the data state or the destination's availability changes, so a screen keeps its own
/// state otherwise.
///
/// `root` is a phone tab's root: its unavailable frame keeps the head's search button, so a
/// disconnected phone still reaches the palette (the way into the fixture preview).
pub fn destination(app: AppState, destination: Destination, root: bool) -> AnyElement {
    let gate = computed(move || (app.data.get(), unavailable(app, destination)));
    dynamic(move || {
        let (_, unavailable) = gate.get();
        untracked(move || {
            if unavailable {
                frame(app, destination, root)
            } else {
                screen(app, destination)
            }
        })
    })
}

/// The content container's page on desktop and terminal: the top page when there is one
/// (the focused question, a session, a thread), else the destination's screen. Tasks, plans
/// and reviews are selected in place there, so their destination's screen shows them. Made
/// again only when the route itself changes; the screens track their own state.
pub fn content(app: AppState) -> AnyElement {
    let route = computed(move || {
        let destination = app.destination();
        let top = app.nav.with(|nav| nav.pages.last().cloned());
        (
            destination,
            top,
            app.data.get(),
            unavailable(app, destination),
        )
    });
    dynamic(move || {
        let (destination, top, data, unavailable) = route.get();
        untracked(move || {
            if unavailable {
                return frame(app, destination, false);
            }
            match (data, top) {
                (DataState::Daemon, Some(Route::Live(Live::Question(key)))) => {
                    inbox::live_question(app, key)
                }
                (DataState::Daemon, Some(Route::Live(Live::Session(id)))) => {
                    agents::live_session(app, id)
                }
                (DataState::Daemon, Some(Route::Live(Live::Thread(id)))) => {
                    assistant::live_thread(app, id)
                }
                (DataState::Fixture, Some(Route::Page(Page::Question))) => inbox::question(app),
                (DataState::Fixture, Some(Route::Page(Page::Session(_)))) => agents::session(app),
                (DataState::Fixture, Some(Route::Page(Page::Thread(_)))) => assistant::thread(app),
                // Pages of the other source never render: its destination shows instead.
                _ => screen(app, destination),
            }
        })
    })
}

/// Tracked: whether the destination shows the unavailable frame instead of its screen.
fn unavailable(app: AppState, destination: Destination) -> bool {
    match app.data.get() {
        DataState::Disconnected => {
            !(destination == Destination::Settings && crate::client::pairing_supported())
        }
        DataState::Daemon => app.available(destination) != Availability::Ready,
        DataState::Fixture => false,
    }
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

/// The unavailable frame: the destination's label as the title (test_id "page-title"), then
/// why it is unavailable (test_id "page-body"): no connection, or what corrald does not serve.
/// A phone tab root (`root`) sets the title as the large head with search, like its own screen.
fn frame(app: AppState, destination: Destination, root: bool) -> AnyElement {
    let frame = Frame::of(app.form);
    let head = if root && app.form == Form::Phone {
        page::head(app, destination.label(), None, vec![page::search(app)])
    } else {
        view()
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
            )
            .into_element()
    };
    let body = view().px(px(frame.pad_x)).pt(px(frame.body_pad_y)).child(
        text(move || {
            if app.daemon() {
                app.feed
                    .with(|feed| crate::model::health::unavailable_line(feed, destination))
            } else {
                app.snapshot().body_line(destination).into_owned()
            }
        })
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
