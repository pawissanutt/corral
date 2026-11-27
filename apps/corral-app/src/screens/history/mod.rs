//! Insight › Git history: one project's commits on one ref, read-only. The head offers Refresh;
//! under it the branch field, the refs (HEAD and the project's local branches the field narrows
//! to) and the walk's pinned tip, then the
//! commits as rows beside their lane graph (`graph.rs`), Load older under them, and the
//! selected commit's inspector beside the rows (under them on the phone and on a narrow
//! terminal). With All projects in scope the page asks for one. Every word, number and branch
//! is `model::history`'s.
//!
//! Test ids: "page-title", "btn-history-refresh", "btn-history-retry", "history",
//! "history-tip", "field-history-branch", "seg-history-branch-<HEAD|branch>",
//! "system-pick-<project>", "history-overflow", "commit-<sha12>", "history-below", "btn-history-more", "inspector", "inspector-title",
//! "history-link-<n>", "history-links-more", "history-refs-incomplete",
//! "btn-history-task-<task>", "btn-history-session-<session>".

mod branch;
mod graph;

use nsh_core::prelude::*;

use crate::model::history::{
    ASK_HEADING, ASK_WORDS, Action, BRANCH_CAPTION, EMPTY, EMPTY_HEADING, HINT, Inspector, LINKS,
    LOAD_OLDER, LOADING, LOADING_OLDER, LinkLine, More, PARENTS, Page, REFRESH, Row, STALE, Status,
    TIP_CAPTION, TITLE, UNAVAILABLE_HEADING, open_session, open_task, page, unavailable,
};
use crate::model::list::inspector_width;
use crate::model::projects::{Scope, branches_ref};
use crate::model::text::span;
use crate::model::{Form, Id, Live};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::notice::{empty, loading, notice};
use crate::ui::kit::page::{
    Width, body, columns, context, head, measure_fill, measured_head, pane,
};
use crate::ui::kit::rows::{list_row, list_row_tall};
use crate::ui::kit::text::{h3, label_value, mono, plain};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, MEASURE_PAGE, Token, color};

/// The least terminal page that keeps the inspector beside the rows.
const BESIDE_CELLS: f32 = 100.0;
/// A stacked terminal inspector's rows.
const STACKED_ROWS: f32 = 12.0;

pub fn screen(app: AppState) -> AnyElement {
    let data = computed(move || {
        let scope = app.project_scope();
        let state = app.history.get();
        app.feed.with(|feed| page(&state, feed, &scope))
    });
    keep_loaded(app, data);
    let asking = computed(move || data.with(|page| page.project.is_none()));
    let content = dynamic(move || {
        if asking.get() {
            untracked(|| picker(app, data))
        } else {
            untracked(|| shown(app, data))
        }
    });
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(page_head(app, data))
        .child(measure_fill(app, MEASURE_PAGE, content.into_element()))
        .into_element()
}

/// Follows the scope's project and keeps its chosen ref's first page and its branches read.
fn keep_loaded(app: AppState, data: Memo<Page>) {
    let project = computed(move || data.with(|page| page.project.clone()));
    effect(move || {
        if let Some(project) = project.get() {
            untracked(|| app.history_act(Action::Follow(project)));
        }
    });
    let wanted = computed(move || {
        let target = data.with(|page| page.target.clone())?;
        app.daemon().then_some(target)
    });
    effect(move || {
        if let Some(target) = wanted.get() {
            untracked(|| app.load(target));
        }
    });
    let branches = computed(move || {
        let project = project.get()?;
        app.daemon().then(|| branches_ref(&project))
    });
    effect(move || {
        if let Some(target) = branches.get() {
            untracked(|| app.load(target));
        }
    });
}

fn refresh(app: AppState, data: Memo<Page>, id: &'static str) -> AnyElement {
    let target = computed(move || data.with(|page| page.target.clone()));
    dynamic(move || {
        let target = target.get()?;
        Some(untracked(|| {
            button(app, REFRESH, ButtonKind::Ghost, id, move || {
                app.refresh(target.clone())
            })
        }))
    })
    .into_element()
}

