//! The Brief: "Ready to land?" as a chain (gate, independent review with its note quoted, your
//! verdict), then what changed, one row per file that opens its patch in place; and the side
//! facts (what the task asked for, the run's facts, the session and the task's Actions menu),
//! beside the candidate on a wide desktop, under the Brief elsewhere.

use nsh_core::EllipsizeMode;
use nsh_core::prelude::*;

use crate::model::review::{Brief, BriefSide, ChangedFile, ReadyLink, ReviewQuote};
use crate::model::work::{Record, VERB_MENU_LABEL};
use crate::model::{Form, Hue};
use crate::screens::markdown::{markdown, provide_styles};
use crate::screens::now::verb_menu;
use crate::screens::plans::menu_trigger;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::hint::{hinted, tip};
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::table::kv;
use crate::ui::kit::text::{dim, mono};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::{Ctx, act, button_id, command, diff, scope, shown_task};

/// A chain link's mark column and the line that joins it to the next (`.ready li`).
const MARK_COLUMN: f32 = 18.0;
const MARK_ROW: f32 = 20.0;
/// The reviewer's quoted note (`.ready blockquote { max-width: 68ch }`).
const QUOTE_MAX: f32 = 560.0;
/// A file's name before its directory takes the rest (`.fr .p { max-width: 340px }`).
const NAME_MAX: f32 = 340.0;
/// A file row (`.fr`), and the phone's least touch target.
const FILE_ROW: f32 = 36.0;
const TARGET: f32 = 44.0;
const RING: f32 = 2.0;

/// The Brief, the side facts after it when `side_below`.
pub(super) fn brief(cx: &Ctx, brief: &Brief, side_below: bool) -> AnyElement {
    let app = cx.app;
    view()
        .child(block(
            app,
            true,
            view()
                .child(caption(app, brief.readiness_heading, None, false))
                .child(chain(app, &brief.readiness))
                .into_element(),
        ))
        .children(
            brief
                .rounds
                .as_ref()
                .map(|rounds| block(app, false, super::rounds::rounds(cx, rounds))),
        )
        .child(block(app, false, changes(cx, brief)))
        .when(side_below, |page| {
            // A terminal draws the edge of a box this tall as a line of its own, so it takes
            // the joined rule alone; elsewhere the box's top edge is the rule.
            let below = if app.form == Form::Terminal {
                view().child(term::hrule())
            } else {
                view()
                    .mt(px(8.0))
                    .border_t(px(1.0))
                    .border_color(color(Token::Rule))
            };
            page.child(below.child(side(cx, &brief.side)))
        })
        .into_element()
}

/// A block of the Brief (`.blk`): 16 pt above (none after the first) and 20 pt either side on
/// desktop, 18 on a phone; a Cell in and a blank row after it in a terminal.
fn block(app: AppState, first: bool, content: AnyElement) -> AnyElement {
    match app.form {
        Form::Desktop => view()
            .px(px(20.0))
            .pt(px(if first { 16.0 } else { 4.0 }))
            .pb(px(16.0))
            .child(content)
            .into_element(),
        Form::Phone => view()
            .px(px(18.0))
            .pt(px(if first { 16.0 } else { 4.0 }))
            .pb(px(16.0))
            .child(content)
            .into_element(),
        Form::Terminal => view()
            .px(term::cells(1.0))
            .child(content)
            .child(term::blank())
            .into_element(),
    }
}

