//! The session's commands on the Agents tab: their group under the agents (`Commands · 1
//! running`, then one line per job: its mark, its name (the command's first line) and state with
//! its time over its last output line, and Peek, which opens its last lines in place), the
//! selected job's detail (the name with the id's first eight characters, its directory, its
//! chips, the last lines of output in the log well, its controls), and an opened job's output in
//! the tab's place: a head that stays put (‹ Agents, the command, whether it follows the newest
//! lines or is paused with what arrived since, Find, the controls), a stop's confirmation, the
//! find bar while a search is open, then the lines in the log pager's face with every match
//! marked, the earlier bytes a press away.
//!
//! Following: the output keeps to its newest line while it runs; scrolling up pauses it and the
//! lines that arrive are counted, not scrolled to; reaching the end again, the Follow control or
//! `f` resumes. Finding: `/` (desktop and terminal) or Find opens the bar; Enter and Shift+Enter
//! (`n` and `N` in a terminal) walk the matches, Esc leaves; Search earlier output reads the
//! bytes before the loaded ones, page by page, until a match or the start. Peek: the Peek control
//! or `p` on the selected job. The find bar, the keys and the following live in `job_find`.
//!
//! Nothing pressable is built again because the clock moved or output arrived: the job lines
//! are kept one view per job id, the detail and the output's head are mounted from what their
//! controls are made of, and elapsed times, output sizes, last lines, the follow count and the
//! output's lines update in place. test ids `job-<id>`, `btn-peek-<id>`, `session-job-detail`,
//! `session-job-output`, `btn-job-more`, `btn-job-follow`, `btn-job-find`, `btn-job-older`,
//! `btn-job-newest`, `btn-retry-job`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{
    Action, AgentsTab, FleetButton, JobDetail, JobOutputView, JobRowView, Opened,
};
use crate::model::text::span;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::marks::mark;
use crate::ui::kit::notice::notice;
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, Token, color};

use super::super::super::run;
use super::super::super::transcript::log_lines;
use super::super::hue_words;
use super::agents_detail::{asking, controls, words};
use super::{Tab, cell, pressable};

#[path = "job_find.rs"]
mod find;

use find::{END_SLACK, NEWEST, Reading, find_bar, hunt, keep_to_end, keys, view_key};

