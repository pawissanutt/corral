//! Activity › System's Lenses view: the project's lenses (kind, title, and whether each is
//! current, stale, incomplete or malformed) with Draft a lens under them, the selected lens
//! drawn with its failing and unchecked checks listed, and the inspector for the picked
//! element (its anchor, its committed line and its checks). Desktop and web draw the lens on a
//! canvas (`lens_canvas.rs`); the phone and a terminal list it: states then transitions, or
//! participants then numbered messages, so execution order survives any width. Every word is
//! `model::system::lens`'s.
//!
//! Test ids: "lens-list", "lens-<n>", "btn-lens-draft", "seg-lens-kind-<n>",
//! "field-lens-about", "btn-lens-draft-send", "btn-lens-draft-cancel", "lens-filed",
//! "btn-lens-filed-open", "lens-head", "btn-lens-repair", "lens-fail-<n>",
//! "lens-unchecked-<n>", "lens-line-<section>-<n>", "lens-inspector", and `lens_canvas.rs`'s.

use std::rc::Rc;

use corral_types::Op;
use corral_types::lenses::LensStatus;
use nsh_core::prelude::*;

use super::lens_canvas::{self, OnPick};
use super::lens_draft;
use crate::model::list::inspector_width;
use crate::model::system::Action;
use crate::model::system::lens::{self, Diagram, HINT, LEAD, NONE, REPAIR_LEAD, Row, Touch};
use crate::model::system::shown::Status;
use crate::model::text::span;
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::notice::notice;
use crate::ui::kit::page::{Width, body, columns, context, pane};
use crate::ui::kit::rows::list_row;
use crate::ui::kit::text::{h3, label_value, plain};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// What the list's read gives the page.
#[derive(Debug, Clone, PartialEq)]
struct Listed {
    status: Status,
    rows: Vec<Row>,
    notices: Vec<(Hue, String)>,
    /// The lens drawn: the one chosen, else the first.
    selected: Option<String>,
}

/// The drawn lens's read.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Read {
    Loading,
    /// corrald has no such lens at that commit: a change deleted it, or it was not yet written.
    Missing,
    Failed(String),
    /// A file that is not a lens: its failures.
    Malformed(Vec<String>),
    Ready(Box<Diagram>, bool),
}

fn status_of(cache: Option<&crate::model::daemon::DetailCache>) -> Status {
    match cache {
        None => Status::Loading,
        Some(cache) => match (cache.body(), &cache.error) {
            (None, Some(error)) => Status::Failed(error.message.clone()),
            (None, None) => Status::Loading,
            (Some(_), Some(_)) => Status::Stale,
            (Some(_), None) if cache.stale => Status::Stale,
            (Some(_), None) => Status::Ready,
        },
    }
}

