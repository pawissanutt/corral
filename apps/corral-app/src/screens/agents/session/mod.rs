//! The one session page every session opens in, of every role: the header with the way back,
//! the state word and the ⋯ actions, the tab strip with the tabs the model offers, the main view
//! (the transcript, the Usage view, the Agents, Changes, Board, Task or Proof tab, or the
//! read-only log of a session without a Pi host) with the sidebar's rail beside it when the page
//! is wide, the dock, the composer (or what takes its place: a decision card, the one
//! confirmation of a destructive option, an answer card, or the bar standing in for it while the
//! session takes no message) with the `/` command menu over it, and the status bar. Every fact
//! is the model's (`model::session::page`); every press runs the session reducer. Desktop and
//! terminal stack the parts top to bottom across the page; the phone pushes the page with the
//! transcript scrolling and the dock, the composer and the status fixed at the bottom. While
//! the page is on top the window's title says who the session is and whether it needs you.

mod agents_tab;
mod board;
mod cards;
mod changes;
mod command_menu;
mod decision;
mod dock;
mod events;
mod exit;
mod header;
mod pager;
mod plan;
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
use crate::model::session::{self, Body, Chat, Cx, Facts, Focus, KeyAction, SessionPage};
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
use super::transcript::{self, watch};

/// How far from its end a scroll still counts as at its end, in points.
const END_SLACK: f32 = 24.0;
/// The page's content width from which the sidebar's rail sits beside the main view: points on
/// desktop, Cells in a terminal.
const RAIL_FROM: f32 = 960.0;
const RAIL_FROM_CELLS: f32 = 105.0;
/// The window's title while no session page is on top.
const APP_TITLE: &str = "Corral";

/// The page of session `id` on desktop and in a terminal.
pub fn page(app: AppState, id: Id) -> AnyElement {
    let page = view_of(app, id.clone());
    follow(app, id.clone(), page);
    bind_keys(app, id.clone());
    window_title(page);
    let commands = command_menu::matches(page);
    let room = signal(None::<f32>);
    let rail = computed(move || {
        let wide = match app.form {
            Form::Terminal => term::page_columns(app) >= RAIL_FROM_CELLS,
            _ => room.get().is_some_and(|width| width >= RAIL_FROM),
        };
        wide && page.with(|p| p.sidebar.as_ref().is_some_and(|sidebar| !sidebar.hidden))
    });
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
        .child(header::head(app, page))
        .child(banner(app, page))
        .child(tabs::strip(app, page))
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .flex_row()
                .child(main(app, page))
                .child(dynamic(move || {
                    rail.get().then(|| untracked(|| sidebar::rail(app, page)))
                })),
        )
        .child(dock::dock(app, page, rail))
        .child(command_menu::menu(app, page, commands))
        .child(sidebar::sheets(app, id.clone()))
        .child(bottom(app, page, id, commands, false))
        .child(status::bar(app, page))
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
/// holds the actions; the state line and the tabs stay over the main view, which scrolls itself;
/// the dock, the composer and the status are the page's bar.
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
        .child(tabs::strip(app, page))
        .child(main(app, page))
        .child(sidebar::sheets(app, id.clone()))
        .into_element();
    let bar = view()
        .bg(color(Token::Bg))
        .border_t_1()
        .border_color(color(Token::Rule))
        .child(dock::dock(app, page, computed(|| false)))
        .child(command_menu::menu(app, page, commands))
        .child(bottom(app, page, id, commands, false))
        .child(status::bar(app, page))
        .into_element();
    PhonePage {
        title,
        body,
        trailing: Some(header::actions(app, page)),
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
    let tail = view()
        .flex_none()
        .child(dock::dock(app, page, computed(|| false)))
        .child(command_menu::menu(app, page, commands))
        .child(bottom(app, page, id.clone(), commands, true))
        .when(form != Compact::Composer, |tail| {
            tail.child(status::bar(app, page))
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
        Compact::Composer => tail.into_element(),
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
        app.feed
            .with(|feed| app.session.with(|state| ask_page(feed, state, &id, env)))
    })
}

/// Session `id`'s page on its chat: its own UI state read as if Chat were shown, the stored tab
/// and view untouched.
fn ask_page(
    feed: &crate::model::daemon::Feed,
    state: &session::State,
    id: &Id,
    env: crate::model::effect::Env,
) -> SessionPage {
    let ui = state.ui(id.as_str());
    if ui.tab == session::Tab::Chat && ui.sub.is_none() {
        return session::page(feed, state, id, env);
    }
    let mut chat = ui.clone();
    chat.tab = session::Tab::Chat;
    chat.sub = None;
    chat.focus = Focus::Composer;
    let mut shown = session::State::default();
    shown.sessions.insert(id.clone(), chat);
    session::page(feed, &shown, id, env)
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

/// Which main view shows: drawn again only when that changes, each following its own part.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shown {
    Chat,
    Usage,
    Agents,
    Changes,
    Board,
    Task,
    Proof,
    Log,
}

