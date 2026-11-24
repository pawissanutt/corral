//! The one session page every session opens in, of every role: the one-row header with the way
//! back, the state word, the Agents switcher, Files and Changes while the page is wide, Details,
//! More and the ⋯ host actions (with the page's keys), the main view (the transcript, or a
//! Details section opened in its place: Usage, Agents, Changes, Files, Board, Task or Proof, or
//! the read-only log of a session without a Pi host) with the Details panel beside it when the
//! page is wide, the dock, the composer (or what takes its place: a decision card, the one
//! confirmation of a destructive option, an answer card, or the bar standing in for it while the
//! session takes no message) with the `/` command menu over it, and the host's alerts only while
//! it has any. No status or key line stays under the page: the cost, context and model are
//! Details' and the composer's, the keys the ⋯ menu's. Every fact is the model's
//! (`model::session::page`); every press runs the session reducer. Desktop and terminal stack
//! the parts top to bottom across the page; the phone pushes the page with the transcript
//! scrolling and the dock and the composer fixed at the bottom. While the page is on top the
//! window's title says who the session is and whether it needs you.

mod agents_tab;
mod board;
mod body;
mod cards;
mod changes;
mod command_menu;
mod decision;
mod dock;
mod events;
mod exit;
mod files;
mod header;
mod pager;
mod plan;
mod plan_review;
mod proof;
mod questions;
mod sidebar;
mod status;
mod tabs;
mod task;
mod usage;

pub(in crate::screens::agents) use header::popup;

use corral_protocol::detail::DetailRef;
use nsh_core::desktop::WindowAttributes;
use nsh_core::element::desktop::current_window;
use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::session::{self, Body, Cx, Facts, Focus, KeyAction, SessionPage};
use crate::model::text::span;
use crate::model::work::{session_task, task_ref};
use crate::model::{Form, Id};
use crate::screens::{PhoneBar, PhonePage};
use crate::state::AppState;
use crate::ui::composer::{ask_composer, session_composer};
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::notice::notice;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::run;
use super::transcript::watch;
use body::{chat, conversation, main, view_bar};

/// How far from its end a scroll still counts as at its end, in points.
const END_SLACK: f32 = 24.0;
/// The page's width from which Details sits beside the conversation (and opens by default):
/// points on desktop, Cells in a terminal. Narrower, it is closed until opened and then takes
/// the conversation's place.
const BESIDE: f32 = 900.0;
const BESIDE_CELLS: f32 = 100.0;
/// The window's title while no session page is on top.
const APP_TITLE: &str = "Corral";

/// Where Details is drawn on desktop and in a terminal.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Closed,
    /// Beside the conversation.
    Beside,
    /// In the conversation's place: the page is too narrow for both.
    Instead,
}