/// The Lenses view of `project`.
pub(super) fn page(app: AppState, project: String) -> AnyElement {
    let listed = {
        let project = project.clone();
        computed(move || {
            let chosen = app.system.with(|s| s.lens.clone());
            app.feed.with(|feed| {
                let list = lens::list_of(feed, &project);
                let rows = list.map(lens::rows).unwrap_or_default();
                let selected = chosen
                    .filter(|key| rows.iter().any(|r| &r.key == key))
                    .or_else(|| rows.first().map(|r| r.key.clone()));
                Listed {
                    status: status_of(feed.detail(&lens::list_request(&project))),
                    notices: lens::notices(list, feed.systems.get(project.as_str()))
                        .into_iter()
                        .map(|n| (n.hue, n.words))
                        .collect(),
                    rows,
                    selected,
                }
            })
        })
    };
    let target = {
        let project = project.clone();
        computed(move || {
            listed
                .with(|l| l.selected.clone())
                .map(|key| lens::lens_request(&project, &key, None))
        })
    };
    // Pinned while shown, so neither read is evicted from under the page.
    let kept = {
        let project = project.clone();
        computed(move || {
            let mut targets = vec![lens::list_request(&project)];
            targets.extend(target.get());
            targets
        })
    };
    crate::screens::task::keep::keep_details(app, kept, None);
    let read = computed(move || {
        let Some(target) = target.get() else {
            return Read::Loading;
        };
        app.feed
            .with(|feed| read_of(feed, &target, &Touch::default()))
    });
    let shown = computed(move || match read.get() {
        Read::Ready(d, _) => Some(*d),
        _ => None,
    });
    let picked = computed(move || app.system.with(|s| s.element.clone()));
    let on_pick: OnPick = Rc::new(move |name| app.system_act(Action::Element(name)));
    let form = app.form;
    let list = list_column(app, listed, project.clone());
    let main = view()
        .flex_col()
        .min_w(px(0.0))
        .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
        .when(form != Form::Terminal, |v| v.px(px(16.0)).py(px(12.0)))
        .child(drawn_lens(app, project, read, shown, picked, on_pick))
        .into_element();
    let side = context(app, "inspector", inspector(app, shown, picked));
    match form {
        Form::Phone => pane(
            app,
            "system-map",
            view().child(list).child(main).child(side).into_element(),
        ),
        Form::Terminal => {
            let room = term::page_columns(app);
            let side_w = inspector_width(room, true) * 12.0;
            columns(
                app,
                vec![
                    (
                        Width::Fill,
                        body(
                            app,
                            "system-map",
                            view().child(list).child(main).into_element(),
                        ),
                    ),
                    (Width::Fixed(side_w), side),
                ],
                0,
            )
        }
        Form::Desktop => columns(
            app,
            vec![
                (Width::Fixed(260.0), pane(app, "lens-list-pane", list)),
                (Width::Fill, pane(app, "system-map", main)),
                (Width::Fixed(inspector_width(1200.0, false)), side),
            ],
            1,
        ),
    }
}

/// A `system_lens` read as the page shows it, with a review's marks.
pub(crate) fn read_of(
    feed: &crate::model::daemon::Feed,
    target: &corral_protocol::detail::DetailRef,
    touch: &Touch,
) -> Read {
    let cache = feed.detail(target);
    let stale = matches!(status_of(cache), Status::Stale);
    match (
        lens::lens_of(feed, target),
        cache.and_then(|c| c.error.as_ref()),
    ) {
        (Some(served), _) => match (&served.lens, &served.error) {
            (_, Some(error)) => Read::Failed(error.clone()),
            // The app asks by file; corrald falls back to a lens whose ID equals the text, and
            // that is another lens, not this file at this commit.
            (Some(result), None) if !asks_for(target, &result.file) => Read::Missing,
            (Some(result), None) => {
                match lens::diagram(result, served.commit.as_deref(), &served.excerpts, touch) {
                    Some(d) => Read::Ready(Box::new(d), stale),
                    None => {
                        Read::Malformed(result.failures.iter().map(|f| f.message.clone()).collect())
                    }
                }
            }
            (None, None) => Read::Loading,
        },
        // A read that failed for any other reason says nothing about whether the lens exists.
        (None, Some(error)) if error.code == "not_found" => Read::Missing,
        (None, Some(error)) => Read::Failed(error.message.clone()),
        (None, None) => Read::Loading,
    }
}

fn asks_for(target: &corral_protocol::detail::DetailRef, file: &str) -> bool {
    match target {
        corral_protocol::detail::DetailRef::SystemLens { id, .. } => id == file,
        _ => true,
    }
}

pub(super) fn quiet(app: AppState, words: impl Into<SharedString>) -> Text {
    let t = text(words.into()).text_color(color(Token::Ink3));
    if app.form == Form::Terminal {
        t
    } else {
        t.font_family(FONT_TEXT).text_size(px(12.0))
    }
}

pub(super) fn hued(app: AppState, hue: Hue, words: impl Into<SharedString>) -> AnyElement {
    plain(app, words).text_color(hue_ink(hue)).into_element()
}