fn main(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || {
        page.with(|p| match &p.body {
            Body::Chat(_) => Shown::Chat,
            Body::Usage(_) => Shown::Usage,
            Body::Agents(_) => Shown::Agents,
            Body::Changes(_) => Shown::Changes,
            Body::Board(_) => Shown::Board,
            Body::Task(_) => Shown::Task,
            Body::Proof(_) => Shown::Proof,
            Body::Log(_) => Shown::Log,
        })
    });
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex_col()
        .child(dynamic(move || {
            let shown = shown.get();
            untracked(|| match shown {
                Shown::Chat => chat(app, page),
                Shown::Usage => usage::tab(app, page),
                Shown::Agents => agents_tab::tab(app, page),
                Shown::Changes => changes::tab(app, page),
                Shown::Board => board::tab(app, page),
                Shown::Task => task::tab(app, page),
                Shown::Proof => proof::tab(app, page),
                Shown::Log => pager::pager(app, page),
            })
        }))
        .into_element()
}

/// Chat: the transcript in its own scroll, which keeps to the newest rows as they arrive unless
/// the owner scrolled up (then a control counts what arrived and returns to the end); before
/// the first row, who receives the message.
fn chat(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let chat = computed(move || {
        page.with(|p| match &p.body {
            Body::Chat(chat) => Some(chat.clone()),
            _ => None,
        })
    });
    let shown = dynamic(move || {
        chat.with(|chat| {
            chat.as_ref()
                .map(|chat| untracked(|| chat_content(app, chat)))
        })
    });
    let content = if app.form == Form::Terminal {
        view().px(term::cells(1.0)).child(shown).into_element()
    } else {
        shown
    };
    transcript::follow(
        app,
        scroll()
            .test_id("session-transcript")
            .flex_1()
            .min_h(px(0.0))
            .child(content),
        move || chat.with(|c| c.as_ref().map(|c| c.transcript.following())),
        move || page.with(|p| p.jump),
    )
}

fn chat_content(app: AppState, chat: &Chat) -> AnyElement {
    let Some(empty) = &chat.empty else {
        return transcript::transcript(app, &chat.transcript);
    };
    let form = app.form;
    let sentence = text(empty.sentence.clone()).text_color(color(Token::Ink2));
    let keys = view()
        .flex_row()
        .flex_wrap()
        .gap(px(if form == Form::Terminal { 16.0 } else { 12.0 }))
        .children(
            empty
                .keys
                .iter()
                .map(|&(keys, name)| term_or_hint(app, keys, name)),
        );
    let block = view().test_id("session-empty").gap(px(8.0));
    match form {
        Form::Terminal => block
            .child(term::blank())
            .child(sentence)
            .child(keys)
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .w_full()
            .items_center()
            .pt(px(32.0))
            .px(px(24.0))
            .child(
                block
                    .max_w(px(560.0))
                    .child(
                        sentence
                            .font_family(FONT_TEXT)
                            .text_size(px(14.0))
                            .line_height(px(21.0)),
                    )
                    .child(keys),
            )
            .into_element(),
    }
}

/// A key and what it does: the terminal's legend, the desktop's cap.
fn term_or_hint(app: AppState, keys: &'static str, name: &'static str) -> AnyElement {
    if app.form == Form::Terminal {
        return term::legend(keys, name);
    }
    view()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .child(crate::ui::atoms::key_hint(keys))
        .child(
            text(name)
                .font_family(FONT_TEXT)
                .text_size(px(11.5))
                .text_color(color(Token::Ink3)),
        )
        .into_element()
}

/// What the composer's place shows: the one confirmation a destructive option asks for, a
/// decision card, an open answer card, the composer, or the bar standing in for it while the
/// session takes no message (not running, ended, read-only), with Resume or Relaunch where the
/// host can be started again. `commands` are the `/` rows the draft matches.
fn bottom(
    app: AppState,
    page: Memo<SessionPage>,
    id: Id,
    commands: Memo<Option<session::SlashMenu>>,
    ask: bool,
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
    let focused = computed(move || page.with(|p| p.focus == Focus::Composer));
    let command = command_menu::chosen(commands);
    let bar = computed(move || page.with(|p| p.bar.clone()));
    dynamic(move || {
        let place = place.get();
        let id = id.clone();
        untracked(|| match place {
            Place::Confirm => decision::confirm(app, page),
            Place::Decision => decision::decision(app, page),
            Place::Answer => questions::answer(app, page),
            Place::Composer if ask => ask_composer(app, id, composer, command, focused),
            Place::Composer => session_composer(app, id, composer, command, focused),
            Place::Bar => dock::host_bar(app, bar),
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