/// The page of session `id` on desktop and in a terminal: the head, then the conversation (or a
/// part's full view) over the dock and the composer, with Details beside it on a wide page.
pub fn page(app: AppState, id: Id) -> AnyElement {
    let page = view_of(app, id.clone());
    follow(app, id.clone(), page);
    bind_keys(app, id.clone());
    window_title(page);
    let commands = command_menu::matches(page);
    let room = signal(None::<f32>);
    let wide = computed(move || match app.form {
        Form::Terminal => term::page_columns(app) >= BESIDE_CELLS,
        _ => room.get().is_none_or(|width| width >= BESIDE),
    });
    // The model opens Details by default only where it fits beside the conversation.
    effect(move || {
        let wide = wide.get();
        if untracked(|| app.session.with(|state| state.panel.wide != wide)) {
            untracked(|| {
                run(
                    app,
                    session::Action::Sidebar(session::SidebarAction::Wide(wide)),
                )
            });
        }
    });
    let panel = computed(move || {
        let shown = page.with(|p| p.details.as_ref().is_some_and(|details| details.shown));
        match (shown, wide.get()) {
            (false, _) => Panel::Closed,
            (true, true) => Panel::Beside,
            (true, false) => Panel::Instead,
        }
    });
    // Details widens with the page; BESIDE reads the whole page, before Details takes its share.
    let panel_width = computed(move || sidebar::panel_width(room.get()));
    let talk = view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex_col()
        .child(view_bar(app, page))
        .child(main(app, page))
        .child(dock::dock(app, page, true))
        .child(conversation(
            app,
            true,
            command_menu::menu(app, page, commands, false),
        ))
        .child(bottom(app, page, id.clone(), commands, false, true, true))
        .child(alerts(app, page))
        .style_when(move || panel.get() == Panel::Instead, |s| s.hidden());
    view()
        .size_full()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex_col()
        .when(app.form != Form::Terminal, |frame| {
            frame.bg(color(Token::Bg))
        })
        .on_layout(move |frame: &Bounds<Points>| {
            let width = Some(frame.size.width.0);
            if room.get_untracked() != width {
                room.set(width);
            }
        })
        .child(header::head(app, page, wide))
        .child(banner(app, page))
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .flex_row()
                .child(talk)
                .child(dynamic(move || match panel.get() {
                    Panel::Closed => None,
                    Panel::Beside => {
                        Some(untracked(|| sidebar::panel(app, page, Some(panel_width))))
                    }
                    Panel::Instead => Some(untracked(|| sidebar::panel(app, page, None))),
                })),
        )
        .child(sidebar::sheets(app, id))
        .into_element()
}

/// While the page is on top, the window's title is the session's (`▲ needs you · <target>`,
/// `● working · <target>`); the app's again once it leaves.
fn window_title(page: Memo<SessionPage>) {
    let window = current_window();
    let title = computed(move || page.with(|p| p.header.title.clone()));
    {
        let window = window.clone();
        effect(move || {
            let mut attributes = WindowAttributes::default();
            attributes.title = Some(title.get());
            window.set(attributes);
        });
    }
    cleanup(move || {
        let mut attributes = WindowAttributes::default();
        attributes.title = Some(APP_TITLE.to_owned());
        window.set(attributes);
    });
}

/// The page of session `id` on the phone: the shell's head names the session, its trailing ⋯
/// holds the actions; the state line (with Details, which opens as a sheet) stays over the main
/// view, which scrolls itself; the dock and the composer are the page's bar. The phone has no
/// queue and no status line.
pub fn phone(app: AppState, id: Id) -> PhonePage {
    let page = view_of(app, id.clone());
    follow(app, id.clone(), page);
    let title = untracked(|| page.with(|p| p.header.target.clone()));
    let commands = command_menu::matches(page);
    let body = view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(header::phone_line(app, page))
        .child(banner(app, page))
        .child(view_bar(app, page))
        .child(main(app, page))
        .child(sidebar::sheet(app, page))
        .child(sidebar::sheets(app, id.clone()))
        .into_element();
    let bar = view()
        .bg(color(Token::Bg))
        .border_t_1()
        .border_color(color(Token::Rule))
        .child(dock::dock(app, page, false))
        .child(command_menu::menu(app, page, commands, false))
        .child(bottom(app, page, id, commands, false, true, false))
        .into_element();
    PhonePage {
        title,
        body,
        trailing: Some(header::phone_trailing(app, page)),
        bar: Some(PhoneBar::always(bar)),
        ask: false,
        scroll: false,
    }
}

/// What Ask shows of a session: in the drawer and the terminal pane the transcript, the dock,
/// the composer and the status row; on the phone sheet the newest reply, the dock and the
/// composer; in the hotkey window the composer alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compact {
    Drawer,
    Sheet,
    Composer,
    /// A Home card or the Sessions inspector: the dock and the session's own composer (or the
    /// answer card in its place), without Ask's chips and draft and without the status row.
    Card,
}