fn list_column(app: AppState, listed: Memo<Listed>, project: String) -> AnyElement {
    let form = app.form;
    let rows = dynamic(move || {
        let Listed {
            status,
            rows,
            notices,
            ..
        } = listed.get();
        untracked(|| {
            let mut said: Vec<AnyElement> = Vec::new();
            let ready = status == Status::Ready;
            match status {
                Status::Loading if rows.is_empty() => {
                    said.push(quiet(app, "Reading the lenses from corrald…").into_element())
                }
                Status::Failed(why) => said.push(hued(
                    app,
                    Hue::Fail,
                    format!("The lenses could not be read: {why}"),
                )),
                Status::Stale => said.push(hued(
                    app,
                    Hue::Held,
                    "Shown as last read; corrald has not confirmed it since.",
                )),
                _ => {}
            }
            said.extend(
                notices
                    .into_iter()
                    .map(|(hue, words)| notice(app, hue, &vec![span(words)], None)),
            );
            if rows.is_empty() && ready {
                said.push(quiet(app, NONE).into_element());
            }
            let count = rows.len();
            view()
                .test_id("lens-list")
                .gap(px(if form == Form::Terminal { 0.0 } else { 2.0 }))
                .child(h3(app, "Lenses", Some(count.to_string())))
                .children(said)
                .children(
                    rows.into_iter()
                        .enumerate()
                        .map(|(n, row)| lens_row(app, listed, n, row)),
                )
                .into_element()
        })
    })
    .into_element();
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .child(rows)
        .child(quiet(app, LEAD))
        .child(lens_draft::draft(app, project))
        .child(lens_draft::filed(app))
        .when(form == Form::Terminal, |v| v.child(term::blank()))
        .into_element()
}

fn lens_row(app: AppState, listed: Memo<Listed>, n: usize, row: Row) -> AnyElement {
    let key = row.key.clone();
    let selected = {
        let key = key.clone();
        move || listed.with(|l| l.selected.as_ref() == Some(&key))
    };
    let label = format!(
        "{}, {} lens, {}",
        row.title,
        row.kind_word(),
        lens::status_word(row.status)
    );
    let facts = format!("{} · {}", row.kind_word(), lens::status_word(row.status));
    list_row(
        app,
        label,
        format!("lens-{n}"),
        selected,
        move || app.system_act(Action::Lens(key.clone())),
        vec![
            view()
                .flex_shrink_0()
                .w(if app.form == Form::Terminal {
                    term::cells(2.0)
                } else {
                    px(16.0)
                })
                .child(
                    text(lens::status_glyph(row.status))
                        .text_color(hue_ink(lens::status_hue(row.status))),
                )
                .into_element(),
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(plain(app, row.title).number_of_lines(1u32))
                .child(quiet(app, facts).number_of_lines(1u32))
                .into_element(),
        ],
    )
}

/// The selected lens: its head, its verdict, the diagram, and its checks that failed or could
/// not run.
fn drawn_lens(
    app: AppState,
    project: String,
    read: Memo<Read>,
    shown: Memo<Option<Diagram>>,
    picked: Memo<Option<String>>,
    on_pick: OnPick,
) -> AnyElement {
    let form = app.form;
    let state = computed(move || match read.get() {
        Read::Loading => Some((Hue::Neutral, "Reading the lens…".to_owned())),
        Read::Missing => Some((Hue::Fail, lens::MISSING.to_owned())),
        Read::Failed(why) => Some((Hue::Fail, format!("The lens could not be read: {why}"))),
        Read::Malformed(why) => Some((
            Hue::Fail,
            format!("Malformed: the file is not a lens. {}", why.join(" ")),
        )),
        Read::Ready(_, true) => Some((
            Hue::Held,
            "Shown as last read; corrald has not confirmed it since.".into(),
        )),
        Read::Ready(_, false) => None,
    });
    let said = dynamic(move || {
        let (hue, words) = state.get()?;
        Some(untracked(|| hued(app, hue, words)))
    });
    let head = dynamic(move || {
        let d = shown.get()?;
        let project = project.clone();
        Some(untracked(|| head(app, d, project)))
    });
    let picture = match form {
        Form::Desktop => lens_canvas::diagram(app, shown, picked, on_pick.clone()),
        Form::Phone | Form::Terminal => listing(app, shown, picked, on_pick.clone()),
    };
    let checks = dynamic(move || {
        let d = shown.get()?;
        let on_pick = on_pick.clone();
        Some(untracked(|| checks(app, d, on_pick)))
    });
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
        .child(said)
        .child(head)
        .child(picture)
        .child(checks)
        .into_element()
}