fn page_head(app: AppState, data: Memo<Page>) -> AnyElement {
    let actions = vec![refresh(app, data, "btn-history-refresh")];
    if app.form == Form::Phone {
        return head(app, TITLE, None, actions);
    }
    measured_head(app, MEASURE_PAGE, TITLE, None, actions)
}

/// All projects in scope: the page asks for one, as Insight's other pages do.
fn picker(app: AppState, data: Memo<Page>) -> AnyElement {
    let projects = computed(move || data.with(|page| page.projects.clone()));
    let choices = dynamic(move || {
        let projects = projects.get();
        untracked(|| {
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(8.0))
                .children(projects.into_iter().map(|id| {
                    let pick = id.clone();
                    button(
                        app,
                        id.to_string(),
                        ButtonKind::Ghost,
                        format!("system-pick-{id}"),
                        move || app.set_scope(Scope::Project(pick.clone())),
                    )
                }))
                .into_element()
        })
    })
    .into_element();
    body(
        app,
        "history",
        empty(app, ASK_HEADING, ASK_WORDS, Some(choices)),
    )
}

fn quiet(app: AppState, words: impl Into<SharedString>) -> Text {
    let t = text(words.into()).text_color(color(Token::Ink3));
    if app.form == Form::Terminal {
        t
    } else {
        t.font_family(FONT_TEXT).text_size(px(12.0))
    }
}

/// The refs, one press each, and the pinned tip.
fn refs_bar(app: AppState, data: Memo<Page>) -> AnyElement {
    let form = app.form;
    let branches = computed(move || data.with(|page| page.branches.clone()));
    let choices = dynamic(move || {
        let branches = branches.get();
        untracked(|| {
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(if form == Form::Terminal { 8.0 } else { 6.0 }))
                .role(AccessibilityRole::Group)
                .label(BRANCH_CAPTION)
                .child(quiet(app, BRANCH_CAPTION).flex_shrink_0())
                .children(branches.into_iter().map(|choice| {
                    let kind = if choice.chosen {
                        ButtonKind::Primary
                    } else {
                        ButtonKind::Ghost
                    };
                    let branch = choice.branch.clone();
                    button(app, choice.label, kind, choice.id, move || {
                        app.history_act(Action::Branch(branch.clone()))
                    })
                }))
                .into_element()
        })
    })
    .into_element();
    let tip = text(move || {
        data.with(|page| {
            if page.pinned.is_empty() {
                String::new()
            } else {
                format!("{TIP_CAPTION}: {}", page.pinned)
            }
        })
    })
    .test_id("history-tip")
    .number_of_lines(1u32)
    .text_color(color(Token::Ink3))
    .when(form != Form::Terminal, |t| {
        t.font_family(FONT_TEXT).text_size(px(12.0))
    });
    view()
        .flex_none()
        .flex_col()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .when(form == Form::Terminal, |v| v.px(term::cells(1.0)))
        .when(form != Form::Terminal, |v| {
            v.px(px(16.0))
                .py(px(8.0))
                .border_b(px(1.0))
                .border_color(color(Token::Rule))
        })
        .child(branch::field(app))
        .child(choices)
        .child(tip)
        .into_element()
}

/// Where the read stands, when it is not simply shown.
fn state_block(app: AppState, data: Memo<Page>) -> AnyElement {
    let status = computed(move || {
        data.with(|page| {
            let rows = !page.rows.is_empty();
            page.status
                .clone()
                .filter(|status| !(rows && *status == Status::Loading))
        })
    });
    dynamic(move || {
        let status = status.get()?;
        Some(untracked(|| match status {
            Status::Ready => view().into_element(),
            Status::Loading => loading(app, LOADING, &[(0.6, 1.0), (0.45, 1.0), (0.7, 1.0)]),
            Status::Empty => empty(app, EMPTY_HEADING, EMPTY, None),
            Status::Unavailable(why) => empty(
                app,
                UNAVAILABLE_HEADING,
                unavailable(&why),
                Some(refresh(app, data, "btn-history-retry")),
            ),
            Status::Stale(why) => notice(
                app,
                crate::model::Hue::Held,
                &vec![span(format!("{STALE} {why}"))],
                None,
            ),
        }))
    })
    .into_element()
}