/// The log well the output lines sit in: sunk on desktop and phone, plain in a terminal.
fn well(app: AppState, content: AnyElement) -> AnyElement {
    match app.form {
        Form::Terminal => content,
        _ => view()
            .px(px(10.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .bg(color(Token::BgSunk))
            .border(px(1.0))
            .border_color(color(Token::Rule))
            .child(content)
            .into_element(),
    }
}

/// `running · 38s`: a state word with the clock's elapsed time, when it has one.
fn with_elapsed(state: &str, elapsed: Option<String>) -> String {
    match elapsed {
        Some(time) => format!("{state} · {time}"),
        None => state.to_owned(),
    }
}

/// The jobs' group: its heading and note, then one line per job, kept by job id. `p` peeks at
/// the selected job.
pub(super) fn list(app: AppState, tab: Tab) -> AnyElement {
    let form = app.form;
    if form != Form::Phone {
        let steady = tab.steady;
        view_key(app, "p", move || {
            let peek = steady.with_untracked(|t| {
                t.as_ref()?
                    .jobs
                    .as_ref()?
                    .rows
                    .iter()
                    .find(|row| row.selected)
                    .map(|row| row.peek_action.clone())
            });
            if let Some(peek) = peek {
                run(app, peek);
            }
        });
    }
    let listed = tab.part(|t| t.jobs.is_some());
    let head = tab.part(|t| {
        t.jobs
            .as_ref()
            .map(|jobs| (jobs.heading.clone(), jobs.note.clone(), jobs.retry.clone()))
    });
    let head = dynamic(move || {
        let (heading, note, retry) = head.get().flatten()?;
        Some(untracked(|| {
            let note = note.map(|(hue, said)| {
                let retry = retry.map(|action| {
                    button(
                        app,
                        "Retry",
                        ButtonKind::Quiet,
                        "btn-retry-jobs",
                        move || run(app, action.clone()),
                    )
                });
                notice(app, hue, &vec![span(said)], retry)
            });
            view()
                .child(h3(app, heading, None))
                .children(note)
                .into_element()
        }))
    });
    let rows = each(
        move || {
            tab.steady.with(|t| {
                t.as_ref()
                    .and_then(|t| t.jobs.as_ref())
                    .map_or_else(Vec::new, |jobs| jobs.rows.clone())
            })
        },
        |row: &JobRowView| row.job.clone(),
        move |row, _| job_line(app, tab, row),
    );
    view()
        .test_id("session-jobs")
        .role(AccessibilityRole::List)
        .label("Commands")
        .style_when(move || listed.get() != Some(true), |s| s.hidden())
        .child(head)
        .child(rows)
        .into_element()
}

/// One job's line, mounted once per job id: its pressable line over its last output line, Peek
/// beside it, and its last lines under it while peeked; its words follow the job in place.
fn job_line(app: AppState, tab: Tab, row: ReadSignal<JobRowView>) -> AnyElement {
    let form = app.form;
    let (test_id, job) = untracked(|| row.with(|r| (r.test_id.clone(), r.job.clone())));
    let look = computed(move || row.with(|r| (r.mark, r.hue)));
    let mark_of = dynamic(move || {
        let (glyph, hue) = look.get();
        untracked(|| mark(app, glyph, hue))
    });
    let timed = job.clone();
    let state = text(move || {
        let elapsed = tab.elapsed(&timed);
        row.with(|r| with_elapsed(&r.state, elapsed))
    })
    .number_of_lines(1u32)
    .flex_shrink_0()
    .text_color(move || hue_words(form, look.get().1))
    .when(form != Form::Terminal, |t| {
        t.font_family(FONT_MONO).text_size(px(12.0))
    });
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(mark_of)
        .child(
            cell(
                form,
                move || row.with(|r| r.command.clone()),
                Token::Ink,
                true,
            )
            .flex_1()
            .font_weight(FontWeight::SEMIBOLD),
        )
        .child(state);
    let has_last = computed(move || row.with(|r| r.last.is_some() && r.peek.is_none()));
    let last = dynamic(move || {
        has_last.get().then(|| {
            untracked(|| {
                view()
                    .pl(if form == Form::Terminal {
                        term::cells(2.0)
                    } else {
                        px(20.0)
                    })
                    .child(cell(
                        form,
                        move || row.with(|r| r.last.clone().unwrap_or_default()),
                        Token::Ink3,
                        true,
                    ))
                    .into_element()
            })
        })
    });
    let selected = computed(move || row.with(|r| r.selected));
    let line = pressable(
        app,
        &test_id,
        move || {
            let elapsed = tab.elapsed(&job);
            row.with(|r| {
                let state = with_elapsed(&r.state, elapsed);
                if r.selected {
                    format!("Command {}, {state}; press to follow its output", r.command)
                } else {
                    format!("Command {}, {state}", r.command)
                }
            })
        },
        selected,
        move || row.with_untracked(|r| r.action.clone()),
        view().child(head).child(last),
    );
    let toggle = computed(move || row.with(|r| (r.peek.is_some(), r.selected)));
    let peek = dynamic(move || {
        let (open, selected) = toggle.get();
        untracked(|| {
            button_with(
                app,
                if open { "Hide" } else { "Peek" },
                ButtonKind::Quiet,
                ButtonOptions {
                    key_hint: selected.then_some("p"),
                    ..ButtonOptions::default()
                },
                format!("btn-peek-{}", row.with(|r| r.job.clone())),
                move || run(app, row.with_untracked(|r| r.peek_action.clone())),
            )
        })
    });
    let peeked = computed(move || row.with(|r| r.peek.clone()));
    let peeked = dynamic(move || {
        let peek = peeked.get()?;
        Some(untracked(|| {
            let note = peek
                .note
                .clone()
                .map(|said| words(form, said, color(Token::Ink3)).into_element());
            let lines = (!peek.lines.is_empty()).then(|| log_lines(app, &peek.lines));
            let body = view()
                .gap(px(4.0))
                .children(lines)
                .children(note)
                .into_element();
            view()
                .pl(if form == Form::Terminal {
                    term::cells(2.0)
                } else {
                    px(20.0)
                })
                .child(well(app, body))
                .into_element()
        }))
    });
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(view().flex_1().min_w(px(0.0)).child(line))
                .child(peek),
        )
        .child(peeked)
        .into_element()
}

/// `2.0 KB of output`, `⌁ watch /ready/ armed`, each in its tone.
fn chips(app: AppState, chips: &[(Hue, String)]) -> Vec<AnyElement> {
    chips
        .iter()
        .map(|(hue, said)| chip(app, cell(app.form, said.clone(), Token::Ink2, true), *hue))
        .collect()
}

/// One chip around `words`, in `hue`.
fn chip(app: AppState, words: Text, hue: Hue) -> AnyElement {
    let form = app.form;
    let words = words.text_color(hue_words(form, hue));
    match form {
        Form::Terminal => words.into_element(),
        _ => view()
            .px(px(8.0))
            .py(px(2.0))
            .rounded(px(999.0))
            .border(px(1.0))
            .border_color(color(Token::Rule2))
            .child(words)
            .into_element(),
    }
}

/// The selected job's detail, built again only when what its controls are made of changes;
/// its elapsed time, its output's size and its last lines follow in place.
pub(super) fn detail(app: AppState, tab: Tab) -> AnyElement {
    let frame = tab.part(|t| {
        t.job.clone().map(|detail| JobDetail {
            chips: Vec::new(),
            tail: Vec::new(),
            ..detail
        })
    });
    dynamic(move || {
        frame
            .get()
            .flatten()
            .map(|detail| untracked(|| detail_frame(app, tab, &detail)))
    })
    .into_element()
}

fn detail_frame(app: AppState, tab: Tab, detail: &JobDetail) -> AnyElement {
    let form = app.form;
    let live = tab.part(|t| {
        t.job
            .as_ref()
            .map(|job| (job.chips.clone(), job.tail.clone()))
    });
    let (hue, state) = detail.state.clone();
    let job = detail.job.clone();
    let state = chip(
        app,
        cell(
            form,
            move || with_elapsed(&state, tab.elapsed(&job)),
            Token::Ink2,
            true,
        ),
        hue,
    );
    let rest = dynamic(move || {
        let (shown, _) = live.get().flatten()?;
        Some(untracked(|| {
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(if form == Form::Terminal { 16.0 } else { 6.0 }))
                .children(chips(app, &shown))
                .into_element()
        }))
    });
    let tail = dynamic(move || {
        let (_, tail) = live.get().flatten()?;
        (!tail.is_empty()).then(|| untracked(|| well(app, log_lines(app, &tail))))
    });
    view()
        .test_id("session-job-detail")
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .child(
                    cell(form, detail.command.clone(), Token::Ink, true)
                        .flex_1()
                        .font_weight(FontWeight::BOLD),
                )
                .child(cell(form, detail.short.clone(), Token::Ink3, true).flex_shrink_0()),
        )
        .children(
            detail
                .dir
                .clone()
                .map(|dir| cell(form, format!("in {dir}"), Token::Ink3, true)),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(if form == Form::Terminal { 16.0 } else { 6.0 }))
                .child(state)
                .child(rest),
        )
        .child(tail)
        .child(controls(app, &detail.buttons))
        .into_element()
}

