//! The screens over the model and the routing between them: a place's screen (`home`, `tasks`,
//! `sessions`, `activity`, `settings`), or the page above it, a task's (`task`) or a session's
//! (`agents::session`). Routes corrald does not serve show a frame that says why; without a
//! connection that frame offers Settings › Connection and the labelled sample-data preview, and
//! Settings itself stays available (its Connection page and this device's own preferences need
//! no connection).
//!
//! `now`, `plans`, `review`, `changes` and `projects` are the former destinations' screens. No
//! route shows Now's, Plans', Review's or the Project page any more; their parts are what the
//! task page and the places are built from, and each module goes once its parts have moved.

pub mod activity;
pub mod agents;
pub mod artifacts;
pub mod assistant;
pub mod changes;
pub mod home;
pub mod landing;
pub(crate) mod markdown;
pub mod now;
pub mod plans;
pub mod projects;
pub mod review;
pub mod sessions;
pub mod settings;
pub mod task;
pub mod tasks;
pub mod timeline;
pub mod usage;

use nsh_core::prelude::*;

use crate::model::settings::connect;
use crate::model::{
    Availability, DataState, Destination, Form, Live, Route, SettingsSection, WorkspaceMode,
};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
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
/// state otherwise. In a workspace with no projects, Home (where such a workspace lands) is the
/// assistant landing.
///
/// `root` is a phone tab's root: its unavailable frame keeps the head's search button, so a
/// disconnected phone still reaches the palette (the way into the labelled preview).
pub fn destination(app: AppState, destination: Destination, root: bool) -> AnyElement {
    let gate = computed(move || {
        (
            app.data.get(),
            unavailable(app, destination),
            lands(app, destination),
        )
    });
    dynamic(move || {
        let (_, unavailable, landing) = gate.get();
        untracked(move || {
            if landing {
                landing::screen(app)
            } else if unavailable {
                frame(app, destination, root)
            } else {
                screen(app, destination)
            }
        })
    })
}

/// The content container's page on desktop and terminal: the top page when there is one (a
/// task's or a session's), else the destination's screen. Made again only when the route
/// itself changes; the screens track their own state.
pub fn content(app: AppState) -> AnyElement {
    let route = computed(move || {
        let destination = app.destination();
        let top = app.nav.with(|nav| nav.pages.last().cloned());
        (
            destination,
            top,
            app.data.get(),
            unavailable(app, destination),
            lands(app, destination),
        )
    });
    dynamic(move || {
        let (destination, top, _, unavailable, landing) = route.get();
        untracked(move || {
            if landing && top.is_none() {
                return landing::screen(app);
            }
            if unavailable {
                return frame(app, destination, false);
            }
            match top {
                Some(Route::Live(Live::Session(id))) => agents::live_session(app, id),
                Some(Route::Live(Live::Task(id) | Live::Plan(id) | Live::Review(id))) => {
                    task::page(app, id)
                }
                None => screen(app, destination),
            }
        })
    })
}

/// Tracked: whether the destination shows the assistant landing: Home, in a served workspace
/// that has no projects (its board places are not offered at all).
fn lands(app: AppState, destination: Destination) -> bool {
    destination == Destination::Home
        && app.data.get() != DataState::Disconnected
        && app.workspace_mode() == WorkspaceMode::AssistantOnly
}

/// Tracked: whether the destination shows the unavailable frame instead of its screen.
/// Settings never does: its sections say for themselves what waits for corrald. The preview
/// feed serves what it illustrates through the same availability as corrald's.
fn unavailable(app: AppState, destination: Destination) -> bool {
    match app.data.get() {
        DataState::Disconnected => destination != Destination::Settings,
        DataState::Daemon | DataState::Fixture => app.available(destination) != Availability::Ready,
    }
}

/// Each destination's own screen.
fn screen(app: AppState, destination: Destination) -> AnyElement {
    match destination {
        Destination::Home => home::screen(app),
        Destination::Tasks => tasks::screen(app),
        Destination::Sessions => sessions::screen(app),
        Destination::Feed | Destination::Timeline | Destination::Usage | Destination::Artifacts => {
            activity::screen(app, destination)
        }
        Destination::Settings => settings::screen(app),
    }
}

/// A live record's page on a phone: the session's, or the task's for a task, a plan and a
/// review candidate.
pub fn live_phone(app: AppState, live: Live) -> PhonePage {
    match live {
        Live::Session(id) => agents::live_session_phone(app, id),
        Live::Task(id) | Live::Plan(id) | Live::Review(id) => task::phone(app, id),
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
/// why it is unavailable (test_id "page-body"): what corrald does not serve, or without a
/// connection why nothing shows, with the way to Settings › Connection (`btn-connect`) and the
/// explicit sample-data preview (`btn-preview`). A phone tab root (`root`) sets the title as the
/// large head with search, like its own screen.
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
    view()
        .size_full()
        .flex_col()
        .bg(color(Token::Bg))
        .child(head)
        .when(frame.rule == Rule::Line, |page| {
            page.child(term::hrule()).child(term::blank())
        })
        .child(
            view()
                .px(px(frame.pad_x))
                .pt(px(frame.body_pad_y))
                .child(unavailable_body(app, destination)),
        )
        .into_element()
}

/// Why the destination shows nothing (test_id "page-body") and, without a connection, the two
/// ways on. Shared with Settings sections that wait for corrald.
pub(crate) fn unavailable_body(app: AppState, destination: Destination) -> AnyElement {
    let form = app.form;
    let size = text_size(form);
    let ink = if form == Form::Terminal {
        Token::Ink
    } else {
        Token::Ink3
    };
    let web = crate::client::pairing_supported();
    dynamic(move || {
        if app.data.get() != DataState::Disconnected {
            let line = app
                .feed
                .with(|feed| crate::model::health::unavailable_line(feed, destination));
            return untracked(move || {
                text(line)
                    .test_id("page-body")
                    .text_size(px(size))
                    .text_color(color(ink))
                    .into_element()
            });
        }
        let view_model = app.feed.with(|feed| connect::unavailable(feed, web));
        untracked(move || {
            let connect = button_with(
                app,
                view_model.connect,
                ButtonKind::Primary,
                ButtonOptions::default(),
                "btn-connect",
                move || app.go_settings(SettingsSection::Connection),
            );
            let preview = button_with(
                app,
                view_model.preview,
                ButtonKind::Ghost,
                ButtonOptions::default(),
                "btn-preview",
                move || app.set_data(DataState::Fixture),
            );
            let actions = if form == Form::Terminal {
                view()
                    .flex_row()
                    .items_center()
                    .child(connect)
                    .child(term::space(2.0))
                    .child(preview)
            } else {
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(connect)
                    .child(preview)
            };
            let terminal = form == Form::Terminal;
            view()
                .when(!terminal, |body| body.max_w(px(560.0)).gap(px(14.0)))
                .child(
                    text(view_model.reason)
                        .test_id("page-body")
                        .text_size(px(size))
                        .when(!terminal, |t| t.line_height(px((size * 1.5).round())))
                        .text_color(color(ink)),
                )
                .when(terminal, |body| body.child(term::blank()))
                .child(actions)
                .into_element()
        })
    })
    .into_element()
}