fn commit_row(app: AppState, row: Row, lanes: usize) -> AnyElement {
    let form = app.form;
    let sha = row.sha.clone();
    let selected = move || {
        app.history
            .with(|state| state.selected.as_deref() == Some(sha.as_str()))
    };
    let pick = row.sha.clone();
    let press = move || app.history_act(Action::Select(pick.clone()));
    let graph = graph::cell(app, &row.graph, &row.glyphs, lanes);
    let short = mono(app, row.short.clone())
        .flex_shrink_0()
        .text_color(color(Token::Ink3));
    let more = |words: &Option<String>| {
        words
            .clone()
            .map(|words| quiet(app, words).flex_shrink_0().into_element())
    };
    let mut refs: Vec<AnyElement> = if form == Form::Terminal {
        (!row.refs.is_empty())
            .then(|| {
                text(format!("({})", row.refs.join(", ")))
                    .flex_shrink_0()
                    .number_of_lines(1u32)
                    .text_color(hue_ink(crate::model::Hue::Held))
                    .into_element()
            })
            .into_iter()
            .collect()
    } else {
        row.refs
            .iter()
            .map(|name| chip(app, name.clone(), crate::model::Hue::Neutral))
            .collect()
    };
    refs.extend(more(&row.refs_more));
    let mut marks: Vec<AnyElement> = row
        .marks
        .iter()
        .map(|mark| {
            if form == Form::Terminal {
                text(mark.words.clone())
                    .flex_shrink_0()
                    .number_of_lines(1u32)
                    .text_color(hue_ink(mark.hue))
                    .into_element()
            } else {
                chip(app, mark.words.clone(), mark.hue)
            }
        })
        .collect();
    marks.extend(more(&row.links_more));
    let subject = plain(app, row.subject.clone()).number_of_lines(1u32);
    let subject = view().flex_1().min_w(px(0.0)).child(subject).into_element();
    let wide = form != Form::Terminal || term::page_columns(app) >= BESIDE_CELLS;
    let by = if wide {
        format!("{} · {}", row.author, row.date)
    } else {
        row.date.clone()
    };
    let by = quiet(app, by)
        .flex_shrink_0()
        .number_of_lines(1u32)
        .into_element();
    if form == Form::Phone {
        let first = view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(graph)
            .child(short)
            .child(subject)
            .into_element();
        let second = view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .children(refs)
            .child(by)
            .children(marks)
            .into_element();
        let lines = view()
            .flex_1()
            .min_w(px(0.0))
            .gap(px(4.0))
            .child(first)
            .child(second)
            .into_element();
        return list_row_tall(app, row.label, row.id, selected, press, vec![lines]);
    }
    let mut content = vec![graph, short.into_element()];
    content.extend(refs);
    content.push(subject);
    content.extend(marks);
    content.push(by);
    list_row(app, row.label, row.id, selected, press, content)
}