fn of(tab: &AgentsTab) -> Option<&JobOutputView> {
    match tab.opened.as_ref()? {
        Opened::Job(output) => Some(output),
        Opened::Conversation(_) => None,
    }
}

/// What the output's head and controls are built from: not its lines, its status words or
/// how much of it shows, which follow in place.
#[derive(Clone, PartialEq)]
struct Bar {
    short: String,
    back: Action,
    command: String,
    buttons: Vec<FleetButton>,
    /// It grows while it shows: Follow or Pause applies.
    live: bool,
}

/// The way to earlier and newer bytes and how the read stands: rebuilt only when those do.
#[derive(Clone, PartialEq)]
struct Paging {
    older: Option<Action>,
    newest: Option<Action>,
    read: Option<(Hue, String)>,
    retry: Option<Action>,
    empty: Option<String>,
}

/// The opened job's head, what it asks, its find bar and its output.
pub(super) fn output(app: AppState, tab: Tab) -> AnyElement {
    let form = app.form;
    let steady = tab.steady;
    let reading = Reading {
        view: computed(move || steady.with(|t| t.as_ref().and_then(of).cloned())),
        finding: signal(false),
        query: signal(String::new()),
        at: signal(NEWEST),
        hunting: signal(false),
        paused: signal(None),
        reveal: signal(0),
        scroller: element_ref(),
        hit: element_ref(),
        field: element_ref(),
    };
    keys(app, reading);
    keep_to_end(reading);
    hunt(app, reading);
    let more = signal(false);
    let shape = tab.part(|t| {
        of(t).map(|v| Bar {
            short: v.short.clone(),
            back: v.back.clone(),
            command: v.command.clone(),
            buttons: v.buttons.clone(),
            live: v.live,
        })
    });
    let paused = computed(move || reading.paused.get().is_some());
    let top = dynamic(move || {
        let open = more.get();
        let paused = paused.get();
        let shown = shape.get().flatten()?;
        Some(untracked(|| {
            bar(app, &shown, reading, paused, (open, more))
        }))
    });
    let asks = tab.part(|t| t.confirm.is_some() || t.picker.is_some());
    let asked = dynamic(move || {
        (asks.get() == Some(true)).then(|| {
            untracked(|| {
                let asked = view().flex_none().child(asking(app, tab));
                match form {
                    Form::Terminal => asked.px(term::cells(1.0)).into_element(),
                    Form::Desktop => asked.px(px(18.0)).py(px(8.0)).into_element(),
                    Form::Phone => asked.px(px(12.0)).py(px(6.0)).into_element(),
                }
            })
        })
    });
    let find = dynamic(move || {
        reading
            .finding
            .get()
            .then(|| untracked(|| find_bar(app, reading)))
    });
    let surface = scroll()
        .test_id("session-job-output")
        .flex_1()
        .min_h(px(0.0))
        .element_ref(&reading.scroller)
        .on_scroll(move |event: &ScrollEvent| {
            let below = event.content_size.height.0 - event.offset.y.0 - event.size.height.0;
            let at_end = below <= END_SLACK;
            match (at_end, reading.paused.get_untracked()) {
                (true, Some(_)) => reading.paused.set(None),
                (false, None) => reading.hold(),
                _ => {}
            }
        });
    let content = body(app, reading);
    let surface = match form {
        Form::Terminal => surface.child(view().px(term::cells(1.0)).child(content)),
        Form::Desktop => surface.child(view().px(px(18.0)).py(px(10.0)).child(content)),
        Form::Phone => surface.child(view().px(px(12.0)).py(px(8.0)).child(content)),
    };
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(top)
        .child(asked)
        .child(find)
        .child(surface)
        .into_element()
}

