//! The assistant's blocks inside Agents: the pinned quick-thread tile (a compact row on the
//! phone and in a terminal), then the assistant sessions with + New session, and Filed from chat
//! naming the session each Idea came from, all from the served rows of the selected feed.

use nsh_core::prelude::*;

use crate::model::assistant::{
    Agents, FiledLine, LABELS, QuickThread, SessionRow, filings_target, live_agents,
};
use crate::model::{Authority, Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::dot;
use crate::ui::kit::page::group_head;
use crate::ui::kit::rows::list_row;
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::parts::{act, button_row, model_button, ring, spacer, test_id, words};

/// The blocks from the selected feed, tracked.
fn blocks(app: AppState) -> Agents {
    let can_send = app.can(Authority::Assistant);
    app.feed.with(|feed| {
        app.assistant
            .with(|state| live_agents(state, feed, can_send))
    })
}

/// The quick thread pinned first in Agents. Desktop: the tile (`.tile.qt`) that fills its cell:
/// state, id and pinning; title; the latest turn; the last question under a rule; model, host,
/// scope and cost in its foot. Phone: the pinned row on the surface (`.prow.qtrow`).
/// Terminal: one row. Pressing it opens the quick thread.
///
/// The tile quotes the quick thread's latest question and answer, so the page placing it holds
/// the thread's subscription for its own lifetime ([`super::open_scope`]). The tile itself
/// subscribes nothing: the Agents wall rebuilds it whenever agent rows change, and a
/// subscription per build would close and reopen the thread on every update.
pub fn agents_head(app: AppState) -> AnyElement {
    let quick = computed(move || blocks(app).quick_thread);
    dynamic(move || {
        let qt = quick.get();
        Some(match app.form {
            Form::Desktop => tile(app, qt),
            Form::Phone => phone_row(app, qt),
            Form::Terminal => terminal_row(app, qt),
        })
    })
}

fn tile(app: AppState, qt: QuickThread) -> AnyElement {
    let action = qt.action.clone();
    let small = |content: String| {
        text(content)
            .number_of_lines(1u32)
            .font_family(FONT_TEXT)
            .text_size(px(11.0))
            .text_color(color(Token::Ink3))
    };
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(12.0))
        .pt(px(9.0))
        .pb(px(2.0))
        .child(chip(app, qt.state.label.clone(), qt.state.hue))
        .child(
            text(qt.id.clone())
                .font_family(FONT_MONO)
                .text_size(px(11.5))
                .text_color(color(Token::Ink3)),
        )
        .child(spacer())
        .child(words(app, qt.pinned, 12.0, Token::Ink3).number_of_lines(1u32));
    let last = words(app, "", 12.5, Token::Ink2)
        .child(text(qt.owner_at).text_color(color(Token::Ink3)))
        .child(text(format!(" {}", qt.last_question)));
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .bg(color(Token::Surface))
        .rounded(px(8.0))
        .border_1()
        .border_color(color(Token::Rule2))
        .overflow_hidden()
        .role(AccessibilityRole::Button)
        .label(qt.title.clone())
        .test_id(&format!("thread-{}", qt.id))
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(ring)
        .on_press(move || act(app, action.clone()))
        .child(head)
        .child(
            words(app, qt.title, 13.5, Token::Ink)
                .font_weight(FontWeight::SEMIBOLD)
                .truncate()
                .px(px(12.0)),
        )
        .child(
            words(app, qt.last_line, 12.0, Token::Ink2)
                .truncate()
                .px(px(12.0))
                .pt(px(2.0))
                .pb(px(8.0)),
        )
        .child(
            view()
                .flex_1()
                .min_h(px(92.0))
                .px(px(12.0))
                .py(px(8.0))
                .overflow_hidden()
                .border_t_1()
                .border_t_color(color(Token::Rule))
                .child(last),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .px(px(12.0))
                .py(px(6.0))
                .border_t_1()
                .border_t_color(color(Token::Rule))
                .child(small(qt.model.into()))
                .child(small(qt.location.into()))
                .child(small(qt.scope.into()).flex_shrink(1.0).min_w(px(0.0)))
                .child(spacer())
                .child(small(qt.lifetime.into())),
        )
        .into_element()
}

