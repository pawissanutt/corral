//! The one session page every session opens in, of every role: the header with the way back,
//! the state word and the ⋯ actions, the tab strip with the tabs the model offers, the main view
//! (the transcript, the Agents tab, or the read-only log of a session without a Pi host), the
//! dock, the composer (or the bar standing in its place while the session takes no message),
//! and the status bar. Every fact is the model's (`model::session::page`); every press runs the
//! session reducer. Desktop and terminal stack the parts top to bottom across the page; the
//! phone pushes the page with the transcript scrolling and the dock, the composer and the
//! status fixed at the bottom.

mod agents_tab;
mod dock;
mod header;
mod pager;
mod status;
mod tabs;

pub(in crate::screens::agents) use header::popup;

use corral_protocol::detail::DetailRef;
use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::session::{self, Body, Chat, Cx, Facts, KeyAction, SessionPage};
use crate::model::text::span;
use crate::model::work::{session_task, task_ref};
use crate::model::{Form, Id};
use crate::screens::{PhoneBar, PhonePage};
use crate::state::AppState;
use crate::ui::composer::session_composer;
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::notice::notice;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::run;
use super::transcript::{self, watch};

/// How far from its end a scroll still counts as at its end, in points.
const END_SLACK: f32 = 24.0;

/// The page of session `id` on desktop and in a terminal.
pub fn page(app: AppState, id: Id) -> AnyElement {
    let page = view_of(app, id.clone());
    follow(app, id.clone(), page);
    bind_keys(app, id.clone());
    view()
        .size_full()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex_col()
        .when(app.form != Form::Terminal, |frame| {
            frame.bg(color(Token::Bg))
        })
        .child(header::head(app, page))
        .child(banner(app, page))
        .child(tabs::strip(app, page))
        .child(main(app, page))
        .child(dock::dock(app, page))
        .child(bottom(app, page, id))
        .child(status::bar(app, page))
        .into_element()
}

/// The page of session `id` on the phone: the shell's head names the session, its trailing ⋯
/// holds the actions; the state line and the tabs stay over the main view, which scrolls itself;
/// the dock, the composer and the status are the page's bar.
pub fn phone(app: AppState, id: Id) -> PhonePage {
    let page = view_of(app, id.clone());
    follow(app, id.clone(), page);
    let title = untracked(|| page.with(|p| p.header.target.clone()));
    let body = view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(header::phone_line(app, page))
        .child(banner(app, page))
        .child(tabs::strip(app, page))
        .child(main(app, page))
        .into_element();
    let bar = view()
        .bg(color(Token::Bg))
        .border_t_1()
        .border_color(color(Token::Rule))
        .child(dock::dock(app, page))
        .child(bottom(app, page, id))
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

/// The session's keys while its page is open (desktop and terminal): the tabs by their terminal
/// key and position, thinking and the receiver. Escape is the app's (it returns to Chat, then
/// stops the turn, before it leaves the page) and Enter the composer's; nothing here acts while
/// a sheet or overlay is up.
fn bind_keys(app: AppState, id: Id) {
    let web = cfg!(target_arch = "wasm32");
    let options = KeyOptions {
        when: When::Always,
        ..KeyOptions::default()
    };
    for binding in session::bound(app.form, web) {
        if matches!(binding.action, KeyAction::Escape | KeyAction::Send(_)) {
            continue;
        }
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
    Agents,
    Log,
}

fn main(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || {
        page.with(|p| match &p.body {
            Body::Chat(_) => Shown::Chat,
            Body::Agents(_) => Shown::Agents,
            Body::Log(_) => Shown::Log,
        })
    });
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(dynamic(move || {
            let shown = shown.get();
            untracked(|| match shown {
                Shown::Chat => chat(app, page),
                Shown::Agents => agents_tab::tab(app, page),
                Shown::Log => pager::pager(app, page),
            })
        }))
        .into_element()
}

/// Chat: the transcript in its own scroll, which keeps to the newest rows as they arrive unless
/// the owner scrolled up; before the first row, who receives the message.
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
    let surface = follow_end(
        scroll()
            .test_id("session-transcript")
            .flex_1()
            .min_h(px(0.0)),
        move || chat.with(|_| ()),
    );
    if app.form == Form::Terminal {
        surface
            .child(view().px(term::cells(1.0)).child(shown))
            .into_element()
    } else {
        surface.child(shown).into_element()
    }
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

/// The composer, or the bar in its place while the session takes no message (not running,
/// ended, read-only), with Resume or Relaunch where the host can be started again.
fn bottom(app: AppState, page: Memo<SessionPage>, id: Id) -> AnyElement {
    let composer = computed(move || page.with(|p| p.composer.clone()));
    let has = computed(move || composer.with(Option::is_some));
    let bar = computed(move || page.with(|p| p.bar.clone()));
    dynamic(move || {
        if has.get() {
            let id = id.clone();
            untracked(|| session_composer(app, id, composer))
        } else {
            untracked(|| dock::host_bar(app, bar))
        }
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