/// Session `id` in Ask (screens §11): the same model, transcript, dock and composer as its page,
/// with no head, tabs or sidebar, which Ask's own head stands in for. `page` is
/// `ask_view(app, id)`, which Ask's head reads too. `quick` is read again as Ask opens on
/// it: the chat it named may have ended since.
pub fn compact(app: AppState, id: Id, page: Memo<SessionPage>, form: Compact) -> AnyElement {
    follow(app, id.clone(), page);
    if id.as_str() == crate::model::assistant::QUICK {
        let quick = DetailRef::Session { id: id.to_string() };
        untracked(|| app.scope().refresh(quick));
    }
    let commands = command_menu::matches(page);
    // A card answers in place on Home or in the inspector: its composer takes the keys only
    // when pressed, so the list's keys keep walking the cards.
    let card = form == Compact::Card;
    let tail = view()
        .flex_none()
        .child(dock::dock(app, page, false))
        .child(command_menu::menu(app, page, commands, !card))
        .child(bottom(app, page, id.clone(), commands, !card, !card, false))
        .when(!matches!(form, Compact::Composer | Compact::Card), |tail| {
            tail.child(status::bar(app, page, false))
        });
    match form {
        Compact::Drawer => view()
            .flex_1()
            .min_h(px(0.0))
            .flex_col()
            .child(banner(app, page))
            .child(
                view()
                    .test_id("ask-transcript")
                    .flex_1()
                    .min_h(px(0.0))
                    .flex_col()
                    .child(chat(app, page)),
            )
            .child(tail)
            .into_element(),
        Compact::Sheet => view()
            .flex_col()
            .child(latest(app, id, page))
            .child(tail)
            .into_element(),
        Compact::Composer | Compact::Card => tail.into_element(),
    }
}