/// A block's or a side section's heading (`.blk > h3`, `.sec > h3`): the label semibold in
/// ink-3, its count after it, at the trailing edge in a side section; bold in a terminal.
pub(super) fn caption(
    app: AppState,
    label: &'static str,
    count: Option<String>,
    trailing: bool,
) -> AnyElement {
    let form = app.form;
    let row = view()
        .flex_row()
        .items_center()
        .role(AccessibilityRole::Header);
    if form == Form::Terminal {
        return row
            .child(
                text(label)
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink)),
            )
            .when(trailing, |row| row.child(view().flex_1()))
            .children(count.map(|count| {
                view()
                    .flex_row()
                    .child(term::space(1.0))
                    .child(term::quiet(count))
            }))
            .into_element();
    }
    let size = if form == Form::Phone { 13.0 } else { 12.0 };
    row.gap(px(8.0))
        .mb(px(10.0))
        .child(
            text(label)
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(size))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink3)),
        )
        .when(trailing, |row| row.child(view().flex_1()))
        .children(count.map(|count| {
            text(count)
                .number_of_lines(1u32)
                .font_family(FONT_MONO)
                .tabular_nums()
                .text_size(px(size - 0.5))
                .text_color(color(Token::Ink3))
        }))
        .into_element()
}

/// `.ready`: each link's mark in its hue on a rule-2 line down to the next, the title (its
/// hint the receipt), the candidate's SHA, a count of older results, the quoted note.
pub(super) fn chain(app: AppState, links: &[ReadyLink]) -> AnyElement {
    view()
        .role(AccessibilityRole::List)
        .children(
            links
                .iter()
                .enumerate()
                .map(|(at, link)| chain_link(app, link, at + 1 == links.len()))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

fn chain_link(app: AppState, link: &ReadyLink, last: bool) -> AnyElement {
    let form = app.form;
    let title = text(link.title)
        .number_of_lines(1u32)
        .font_weight(if form == Form::Desktop {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::BOLD
        })
        .text_color(color(Token::Ink))
        .when(form != Form::Terminal, |t| {
            t.font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone { 15.0 } else { 13.5 }))
        });
    let mut line: Vec<AnyElement> = vec![hinted(app, title, link.hint.clone())];
    line.extend(
        link.sha7
            .clone()
            .map(|sha| mono(app, sha).text_color(color(Token::Ink3)).into_element()),
    );
    let quote = link.quote.as_ref().map(|quote| quoted(app, quote));
    if form == Form::Terminal {
        return view()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .child(mark(app, link.mark, link.hue))
                    .children(line.into_iter().map(|part| {
                        view()
                            .flex_row()
                            .flex_shrink_0()
                            .child(term::space(1.0))
                            .child(part)
                            .into_element()
                    })),
            )
            .children(quote)
            .into_element();
    }
    view()
        .relative()
        .flex_row()
        .gap(px(10.0))
        .when(!last, |item| {
            item.pb(px(16.0)).child(
                view()
                    .absolute()
                    .left(px(MARK_COLUMN / 2.0 - 0.5))
                    .top(px(MARK_ROW))
                    .bottom(px(2.0))
                    .w(px(1.0))
                    .bg(color(Token::Rule2)),
            )
        })
        .child(
            view()
                .w(px(MARK_COLUMN))
                .h(px(MARK_ROW))
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .child(mark(app, link.mark, link.hue)),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.0))
                        .min_h(px(MARK_ROW))
                        .children(line),
                )
                .children(quote),
        )
        .into_element()
}

/// The reviewer's note in a quiet bordered block, who said it above it (`.ready blockquote`);
/// a terminal sets it two Cells in, who quiet over the words.
fn quoted(app: AppState, quote: &ReviewQuote) -> AnyElement {
    let form = app.form;
    if form == Form::Terminal {
        return view()
            .pl(term::cells(2.0))
            .child(term::quiet(quote.by.clone()))
            .child(text(quote.text.clone()).text_color(color(Token::Ink)))
            .into_element();
    }
    let (by, size) = if form == Form::Phone {
        (13.0, 14.0)
    } else {
        (12.0, 13.0)
    };
    view()
        .mt(px(8.0))
        .max_w(px(QUOTE_MAX))
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule2))
        .bg(color(Token::Surface))
        .gap(px(2.0))
        .child(
            text(quote.by.clone())
                .font_family(FONT_TEXT)
                .text_size(px(by))
                .text_color(color(Token::Ink3)),
        )
        .child(
            text(quote.text.clone())
                .font_family(FONT_TEXT)
                .text_size(px(size))
                .line_height(px((size * 1.5).round()))
                .text_color(color(Token::Ink)),
        )
        .into_element()
}