/// The head: the way back, the job, whether it follows its newest lines (or how its output
/// stands), Find and the controls. The output keeps the room: on the phone the head is one line
/// (the way back, how the output stands, Follow or Pause, Find, More) and in a terminal two,
/// with the rest behind More; on desktop all of it shows. How the output stands, the follow
/// count and how much of it shows update in place.
fn bar(
    app: AppState,
    shown: &Bar,
    reading: Reading,
    paused: bool,
    (open, more): (bool, Signal<bool>),
) -> AnyElement {
    let form = app.form;
    let compact = form != Form::Desktop;
    let back = shown.back.clone();
    let back = button(
        app,
        "‹ Agents",
        ButtonKind::Quiet,
        "btn-agents-back",
        move || run(app, back.clone()),
    );
    let toggle_more = compact.then(|| {
        button(
            app,
            if open { "Less" } else { "⋯ More" },
            ButtonKind::Quiet,
            "btn-job-more",
            move || more.update(|open| *open = !*open),
        )
    });
    let name = view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 16.0 } else { 10.0 }))
        .child(
            cell(form, shown.command.clone(), Token::Ink, true)
                .flex_1()
                .font_weight(FontWeight::BOLD),
        )
        .child(cell(form, shown.short.clone(), Token::Ink3, true).flex_shrink_0());
    let with_span = form != Form::Phone || open;
    let standing = computed(move || {
        let since = reading.paused.get();
        reading.view.with(|v| {
            let Some(v) = v.as_ref() else {
                return (Hue::Neutral, String::new());
            };
            let (hue, mut said) = match v.follow(since) {
                Some(line) => (line.hue, line.words),
                None => v.status.clone(),
            };
            if let Some(span) = v.span.as_ref().filter(|_| with_span) {
                said = format!("{said} · {span}");
            }
            (hue, said)
        })
    });
    let toggle = shown.live.then(|| {
        button_with(
            app,
            if paused { "Follow" } else { "Pause" },
            ButtonKind::Ghost,
            ButtonOptions {
                key_hint: Some("f"),
                ..ButtonOptions::default()
            },
            "btn-job-follow",
            move || reading.toggle(),
        )
    });
    let find = button_with(
        app,
        "Find",
        ButtonKind::Quiet,
        ButtonOptions {
            key_hint: Some("/"),
            ..ButtonOptions::default()
        },
        "btn-job-find",
        move || reading.find(),
    );
    let said = move || standing.with(|(_, said)| said.clone());
    let ink = move || hue_words(form, standing.with(|(hue, _)| *hue));
    let standing_words = if form == Form::Phone {
        cell(form, said, Token::Ink2, false)
            .text_color(ink)
            .flex_1()
    } else {
        text(said)
            .text_color(ink)
            .when(form != Form::Terminal, |t| {
                t.font_family(crate::ui::tokens::FONT_TEXT)
                    .text_size(px(12.5))
                    .line_height(px(18.0))
            })
            .flex_1()
            .min_w(px(0.0))
    };
    let gap = px(if form == Form::Terminal { 16.0 } else { 8.0 });
    let standing = view()
        .flex_row()
        .items_center()
        .gap(gap)
        .when(form != Form::Phone, |row| row.flex_wrap())
        .child(standing_words)
        .children(toggle)
        .child(find);
    let column = view().gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }));
    let column = match form {
        // One line: the way back, how the output stands, Follow or Pause, Find, More; the name
        // and the controls behind More.
        Form::Phone => column
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(gap)
                    .child(back)
                    .child(standing.flex_1().min_w(px(0.0)))
                    .children(toggle_more),
            )
            .children(open.then_some(name)),
        _ => column
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(gap)
                    .child(back)
                    .child(name.flex_1().min_w(px(0.0)))
                    .children(toggle_more),
            )
            .child(standing),
    };
    let column = column.children((!compact || open).then(|| controls(app, &shown.buttons)));
    match form {
        Form::Terminal => view()
            .flex_none()
            .child(column.px(term::cells(1.0)))
            .child(term::hrule())
            .into_element(),
        _ => column
            .flex_none()
            .px(px(if form == Form::Phone { 12.0 } else { 18.0 }))
            .py(px(if form == Form::Phone { 6.0 } else { 10.0 }))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .bg(color(Token::BgSunk))
            .into_element(),
    }
}

