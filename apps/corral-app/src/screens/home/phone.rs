//! Home on a phone: the three sections of one list (Needs you, Working, Just finished), tasks
//! and sessions in one row anatomy. A small thing is answered in the list in place: a Blocked
//! task whose next step is a reply gets its reply box, a chat's question its dock and composer.
//! Anything else opens its page. No dashboard numbers; with a project in scope the title is the
//! project and a line of its facts shows.

use nsh_core::prelude::*;

use crate::model::list::{self, Section};
use crate::model::projects::Scope;
use crate::model::{Id, Live, home, work};
use crate::screens::agents::{Compact, ask_session, ask_view};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::list::phone_lines;
use crate::ui::kit::next::next_button;
use crate::ui::kit::page::{head, search};
use crate::ui::kit::status::look_mark;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::cards::reply_box;

pub(super) fn screen(app: AppState, scope: Memo<Scope>) -> AnyElement {
    let said = computed(move || {
        let scope = scope.get();
        app.feed.with(|feed| {
            (
                home::title(&scope),
                home::summary(feed, &scope),
                scope
                    .project()
                    .map(|id| home::project_facts(feed, id))
                    .unwrap_or_default(),
            )
        })
    });
    let top = dynamic(move || {
        let (title, summary, facts) = said.get();
        untracked(move || {
            view()
                .child(head(app, title, None, vec![search(app)]))
                .child(
                    view()
                        .px(px(18.0))
                        .pb(px(6.0))
                        .gap(px(4.0))
                        .child(
                            text(summary)
                                .test_id("home-summary")
                                .font_family(FONT_TEXT)
                                .text_size(px(14.0))
                                .text_color(color(Token::Ink2)),
                        )
                        .when(!facts.is_empty(), |lines| {
                            lines.child(
                                text(
                                    facts
                                        .into_iter()
                                        .map(|(label, value)| format!("{label} {value}"))
                                        .collect::<Vec<_>>()
                                        .join(" · "),
                                )
                                .test_id("home-facts")
                                .font_family(FONT_TEXT)
                                .text_size(px(13.0))
                                .text_color(color(Token::Ink3)),
                            )
                        }),
                )
                .into_element()
        })
    });
    let sections = computed(move || {
        let scope = scope.get();
        app.feed.with(|feed| home::sections(feed, &scope))
    });
    let rows = dynamic(move || {
        let sections = sections.get();
        untracked(move || {
            if sections.is_empty() {
                return view()
                    .px(px(18.0))
                    .py(px(16.0))
                    .child(
                        text(home::EMPTY)
                            .font_family(FONT_TEXT)
                            .text_size(px(13.0))
                            .text_color(color(Token::Ink3)),
                    )
                    .into_element();
            }
            let mut out = Vec::new();
            for section in &sections {
                out.push(section_head(section));
                out.extend(section.rows.iter().map(|id| row(app, id.clone())));
            }
            view().children(out).into_element()
        })
    });
    view()
        .size_full()
        .flex_grow(1.0)
        .flex_col()
        .min_h(px(0.0))
        .bg(color(Token::Bg))
        .child(top)
        .child(
            scroll()
                .test_id("list-home")
                .element_ref(&app.list_keys)
                .flex_1()
                .min_h(px(0.0))
                .child(rows),
        )
        .into_element()
}

fn section_head(section: &Section) -> AnyElement {
    view()
        .test_id(&format!("section-{}", section.key))
        .role(AccessibilityRole::Header)
        .label(format!("{}, {}", section.label, section.count))
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .pt(px(14.0))
        .pb(px(6.0))
        .px(px(18.0))
        .child(
            text(section.label.clone())
                .font_family(FONT_TEXT)
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink)),
        )
        .child(
            text(section.count.to_string())
                .font_family(FONT_MONO)
                .text_size(px(12.0))
                .text_color(color(Token::Ink3)),
        )
        .into_element()
}

/// One row: its mark, its id and title over what is happening, its next step (or the chevron
/// to its page); under it, for a small thing, the answer in place.
fn row(app: AppState, id: Id) -> AnyElement {
    let key = id.clone();
    let line = computed(move || app.feed.with(|feed| home::row(feed, &key)));
    let test_id = app
        .feed
        .with_untracked(|feed| list::row_id(&home::live_of(feed, &id)));
    let spoken = line.with_untracked(|line| line.as_ref().map(home::HomeRow::spoken));
    let shown = id.to_string();
    let content = dynamic(move || {
        let current = line.get();
        let shown = shown.clone();
        untracked(move || {
            current.map(|row| {
                let trailing = match &row.next {
                    Some(next) if !row.in_place => next_button(app, &shown, next, false),
                    _ => icon(Icon::Chevron, 16.0, color(Token::Ink3)),
                };
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .child(look_mark(app, row.look, row.word))
                    .child(phone_lines(app, shown, row.title, row.now))
                    .child(trailing)
                    .into_element()
            })
        })
    });
    let opens = id.clone();
    let line_view = view()
        .test_id(&test_id)
        .role(AccessibilityRole::Button)
        .label(spoken.unwrap_or_default())
        .cursor_pointer()
        .min_h(px(60.0))
        .py(px(10.0))
        .px(px(18.0))
        .justify_center()
        .active(|s| s.bg(color(Token::Hover)))
        .on_press(move || {
            let live = app.feed.with_untracked(|feed| home::live_of(feed, &opens));
            app.open_live(live);
        })
        .child(content);
    let in_place = computed(move || {
        line.with(|line| {
            line.as_ref()
                .filter(|line| line.in_place)
                .map(|line| line.live.clone())
        })
    });
    let answer = dynamic(move || {
        let live = in_place.get();
        untracked(move || live.map(|live| answer(app, live)))
    });
    view()
        .border_b_1()
        .border_b_color(color(Token::Rule))
        .child(line_view)
        .child(answer)
        .into_element()
}

/// The answer in place: a task's reply box, or a chat's own dock and composer.
fn answer(app: AppState, live: Live) -> AnyElement {
    let body = match live {
        Live::Session(id) => {
            let page = ask_view(app, id.clone());
            ask_session(app, id, page, Compact::Card)
        }
        Live::Task(id) | Live::Plan(id) | Live::Review(id) => {
            let reply = computed(move || {
                app.feed
                    .with(|feed| feed.board.task(&id).and_then(work::reply_box))
            });
            dynamic(move || {
                let reply = reply.get();
                untracked(|| reply.map(|reply| reply_box(app, reply)))
            })
            .into_element()
        }
    };
    view()
        .test_id("home-in-place")
        .px(px(18.0))
        .pb(px(12.0))
        .child(body)
        .into_element()
}