fn phone_row(app: AppState, qt: QuickThread) -> AnyElement {
    let action = qt.action.clone();
    let first = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(mono(app, qt.id.clone(), 13.0, Token::Ink3))
        .child(
            words(app, qt.row_title, 16.0, Token::Ink)
                .font_weight(FontWeight::SEMIBOLD)
                .flex_1()
                .min_w(px(0.0))
                .truncate(),
        )
        .child(chip(app, qt.state.label.clone(), qt.state.hue));
    let second = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(
            words(app, qt.last_line, 13.5, Token::Ink2)
                .flex_1()
                .min_w(px(0.0))
                .truncate(),
        )
        .child(mono(app, LABELS.pinned, 12.0, Token::Ink3));
    view()
        .bg(color(Token::Surface))
        .border_t_1()
        .border_t_color(color(Token::Rule))
        .child(list_row(
            app,
            qt.title,
            format!("thread-{}", qt.id),
            || false,
            move || act(app, action.clone()),
            vec![
                view()
                    .w(px(12.0))
                    .flex_shrink_0()
                    .items_center()
                    .child(dot(app, Hue::Corral))
                    .into_element(),
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(2.0))
                    .child(first)
                    .child(second)
                    .into_element(),
            ],
        ))
        .into_element()
}

fn terminal_row(app: AppState, qt: QuickThread) -> AnyElement {
    let action = qt.action.clone();
    list_row(
        app,
        qt.title,
        format!("thread-{}", qt.id),
        || false,
        move || act(app, action.clone()),
        vec![
            dot(app, Hue::Corral),
            text(qt.id.clone())
                .flex_shrink_0()
                .text_color(color(Token::Ink3))
                .into_element(),
            text(qt.row_title)
                .flex_shrink_0()
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink))
                .into_element(),
            chip(app, qt.state.label.clone(), qt.state.hue),
            text(qt.last_line)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_color(color(Token::Ink))
                .into_element(),
            term::quiet(LABELS.pinned).into_element(),
        ],
    )
}