/// The way to earlier bytes and how the read stands, built again only when those change; the
/// lines with the search's matches marked (the current match's line carries the handle that
/// brings it into view), drawn again as output arrives; what an empty output says; then the way
/// back to the newest bytes.
fn body(app: AppState, reading: Reading) -> AnyElement {
    let form = app.form;
    let paging = computed(move || {
        reading.view.with(|v| {
            v.as_ref().map(|v| Paging {
                older: v.older.clone(),
                newest: v.newest.clone(),
                read: v.read.clone(),
                retry: v.retry.clone(),
                empty: v.empty.clone(),
            })
        })
    });
    let before = dynamic(move || {
        let paging = paging.get()?;
        Some(untracked(|| {
            let older = paging.older.map(|action| {
                button(
                    app,
                    "Earlier output",
                    ButtonKind::Quiet,
                    "btn-job-older",
                    move || run(app, action.clone()),
                )
            });
            let read = paging.read.map(|(hue, said)| {
                let retry = paging.retry.map(|action| {
                    button(
                        app,
                        "Retry",
                        ButtonKind::Quiet,
                        "btn-retry-job",
                        move || run(app, action.clone()),
                    )
                });
                notice(app, hue, &vec![span(said)], retry)
            });
            let empty = paging
                .empty
                .map(|said| words(form, said, color(Token::Ink3)).into_element());
            view()
                .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
                .children(older)
                .children(read)
                .children(empty)
                .into_element()
        }))
    });
    let after = dynamic(move || {
        let newest = paging.get()?.newest?;
        Some(untracked(|| {
            button(
                app,
                "Newest output",
                ButtonKind::Ghost,
                "btn-job-newest",
                move || run(app, newest.clone()),
            )
        }))
    });
    let lines = dynamic(move || {
        let query = reading.query.get();
        let at = reading.at.get();
        reading.view.with(|output| {
            output
                .as_ref()
                .map(|output| untracked(|| marked(app, output, &query, at, reading)))
        })
    });
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .child(before)
        .child(lines)
        .child(after)
        .into_element()
}

/// The output's lines, every match of the search marked and the current match's line holding
/// the handle that brings it into view; nothing pressable.
fn marked(
    app: AppState,
    output: &JobOutputView,
    query: &str,
    at: usize,
    reading: Reading,
) -> AnyElement {
    let found = output.search(query, at);
    let (shown, current) = match &found {
        Some(found) => (&found.lines, found.line),
        None => (&output.lines, None),
    };
    if shown.is_empty() {
        return view().into_element();
    }
    match current {
        Some(n) if n < shown.len() => view()
            .min_w(px(0.0))
            .child(log_lines(app, &shown[..n]))
            .child(
                view()
                    .element_ref(&reading.hit)
                    .child(log_lines(app, &shown[n..=n])),
            )
            .child(log_lines(app, &shown[n + 1..]))
            .into_element(),
        _ => log_lines(app, shown),
    }
}