/// What changed: the heading with the number of files, then the files in a framed list
/// (`.fl`), or the model's line where none are known.
fn changes(cx: &Ctx, brief: &Brief) -> AnyElement {
    let app = cx.app;
    let count = (!brief.files.is_empty()).then(|| brief.files.len().to_string());
    let files = view()
        .when(app.form != Form::Terminal, |list| {
            list.rounded(px(8.0))
                .border_1()
                .border_color(color(Token::Rule))
                .overflow_hidden()
        })
        .children(
            brief
                .files
                .iter()
                .enumerate()
                .map(|(at, file)| changed_file(cx, file, at > 0))
                .collect::<Vec<_>>(),
        );
    view()
        .child(caption(app, brief.changes_heading, count, false))
        .children(
            brief
                .files_empty
                .clone()
                .map(|line| dim(app, line).into_element()),
        )
        .when(!brief.files.is_empty(), |block| block.child(files))
        .into_element()
}

/// A file's row and, while it is open, its patch under it (`.fbody`).
fn changed_file(cx: &Ctx, file: &ChangedFile, ruled: bool) -> AnyElement {
    let app = cx.app;
    let terminal = app.form == Form::Terminal;
    view()
        .when(ruled && !terminal, |item| {
            item.border_t(px(1.0)).border_color(color(Token::Rule))
        })
        .child(file_row(cx, file))
        .children(file.body.as_ref().map(|body| {
            let place = view();
            let place = if terminal {
                place.pl(term::cells(2.0))
            } else {
                place
                    .px(px(12.0))
                    .bg(color(Token::Bg))
                    .border_t(px(1.0))
                    .border_color(color(Token::Rule))
            };
            place.child(diff::file_body(cx, body)).into_element()
        }))
        .into_element()
}

/// `.fr`: the chevron, the file's name in mono, its directory in ink-3 cut from the left so
/// the part nearest the name stays, then `+n −m`; pressing it opens or
/// closes the patch in place (`btn-expand-<slug>`, `btn-collapse-<slug>`). The full path is
/// its name for screen readers and its hint.
fn file_row(cx: &Ctx, file: &ChangedFile) -> AnyElement {
    let app = cx.app;
    let form = app.form;
    let expanded = file.expanded;
    let press = act(cx, file.toggle.clone());
    let row = view()
        .flex_row()
        .items_center()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(format!("{}, {} {}", file.hint, file.added, file.deleted))
        .accessibility_state(move || AccessibilityState {
            expanded: Some(expanded),
            ..AccessibilityState::default()
        })
        .test_id(&button_id(&file.toggle))
        .on_press(move || press());
    let directory = text(file.directory.clone())
        .flex_1()
        .min_w(px(0.0))
        .truncate()
        .text_overflow(EllipsizeMode::Head)
        .text_color(color(Token::Ink3));
    let counts =
        |words: &String, hue: Hue| text(words.clone()).flex_shrink_0().text_color(hue_ink(hue));
    let row = if form == Form::Terminal {
        row.h(term::rows(1.0))
            .child(term::quiet(if expanded { "▾" } else { "▸" }))
            .child(term::space(1.0))
            .child(
                text(file.basename.clone())
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(color(Token::Ink)),
            )
            .child(term::space(1.0))
            .child(directory)
            .child(term::space(1.0))
            .child(counts(&file.added, Hue::Ok))
            .child(term::space(1.0))
            .child(counts(&file.deleted, Hue::Fail))
    } else {
        let size = if form == Form::Phone { 13.0 } else { 12.5 };
        let mono_counts = |words: &String, hue: Hue| {
            counts(words, hue)
                .font_family(FONT_MONO)
                .tabular_nums()
                .text_size(px(size - 0.5))
        };
        row.gap(px(10.0))
            .px(px(12.0))
            .when(form == Form::Desktop, |row| row.h(px(FILE_ROW)))
            .when(form == Form::Phone, |row| row.min_h(px(TARGET)))
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(-RING))
            })
            .child(
                view()
                    .flex_shrink_0()
                    .accessibility_elements_hidden(true)
                    .rotate(deg(if expanded { 90.0 } else { 0.0 }))
                    .child(icon(Icon::Chevron, 14.0, color(Token::Ink3))),
            )
            .child(
                text(file.basename.clone())
                    .max_w(px(NAME_MAX))
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .truncate()
                    .font_family(FONT_MONO)
                    .text_size(px(size))
                    .text_color(color(Token::Ink)),
            )
            .child(directory.font_family(FONT_TEXT).text_size(px(size - 0.5)))
            .child(
                view()
                    .flex_row()
                    .flex_shrink_0()
                    .gap(px(6.0))
                    .child(mono_counts(&file.added, Hue::Ok))
                    .child(mono_counts(&file.deleted, Hue::Fail)),
            )
    };
    tip(form, row, file.hint.clone().into()).into_element()
}