/// The assistant sessions and Filed from chat, under the agents. Desktop: two columns 24 pt
/// apart (`.cols`, padding 4 18 18): sessions as two-line rows (id and title; state chip and
/// facts), a full-width + New session, the disclosure; filed rows of id, title and source, time.
/// Phone: two sections with their heads (+ New session at the trailing edge) and two-line rows.
/// Terminal: the two sections stacked, one line per row.
///
/// The page placing these blocks settles their session operations for its own lifetime
/// ([`super::watch_ops`]); the blocks are rebuilt with every agent update.
pub fn agents_tail(app: AppState) -> AnyElement {
    load_filings(app);
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

/// Keeps the global filings page the wall shows served: fetched when absent and refetched when
/// it went stale. A failed fetch waits for the owner's Retry; nothing loops.
fn load_filings(app: AppState) {
    effect(move || {
        if !app.capable(corral_protocol::app::FILINGS_V1) {
            return;
        }
        let target = app.assistant.with(filings_target);
        let wanted = app.with_detail(&target, |cache| {
            cache.is_none_or(|cache| cache.stale && !cache.loading && cache.error.is_none())
        });
        if wanted {
            untracked(|| app.load(target));
        }
    });
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
    let filed = view()
        .flex_1()
        .min_w(px(0.0))
        .child(h3(
            app,
            blocks.filed_title,
            blocks.filed_total.map(|total| total.to_string()),
        ))
        .child(
            view()
                .gap(px(2.0))
                .children(blocks.filed.iter().map(|row| filed_row(app, row))),
        )
        .children(blocks.filed_note.clone().map(|note| {
            view()
                .py(px(4.0))
                .child(words(app, note, 12.0, Token::Ink3))
        }))
        .children(pager(app, blocks).map(|row| view().py(px(4.0)).child(row)))
        .children(
            blocks
                .filed_illustrative
                .map(|label| view().py(px(4.0)).child(illus(app, label))),
        );
    view()
        .flex_row()
        .items_start()
        .gap(px(24.0))
        .px(px(18.0))
        .pt(px(4.0))
        .pb(px(18.0))
        .child(sessions)
        .child(filed)
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

/// A desktop Filed from chat row (`.irow.fr`): mono id, the title over its source, the time.
fn filed_row(app: AppState, row: &FiledLine) -> AnyElement {
    let action = row.action.clone();
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(4.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .role(AccessibilityRole::Button)
        .label(format!("{} {}", row.id, row.title))
        .test_id(&format!("task-{}", row.id))
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(ring)
        .on_press(move || act(app, action.clone()))
        .child(view().w(px(46.0)).flex_shrink_0().child(mono(
            app,
            row.id.clone(),
            11.5,
            Token::Ink3,
        )))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(words(app, row.title.clone(), 12.5, Token::Ink).truncate())
                .child(source(app, row, 11.0)),
        )
        .child(words(app, row.at.clone(), 12.0, Token::Ink3).flex_shrink_0())
        .into_element()
}

/// The row's source line as the model wrote it (`From A-7 · quick thread`); the credit is the
/// model's to state, never added here.
fn source(app: AppState, row: &FiledLine, size: f32) -> Text {
    words(app, row.source.clone(), size, Token::Ink3).truncate()
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
        .child(group_head(
            app,
            blocks.filed_title,
            blocks.filed_total.map(|total| total.to_string()),
            Hue::Neutral,
            None,
            None,
        ))
        .children(blocks.filed.iter().map(|row| {
            let action = row.action.clone();
            list_row(
                app,
                format!("{} {}", row.id, row.title),
                format!("task-{}", row.id),
                || false,
                move || act(app, action.clone()),
                vec![
                    view()
                        .w(px(46.0))
                        .flex_shrink_0()
                        .child(mono(app, row.id.clone(), 13.0, Token::Ink3))
                        .into_element(),
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap(px(2.0))
                        .child(words(app, row.title.clone(), 14.5, Token::Ink).truncate())
                        .child(source(app, row, 12.0))
                        .into_element(),
                    words(app, row.at.clone(), 12.5, Token::Ink3)
                        .flex_shrink_0()
                        .into_element(),
                ],
            )
        }))
        .children(blocks.filed_note.clone().map(|note| {
            view()
                .px(px(18.0))
                .pt(px(6.0))
                .child(words(app, note, 13.0, Token::Ink3))
        }))
        .children(pager(app, blocks).map(|row| view().px(px(18.0)).pt(px(6.0)).child(row)))
        .children(blocks.filed_illustrative.map(|label| {
            view()
                .px(px(18.0))
                .pt(px(6.0))
                .pb(px(20.0))
                .child(illus(app, label))
        }))
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
        .child(term::blank())
        .child(group_head(
            app,
            blocks.filed_title,
            blocks.filed_total.map(|total| total.to_string()),
            Hue::Neutral,
            blocks
                .filed_note
                .clone()
                .or_else(|| blocks.filed_illustrative.map(str::to_owned)),
            None,
        ))
        .children(blocks.filed.iter().map(|row| {
            let action = row.action.clone();
            list_row(
                app,
                format!("{} {}", row.id, row.title),
                format!("task-{}", row.id),
                || false,
                move || act(app, action.clone()),
                vec![
                    view()
                        .w(term::cells(5.0))
                        .flex_shrink_0()
                        .child(text(row.id.clone()).text_color(color(Token::Ink3)))
                        .into_element(),
                    text(row.title.clone())
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_color(color(Token::Ink))
                        .into_element(),
                    term::quiet(row.source.clone()).into_element(),
                    term::quiet(row.at.clone()).into_element(),
                ],
            )
        }))
        .children(pager(app, blocks).map(|row| view().px(term::cells(1.0)).child(row)))
        .into_element()
}

/// Newest, Newer, Older (or Retry) under the served filings page, quiet; nothing without them.
fn pager(app: AppState, blocks: &Agents) -> Option<AnyElement> {
    (!blocks.filed_pager.is_empty())
        .then(|| button_row(app, &blocks.filed_pager, ButtonKind::Quiet, 6.0).into_element())
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