/// The rows, the line of lanes continuing below them, and Load older.
fn commits(app: AppState, data: Memo<Page>) -> AnyElement {
    let rows = computed(move || data.with(|page| (page.rows.clone(), page.lanes)));
    let list = dynamic(move || {
        let (rows, lanes) = rows.get();
        untracked(|| {
            view()
                .test_id("history-rows")
                .children(rows.into_iter().map(|row| commit_row(app, row, lanes)))
                .into_element()
        })
    })
    .into_element();
    let below = computed(move || {
        data.with(|page| {
            page.continuing
                .clone()
                .map(|(glyphs, words)| (glyphs, words, page.lanes, page.below.clone()))
        })
    });
    let below = dynamic(move || {
        let (glyphs, words, lanes, open) = below.get()?;
        Some(untracked(|| {
            view()
                .test_id("history-below")
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .when(app.form == Form::Desktop, |v| v.pl(px(16.0)))
                .when(app.form == Form::Phone, |v| v.px(px(18.0)))
                .when(app.form == Form::Terminal, |v| v.pl(term::cells(1.0)))
                .child(graph::continuing(app, &glyphs, lanes, open))
                .child(quiet(app, words))
                .into_element()
        }))
    })
    .into_element();
    let more = computed(move || data.with(|page| page.more.clone()));
    let more = dynamic(move || {
        let more = more.get()?;
        Some(untracked(|| match more {
            More::None => view().into_element(),
            More::Loading => quiet(app, LOADING_OLDER).into_element(),
            More::Capped(words) => quiet(app, words).into_element(),
            More::Offer(get) => button(
                app,
                LOAD_OLDER,
                ButtonKind::Quiet,
                "btn-history-more",
                move || app.detail(get.clone()),
            ),
        }))
    })
    .into_element();
    let more = view()
        .flex_row()
        .when(app.form == Form::Desktop, |v| v.px(px(16.0)).py(px(8.0)))
        .when(app.form == Form::Phone, |v| v.px(px(18.0)).py(px(10.0)))
        .when(app.form == Form::Terminal, |v| v.pl(term::cells(1.0)))
        .child(more)
        .into_element();
    // Only the desktop canvas caps its lanes; glyph rows draw them all.
    let overflow = computed(move || data.with(|page| page.overflow.clone()));
    let overflow = dynamic(move || {
        let words = overflow.get().filter(|_| app.form == Form::Desktop)?;
        Some(untracked(|| {
            quiet(app, words).test_id("history-overflow").into_element()
        }))
    });
    let overflow = view().px(px(16.0)).child(overflow).into_element();
    let refs_note = computed(move || data.with(|page| page.refs_note));
    let refs_note = dynamic(move || {
        let words = refs_note.get()?;
        Some(untracked(|| {
            view()
                .test_id("history-refs-incomplete")
                .child(notice(
                    app,
                    crate::model::Hue::Held,
                    &vec![span(words)],
                    None,
                ))
                .into_element()
        }))
    })
    .into_element();
    view()
        .flex_col()
        .min_w(px(0.0))
        .child(state_block(app, data))
        .child(refs_note)
        .child(overflow)
        .child(list)
        .child(below)
        .child(more)
        .into_element()
}

fn link_block(app: AppState, n: usize, link: LinkLine) -> AnyElement {
    let words = plain(app, link.words.clone())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(hue_ink(link.hue));
    let task = link.task.clone();
    let mut buttons = vec![button(
        app,
        open_task(&link.task),
        ButtonKind::Ghost,
        format!("btn-history-task-{}", link.task),
        move || app.open_live(Live::Task(Id::from(task.as_str()))),
    )];
    if let Some(session) = link.session.clone() {
        buttons.push(button(
            app,
            open_session(&session),
            ButtonKind::Ghost,
            format!("btn-history-session-{session}"),
            move || app.open_live(Live::Session(Id::from(session.as_str()))),
        ));
    }
    view()
        .test_id(&format!("history-link-{n}"))
        .flex_col()
        .gap(px(4.0))
        .child(words)
        .children((!link.title.is_empty()).then(|| quiet(app, link.title.clone())))
        .child(view().flex_row().flex_wrap().gap(px(8.0)).children(buttons))
        .into_element()
}