fn head(app: AppState, d: Diagram, project: String) -> AnyElement {
    let form = app.form;
    let title = text(d.title.clone())
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink))
        .when(form != Form::Terminal, |t| t.text_size(px(15.0)));
    let status = text(format!(
        "{} {}",
        lens::status_glyph(d.status),
        lens::status_word(d.status)
    ))
    .text_color(hue_ink(lens::status_hue(d.status)));
    let repair = (d.status != LensStatus::Current).then(|| {
        // The engine resolves an exact file before an ID, so the file names this lens.
        let id = d.file.clone();
        let name = d.id.clone();
        let project = project.clone();
        view()
            .gap(px(4.0))
            .child(quiet(app, REPAIR_LEAD))
            .child(button(
                app,
                "Repair",
                ButtonKind::Primary,
                "btn-lens-repair",
                move || {
                    lens_draft::file(
                        app,
                        Op::LensRepair {
                            project: project.clone(),
                            id: id.clone(),
                            ready: false,
                        },
                        format!("repair lens {name}"),
                    )
                },
            ))
            .into_element()
    });
    view()
        .test_id("lens-head")
        .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(10.0))
                .child(title)
                .child(status),
        )
        .child(quiet(app, d.figures()))
        .child(
            quiet(app, d.provenance()).when(form != Form::Terminal, |t| t.font_family(FONT_MONO)),
        )
        .child(plain(app, d.verdict()))
        .children(
            d.mismatch()
                .into_iter()
                .map(|line| hued(app, Hue::Held, line)),
        )
        .children(repair)
        .into_element()
}

/// Checks that failed, then checks that could not run, each picking its element.
fn checks(app: AppState, d: Diagram, on_pick: OnPick) -> AnyElement {
    let section = |title: &'static str, id: &'static str, hue: Hue, issues: Vec<lens::Issue>| {
        (!issues.is_empty()).then(|| {
            let rows: Vec<AnyElement> = issues
                .into_iter()
                .enumerate()
                .map(|(n, issue)| {
                    let (element, on_pick) = (issue.element.clone(), on_pick.clone());
                    list_row(
                        app,
                        format!("{}: {}", issue.what, issue.message),
                        format!("{id}-{n}"),
                        || false,
                        move || on_pick(element.clone()),
                        vec![
                            view()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(plain(app, issue.what).text_color(hue_ink(hue)))
                                .child(quiet(app, issue.message))
                                .into_element(),
                        ],
                    )
                })
                .collect();
            view()
                .child(h3(app, title, Some(rows.len().to_string())))
                .children(rows)
                .into_element()
        })
    };
    view()
        .gap(px(6.0))
        .children(section("Failed checks", "lens-fail", Hue::Fail, d.failures))
        .children(section(
            "Not checked",
            "lens-unchecked",
            Hue::Neutral,
            d.not_checked,
        ))
        .into_element()
}

/// The phone's and a terminal's form: a section per kind of element, a row each.
pub(super) fn listing(
    app: AppState,
    shown: Memo<Option<Diagram>>,
    picked: Memo<Option<String>>,
    on_pick: OnPick,
) -> AnyElement {
    dynamic(move || {
        let d = shown.get()?;
        let on_pick = on_pick.clone();
        Some(untracked(|| {
            let sections = lens::listing(&d)
                .into_iter()
                .enumerate()
                .map(|(s, section)| {
                    let count = section.lines.len().to_string();
                    let rows = section.lines.into_iter().enumerate().map(|(n, line)| {
                        line_row(
                            app,
                            format!("lens-line-{s}-{n}"),
                            line,
                            picked,
                            on_pick.clone(),
                        )
                    });
                    view()
                        .child(h3(app, section.title, Some(count)))
                        .children(rows)
                        .into_element()
                });
            view()
                .test_id("lens-diagram")
                .children(sections)
                .into_element()
        }))
    })
    .into_element()
}