/// The phone sheet's newest reply, its first three lines; pressing it opens the session page,
/// where the whole transcript is. Nothing before the agent has said anything.
fn latest(app: AppState, id: Id, page: Memo<SessionPage>) -> AnyElement {
    use crate::model::session::transcript::{MessageItem, MessageKind};
    let words = computed(move || {
        page.with(|p| {
            let Body::Chat(chat) = &p.body else {
                return None;
            };
            let reply = chat
                .transcript
                .messages
                .iter()
                .chain(chat.transcript.live.iter())
                .rev()
                .find(|message| message.kind == MessageKind::Agent)?;
            reply.items.iter().find_map(|item| match item {
                MessageItem::Prose(prose) => Some(
                    prose
                        .markdown
                        .lines()
                        .filter(|line| !line.trim().is_empty())
                        .take(3)
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                _ => None,
            })
        })
    });
    dynamic(move || {
        words.get().map(|words| {
            let id = id.clone();
            untracked(|| {
                view()
                    .test_id("ask-latest")
                    .role(AccessibilityRole::Button)
                    .label("Open the conversation")
                    .on_press(move || {
                        run(
                            app,
                            session::Action::OpenLive(crate::model::Live::Session(id.clone())),
                        )
                    })
                    .cursor_pointer()
                    .px(px(18.0))
                    .py(px(8.0))
                    .border_b(px(1.0))
                    .border_color(color(Token::Rule))
                    .child(
                        text(words)
                            .number_of_lines(3u32)
                            .font_family(FONT_TEXT)
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .text_color(color(Token::Ink2)),
                    )
                    .into_element()
            })
        })
    })
    .into_element()
}

/// The session's page as Ask shows it (tracked): always the chat, whatever tab or view its own
/// page was left on; that page keeps its tab.
pub fn ask_view(app: AppState, id: Id) -> Memo<SessionPage> {
    computed(move || {
        let env = app.env();
        app.feed.with(|feed| {
            app.session
                .with(|state| session::compact(feed, state, &id, env))
        })
    })
}

#[cfg(test)]
#[path = "compact_tests.rs"]
mod compact_tests;

/// The page as the model builds it now (tracked).
fn view_of(app: AppState, id: Id) -> Memo<SessionPage> {
    computed(move || {
        let env = app.env();
        app.feed.with(|feed| {
            app.session
                .with(|state| session::page(feed, state, &id, env))
        })
    })
}

/// What the open page keeps going, through the source it mounted on: the served page read (or,
/// for a session without a Pi host, its log capture), a closed run's task record (where an ended
/// headless run is read from), the live topic where the session streams, and the complete words
/// behind every preview on the shown page.
fn follow(app: AppState, id: Id, page: Memo<SessionPage>) {
    let scoped = app.scope();
    if let Some(task) = session_task(id.as_str()).map(task_ref)
        && untracked(|| app.feed.with(|feed| !feed.agents.contains_key(id.as_str())))
    {
        untracked(|| scoped.load(task));
    }
    let log = computed(move || page.with(|p| matches!(p.body, Body::Log(_))));
    {
        let id = id.clone();
        effect(move || {
            let target = if log.get() {
                DetailRef::Terminal { id: id.to_string() }
            } else {
                DetailRef::Session { id: id.to_string() }
            };
            untracked(|| scoped.load(target));
        });
    }
    if untracked(|| page.with(|p| p.watch.is_some())) {
        watch(app, id);
    }
    let wanted = computed(move || page.with(|p| p.fetches.clone()));
    effect(move || {
        let actions = wanted.get();
        untracked(|| {
            for action in actions {
                run(scoped, action);
            }
        });
    });
}

/// The session's chords while its page is open (desktop and terminal): the tabs by their
/// terminal key and position, thinking, expand all, the jump to the latest rows, effort and model
/// steps and the receiver, in the composer's field too. A view's bare keys are the app's key
/// router's (`state/keys.rs`), taken only while no field is edited; Ctrl+Shift+M is bound there
/// app-wide. Escape is the app's (it closes a picker or the `/` menu, returns to Chat, then
/// stops the turn, before it leaves the page) and Enter the composer's; nothing here acts while
/// a sheet or overlay is up. End also reaches the field, where it moves the caret.
fn bind_keys(app: AppState, id: Id) {
    let web = cfg!(target_arch = "wasm32");
    for binding in session::bound(app.form, web) {
        if !binding.typing()
            || matches!(
                binding.action,
                KeyAction::Escape | KeyAction::Send(_) | KeyAction::Model
            )
        {
            continue;
        }
        let options = KeyOptions {
            when: When::Always,
            pass_through: binding.pass_through(),
            ..KeyOptions::default()
        };
        let id = id.clone();
        let key = binding.key;
        on_key_with(key, options, move |_| {
            if app.nav.with_untracked(|nav| nav.newest().is_some()) {
                return;
            }
            let env = untracked(|| app.env());
            let action = app.feed.with_untracked(|feed| {
                app.session.with_untracked(|state| {
                    let facts = Facts::of(feed, &id);
                    let cx = Cx::new(feed, state.ui(id.as_str()), &id, env, &facts);
                    session::press(key, &cx, web)
                })
            });
            if let Some(action) = action {
                run(app, action);
            }
        });
    }
}

/// `! corrald unreachable · last update 14:02` over the page while the link is down.
fn banner(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || page.with(|p| p.banner.clone()));
    dynamic(move || {
        shown.get().map(|(hue, words)| {
            untracked(|| {
                let line = notice(app, hue, &vec![span(words)], None);
                match app.form {
                    Form::Terminal => line,
                    Form::Phone => view().px(px(16.0)).pt(px(8.0)).child(line).into_element(),
                    Form::Desktop => view().px(px(18.0)).pt(px(10.0)).child(line).into_element(),
                }
            })
        })
    })
}

/// The host's standing alerts and last error under the composer, each in its tone and never
/// dropped, while there are any; nothing otherwise.
fn alerts(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || {
        page.with(|p| {
            p.status
                .alerts
                .iter()
                .map(|alert| (alert.hue, alert.text.clone()))
                .collect::<Vec<_>>()
        })
    });
    dynamic(move || {
        let alerts = shown.get();
        (!alerts.is_empty()).then(|| {
            untracked(|| {
                let lines = view()
                    .test_id("session-alerts")
                    .flex_none()
                    .gap(px(if app.form == Form::Terminal { 0.0 } else { 4.0 }))
                    .children(
                        alerts
                            .into_iter()
                            .map(|(hue, words)| notice(app, hue, &vec![span(words)], None)),
                    );
                match app.form {
                    Form::Terminal => lines.into_element(),
                    _ => conversation(app, true, lines.px(px(18.0)).pb(px(6.0)).into_element()),
                }
            })
        })
    })
    .into_element()
}