fn inspected(app: AppState, inspector: Inspector) -> AnyElement {
    let form = app.form;
    let title = text(inspector.subject.clone())
        .test_id("inspector-title")
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink))
        .when(form != Form::Terminal, |t| {
            t.font_family(FONT_MONO).text_size(px(15.0))
        });
    let facts = inspector
        .facts
        .into_iter()
        .map(|(label, value)| label_value(app, label, plain(app, value).into_element()));
    let parents = inspector.parents.into_iter().map(|(sha, note)| {
        view()
            .flex_row()
            .flex_wrap()
            .gap_x(px(8.0))
            .children((!sha.is_empty()).then(|| mono(app, sha)))
            .children(note.map(|note| quiet(app, note)))
            .into_element()
    });
    let links: Vec<AnyElement> = inspector
        .links
        .into_iter()
        .enumerate()
        .map(|(n, link)| link_block(app, n, link))
        .collect();
    let mut parts = vec![title.into_element()];
    parts.extend(facts);
    parts.push(h3(app, PARENTS, None));
    parts.extend(parents);
    if !links.is_empty() || inspector.links_more.is_some() {
        parts.push(h3(app, LINKS, None));
        parts.extend(links);
        parts.extend(inspector.links_more.map(|words| {
            quiet(app, words)
                .test_id("history-links-more")
                .into_element()
        }));
    }
    parts.extend(
        inspector
            .caveat
            .map(|caveat| quiet(app, caveat).into_element()),
    );
    view()
        .test_id("inspector-body")
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .children(parts)
        .into_element()
}

fn inspector(data: Memo<Page>) -> Memo<Option<Inspector>> {
    computed(move || data.with(|page| page.inspector.clone()))
}

fn inspector_body(app: AppState, shown: Memo<Option<Inspector>>) -> AnyElement {
    dynamic(move || {
        let inspector = shown.get();
        untracked(|| match inspector {
            None => quiet(app, HINT).into_element(),
            Some(inspector) => inspected(app, inspector),
        })
    })
    .into_element()
}

fn shown(app: AppState, data: Memo<Page>) -> AnyElement {
    let form = app.form;
    let main = commits(app, data);
    let picked = inspector(data);
    let page = match form {
        Form::Phone => pane(
            app,
            "history",
            view()
                .child(main)
                .child(context(app, "inspector", inspector_body(app, picked)))
                .into_element(),
        ),
        Form::Desktop => columns(
            app,
            vec![
                (Width::Fill, pane(app, "history", main)),
                (
                    Width::Fixed(inspector_width(1200.0, false)),
                    context(app, "inspector", inspector_body(app, picked)),
                ),
            ],
            0,
        ),
        Form::Terminal => terminal(app, main, picked),
    };
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(refs_bar(app, data))
        .when(form == Form::Terminal, |v| v.child(term::hrule()))
        .child(page)
        .into_element()
}

/// A wide terminal keeps the inspector beside the rows; a narrow one puts the selected
/// commit's under them, so an 80-column row keeps its subject.
fn terminal(app: AppState, main: AnyElement, picked: Memo<Option<Inspector>>) -> AnyElement {
    let room = computed(move || term::page_columns(app));
    if untracked(|| room.get()) >= BESIDE_CELLS {
        let side = inspector_width(untracked(|| room.get()), true);
        return columns(
            app,
            vec![
                (Width::Fill, body(app, "history", main)),
                (
                    Width::Fixed(side * 12.0),
                    context(app, "inspector", inspector_body(app, picked)),
                ),
            ],
            0,
        );
    }
    let below = dynamic(move || {
        let open = picked.with(Option::is_some);
        Some(untracked(|| {
            let content = inspector_body(app, picked);
            if open {
                view()
                    .flex_none()
                    .flex_col()
                    .child(term::hrule())
                    .child(
                        scroll()
                            .test_id("inspector")
                            .h(term::rows(STACKED_ROWS))
                            .child(view().px(term::cells(1.0)).child(content)),
                    )
                    .into_element()
            } else {
                view()
                    .test_id("inspector")
                    .flex_none()
                    .px(term::cells(1.0))
                    .child(content)
                    .into_element()
            }
        }))
    })
    .into_element();
    view()
        .flex_1()
        .flex_col()
        .min_h(px(0.0))
        .child(body(app, "history", main))
        .child(below)
        .into_element()
}