/// One element of the list form: its mark, then its words (a message numbered) with a
/// guard or a note under them.
fn line_row(
    app: AppState,
    test_id: String,
    line: lens::Line,
    picked: Memo<Option<String>>,
    on_pick: OnPick,
) -> AnyElement {
    let name = line.element.clone();
    let selected = {
        let name = name.clone();
        move || picked.with(|p| p.as_ref() == Some(&name))
    };
    let mark = line.mark;
    let main = match &line.lead {
        Some(lead) => format!("{lead} {}", line.main),
        None => line.main.clone(),
    };
    let spoken = format!("{main}{}", line.words);
    let glyph = text(line.glyphs.clone().unwrap_or_default())
        .text_color(hue_ink(mark.map_or(Hue::Neutral, |m| m.hue())));
    list_row(
        app,
        spoken,
        test_id,
        selected,
        move || on_pick(name.clone()),
        vec![
            view()
                .flex_shrink_0()
                .w(if app.form == Form::Terminal {
                    term::cells(3.0)
                } else {
                    px(24.0)
                })
                .child(glyph)
                .into_element(),
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(plain(app, main))
                .children(line.sub.map(|sub| quiet(app, sub)))
                .into_element(),
        ],
    )
}

/// The picked element: what it is, its anchor and line, and how each of its checks stands.
pub(crate) fn inspector(
    app: AppState,
    shown: Memo<Option<Diagram>>,
    picked: Memo<Option<String>>,
) -> AnyElement {
    let form = app.form;
    let inspected = computed(move || {
        let name = picked.get()?;
        shown.with(|d| d.as_ref().and_then(|d| lens::inspect(d, &name)))
    });
    dynamic(move || {
        let Some(i) = inspected.get() else {
            return untracked(|| quiet(app, HINT).into_element());
        };
        untracked(|| {
            view()
                .test_id("lens-inspector")
                .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
                .child(quiet(app, i.what))
                .child(
                    text(i.title)
                        .test_id("inspector-title")
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink))
                        .when(form != Form::Terminal, |t| {
                            t.font_family(FONT_MONO).text_size(px(15.0))
                        }),
                )
                .children(i.rows.into_iter().map(|(label, value)| {
                    label_value(app, label, plain(app, value).into_element())
                }))
                .child(h3(app, "Source", None))
                .child(source(
                    app,
                    i.source,
                    "lens-source",
                    lens::SOURCE_UNAVAILABLE,
                ))
                .child(h3(app, "Checks", None))
                .children(
                    i.checks
                        .into_iter()
                        .map(|(hue, words)| hued(app, hue, words)),
                )
                .into_element()
        })
    })
    .into_element()
}

/// Committed lines around an anchor, the anchored line marked, under the test id `id`, or why
/// there are none (`unavailable`). A lens element's Source and an Impact item's share it.
pub(crate) fn source(
    app: AppState,
    source: Option<lens::Source>,
    id: &'static str,
    unavailable: &'static str,
) -> AnyElement {
    let form = app.form;
    let Some(source) = source else {
        return view()
            .test_id(id)
            .child(quiet(app, unavailable))
            .into_element();
    };
    let width = source
        .lines
        .last()
        .map_or(1, |(n, _, _)| n.to_string().len());
    let lines = source.lines.into_iter().map(move |(n, words, anchored)| {
        let row = format!("{}{n:>width$}  {words}", if anchored { "▸" } else { " " });
        text(row)
            .number_of_lines(1u32)
            .text_color(color(if anchored { Token::Ink } else { Token::Ink3 }))
            .when(anchored, |t| t.font_weight(FontWeight::BOLD))
            .when(form != Form::Terminal, |t| {
                t.font_family(FONT_MONO).text_size(px(11.5))
            })
            .into_element()
    });
    view()
        .test_id(id)
        .child(quiet(app, source.heading))
        .children(lines)
        .into_element()
}
