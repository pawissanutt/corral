//! The assistant's block inside Agents, under the roster: the assistant sessions with + New
//! session, from the served rows of the selected feed.

use nsh_core::prelude::*;

use crate::model::assistant::{Agents, SessionRow, live_agents};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::dot;
use crate::ui::kit::page::group_head;
use crate::ui::kit::rows::list_row;
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, Token, color};

use super::parts::{act, model_button, ring, test_id, words};

/// The blocks from the selected feed, tracked.
fn blocks(app: AppState) -> Agents {
    app.feed.with(|feed| app.assistant.with(|state| live_agents(state, feed)))
}

/// The assistant sessions, under the agents. Desktop (padding 4 18 18): sessions as two-line
/// rows (id and title; state chip and facts), a full-width + New session, the disclosure.
/// Phone: the section with its head (+ New session at the trailing edge) and two-line rows.
/// Terminal: one line per row.
///
/// The page placing these blocks settles their session operations for its own lifetime
/// ([`super::watch_ops`]); the blocks are rebuilt with every agent update.
pub fn agents_tail(app: AppState) -> AnyElement {
    let built = computed(move || blocks(app));
    dynamic(move || {
        let blocks = built.get();
        Some(match app.form {
            Form::Desktop => desktop_tail(app, &blocks),
            Form::Phone => phone_tail(app, &blocks),
            Form::Terminal => terminal_tail(app, &blocks),
        })
    })
}

/// The disclosure, the loading/empty/refusal line, or nothing.
fn sessions_foot(app: AppState, blocks: &Agents) -> Option<AnyElement> {
    if let Some(note) = &blocks.sessions_note {
        return Some(words(app, note.clone(), 12.0, Token::Ink3).into_element());
    }
    blocks.sessions_illustrative.map(|label| illus(app, label))
}

fn desktop_tail(app: AppState, blocks: &Agents) -> AnyElement {
    let sessions = view()
        .flex_1()
        .min_w(px(0.0))
        .child(h3(
            app,
            blocks.sessions_title,
            Some(blocks.sessions_count.clone()),
        ))
        .children(blocks.sessions.iter().map(|row| session_row(app, row)))
        .child(view().flex_row().my(px(8.0)).child(button_with(
            app,
            blocks.new_session.label,
            ButtonKind::Ghost,
            ButtonOptions {
                wide: true,
                disabled: !blocks.new_session.enabled,
                ..ButtonOptions::default()
            },
            test_id(&blocks.new_session),
            {
                let action = blocks.new_session.action.clone();
                move || act(app, action.clone())
            },
        )))
        .children(sessions_foot(app, blocks).map(|foot| view().py(px(4.0)).child(foot)));
    view()
        .flex_row()
        .items_start()
        .px(px(18.0))
        .pt(px(4.0))
        .pb(px(18.0))
        .child(sessions)
        .into_element()
}

/// A desktop session row (`.srow`): mono id and bold title, then the state chip and the facts.
fn session_row(app: AppState, row: &SessionRow) -> AnyElement {
    let action = row.action.clone();
    view()
        .gap(px(3.0))
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(6.0))
        .role(AccessibilityRole::Button)
        .label(format!("{} {}", row.id, row.title))
        .test_id(&format!("thread-{}", row.id))
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(ring)
        .on_press(move || act(app, action.clone()))
        .child(
            words(app, "", 13.0, Token::Ink)
                .truncate()
                .child(
                    text(row.id.clone())
                        .font_family(FONT_MONO)
                        .text_size(px(12.0))
                        .text_color(color(Token::Ink3)),
                )
                .child(text(" "))
                .child(text(row.title.clone()).font_weight(FontWeight::BOLD)),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .child(chip(app, row.state.label.clone(), row.state.hue))
                .child(words(app, row.desktop_facts.clone(), 11.5, Token::Ink3)),
        )
        .into_element()
}

fn phone_tail(app: AppState, blocks: &Agents) -> AnyElement {
    let new_session = model_button(app, &blocks.new_session, ButtonKind::Quiet);
    view()
        .child(group_head(
            app,
            blocks.sessions_title,
            Some(blocks.sessions.len().to_string()),
            Hue::Neutral,
            None,
            Some(new_session),
        ))
        .children(blocks.sessions.iter().map(|row| {
            let action = row.action.clone();
            let first = view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(mono(app, row.id.clone(), 13.0, Token::Ink3))
                .child(
                    words(app, row.title.clone(), 16.0, Token::Ink)
                        .font_weight(FontWeight::SEMIBOLD)
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate(),
                )
                .child(mono(app, row.cost.clone(), 12.0, Token::Ink3));
            list_row(
                app,
                format!("{} {}", row.id, row.title),
                format!("thread-{}", row.id),
                || false,
                move || act(app, action.clone()),
                vec![
                    view()
                        .w(px(12.0))
                        .flex_shrink_0()
                        .items_center()
                        .child(dot(app, row.state.hue))
                        .into_element(),
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap(px(2.0))
                        .child(first)
                        .child(words(app, row.phone_facts.clone(), 13.5, Token::Ink2).truncate())
                        .into_element(),
                ],
            )
        }))
        .children(
            sessions_foot(app, blocks).map(|foot| view().px(px(18.0)).py(px(6.0)).child(foot)),
        )
        .into_element()
}

fn terminal_tail(app: AppState, blocks: &Agents) -> AnyElement {
    let new_session = model_button(app, &blocks.new_session, ButtonKind::Ghost);
    view()
        .child(group_head(
            app,
            blocks.sessions_title,
            Some(blocks.sessions_count.clone()),
            Hue::Neutral,
            None,
            None,
        ))
        .children(blocks.sessions.iter().map(|row| {
            let action = row.action.clone();
            list_row(
                app,
                format!("{} {}", row.id, row.title),
                format!("thread-{}", row.id),
                || false,
                move || act(app, action.clone()),
                vec![
                    dot(app, row.state.hue),
                    view()
                        .w(term::cells(5.0))
                        .flex_shrink_0()
                        .child(text(row.id.clone()).text_color(color(Token::Ink3)))
                        .into_element(),
                    text(row.title.clone())
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink))
                        .into_element(),
                    chip(app, row.state.label.clone(), row.state.hue),
                    text(row.desktop_facts.clone())
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_color(color(Token::Ink3))
                        .into_element(),
                ],
            )
        }))
        .child(
            view()
                .flex_row()
                .px(term::cells(1.0))
                .child(new_session)
                .children(
                    sessions_foot(app, blocks)
                        .map(|foot| view().flex_row().child(term::space(2.0)).child(foot)),
                ),
        )
        .into_element()
}

/// A mono id, cost or caption in one line.
fn mono(app: AppState, content: impl Into<SharedString>, size: f32, ink: Token) -> Text {
    let t = text(content.into())
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(color(ink));
    if app.form == Form::Terminal {
        t
    } else {
        t.font_family(FONT_MONO).tabular_nums().text_size(px(size))
    }
}