/// What the composer's place shows: the one confirmation a destructive option asks for, a
/// decision card, an open answer card, the composer, or the bar standing in for it while the
/// session takes no message (not running, ended, read-only), with Resume or Relaunch where the
/// host can be started again. `commands` are the `/` rows the draft matches; `ask` is Ask's
/// composer; `takes_focus` lets the composer take the keyboard while the page gives it the keys;
/// `measured` keeps it under the transcript's measure (the page).
fn bottom(
    app: AppState,
    page: Memo<SessionPage>,
    id: Id,
    commands: Memo<Option<session::SlashMenu>>,
    ask: bool,
    takes_focus: bool,
    measured: bool,
) -> AnyElement {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Place {
        Confirm,
        Decision,
        Answer,
        Composer,
        Bar,
    }
    let place = computed(move || {
        page.with(|p| {
            if p.dock.confirm.is_some() {
                Place::Confirm
            } else if p.dock.decision.is_some() {
                Place::Decision
            } else if p.dock.answer.is_some() {
                Place::Answer
            } else if p.composer.is_some() {
                Place::Composer
            } else {
                Place::Bar
            }
        })
    });
    let composer = computed(move || page.with(|p| p.composer.clone()));
    let focused = computed(move || takes_focus && page.with(|p| p.focus == Focus::Composer));
    let command = command_menu::chosen(commands);
    let bar = computed(move || page.with(|p| p.bar.clone()));
    dynamic(move || {
        let place = place.get();
        let id = id.clone();
        untracked(|| {
            let shown = match place {
                Place::Confirm => decision::confirm(app, page),
                Place::Decision => decision::decision(app, page),
                Place::Answer => questions::answer(app, page),
                Place::Composer if ask => ask_composer(app, id, composer, command, focused),
                Place::Composer => session_composer(app, id, composer, command, focused),
                // The bar's band runs edge to edge; it keeps its words to the measure itself.
                Place::Bar => return dock::host_bar(app, bar, measured),
            };
            conversation(app, measured, shown)
        })
    })
    .into_element()
}

/// A state's mark: the running mark for a spinner, else its glyph in its hue; none without one.
pub(super) fn state_mark(
    app: AppState,
    spinner: bool,
    glyph: Option<&'static str>,
    hue: Hue,
) -> Option<AnyElement> {
    if spinner {
        return Some(mark(app, Mark::Active, hue));
    }
    let glyph = text(glyph?)
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_color(hue_ink(hue));
    Some(if app.form == Form::Terminal {
        glyph.into_element()
    } else {
        glyph
            .font_family(FONT_TEXT)
            .text_size(px(12.0))
            .font_weight(FontWeight::BOLD)
            .into_element()
    })
}

/// Words in a state's hue: ink-2 for a neutral one (ink in a terminal).
pub(super) fn hue_words(form: Form, hue: Hue) -> AdaptiveColor {
    match (hue, form) {
        (Hue::Neutral, Form::Terminal) => color(Token::Ink),
        (Hue::Neutral, _) => color(Token::Ink2),
        _ => hue_ink(hue),
    }
}

/// A scroll that opens at its end and keeps to it as `track`'s rows change, unless the owner has
/// scrolled away from the end: then what they read stays put until they scroll back down.
pub(super) fn follow_end(surface: Scroll, track: impl Fn() + 'static) -> Scroll {
    let scroller = element_ref();
    let at_end = stored(true);
    effect(move || {
        track();
        if at_end.get() {
            after_answer(move || scroller.scroll_to_end(false));
        }
    });
    surface
        .element_ref(&scroller)
        .on_scroll(move |event: &ScrollEvent| {
            let below = event.content_size.height.0 - event.offset.y.0 - event.size.height.0;
            at_end.replace(below <= END_SLACK);
        })
}