/// The side facts (`.aside`): what the task asked for, the run's facts, then Open session and
/// the task's Actions menu; each a section with a rule under it.
pub(super) fn side(cx: &Ctx, side: &BriefSide) -> AnyElement {
    let app = cx.app;
    let mut sections = Vec::with_capacity(3);
    if let Some(asked) = &side.asked_for {
        provide_styles(app);
        sections.push(
            view()
                .child(caption(app, asked.heading, None, true))
                .child(markdown(app, &asked.text, false))
                .into_element(),
        );
    }
    if !side.facts.is_empty() {
        sections.push(
            view()
                .role(AccessibilityRole::Group)
                .label(side.facts_heading)
                .child(kv(
                    app,
                    side.facts
                        .iter()
                        .map(|fact| {
                            (
                                fact.label.to_owned(),
                                mono(app, fact.value.clone()).into_element(),
                            )
                        })
                        .collect(),
                ))
                .into_element(),
        );
    }
    sections.push(
        view()
            .gap(px(if app.form == Form::Terminal { 0.0 } else { 8.0 }))
            .children(
                side.open_session
                    .as_ref()
                    .map(|open| view().flex_row().child(command(cx, open)).into_element()),
            )
            .child(actions(cx))
            .into_element(),
    );
    view()
        .children(sections.into_iter().map(|content| section(app, content)))
        .into_element()
}

/// One side section (`.sec`): 14 pt by 18 pt with a rule under it; a Cell in and a blank row
/// after it in a terminal.
fn section(app: AppState, content: AnyElement) -> AnyElement {
    match app.form {
        Form::Terminal => view()
            .px(term::cells(1.0))
            .child(content)
            .child(term::blank())
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .px(px(18.0))
            .py(px(14.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(content)
            .into_element(),
    }
}

/// The shown task's Actions menu: every engine operation, the same menu as Now's rows.
fn actions(cx: &Ctx) -> AnyElement {
    let focus = cx.focus.clone();
    let app = cx.app;
    let task = computed(move || {
        let selection = app.selection();
        let filter = scope(app);
        app.feed.with(|feed| {
            shown_task(feed, &selection, focus.as_ref(), &filter).map(|task| task.id.clone())
        })
    });
    dynamic(move || {
        task.get().map(|id| {
            let spoken = format!("{VERB_MENU_LABEL} for {id}");
            view()
                .flex_row()
                .child(verb_menu(
                    app,
                    Record::Task(id),
                    menu_trigger(app, VERB_MENU_LABEL, spoken, "btn-review-actions"),
                ))
                .into_element()
        })
    })
}
