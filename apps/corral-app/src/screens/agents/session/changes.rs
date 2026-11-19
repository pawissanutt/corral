//! The Changes tab (`session::ChangesTab`): what the session changed, as its host serves it.
//! Over the content, fixed: the summary and where the read stands, the comparison switch (one
//! the host cannot serve muted, its reason said under the switch), the compared commits, the
//! repository and the totals, the repositories to read, then the path filter and the author
//! chips. Under them the file list and the opened file's diff (`changes_diff.rs`): beside each
//! other from 900 pt of tab width on desktop (the list 320 pt) and 100 Cells in a terminal
//! (36 Cells); narrower, the list above the diff, cut to five rows while a file is open; the
//! phone shows one at a time, the diff with a way back to the files. test ids
//! `session-changes`, `seg-compare-<slug>`, `seg-scope-<n>`, `seg-author-<id>`,
//! `field-changes-filter`, `change-file-<n>`.

#[path = "changes_diff.rs"]
mod diff;

use nsh_core::EllipsizeMode;
use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{
    Action, AuthorChip, Body, ChangesAction, ChangesTab, ComparisonChip, ComparisonLine,
    FILTER_FIELD, FileRow, ScopeChip, SessionPage,
};
use crate::model::text::span;
use crate::screens::now::list::revealing;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::hint::hint_on;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::notice::notice;
use crate::ui::kit::rows::{list_row, list_row_tall};
use crate::ui::kit::text::{dim, mono};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

use super::super::run;
use super::tabs::part;

/// Tab widths from which the list stands beside the diff: points, and Cells in a terminal.
const BESIDE: f32 = 900.0;
const BESIDE_CELLS: f32 = 100.0;
/// The list column beside the diff.
const LIST: f32 = 320.0;
const LIST_CELLS: f32 = 36.0;
/// The rows the list keeps over an opened diff when it stands above it.
const LIST_ROWS: f32 = 5.0;
const ROW: f32 = 30.0;
const TARGET: f32 = 44.0;
const RING: f32 = 2.0;

pub(super) fn tab(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let form = app.form;
    let shown = computed(move || {
        page.with(|p| match &p.body {
            Body::Changes(view) => Some(view.clone()),
            _ => None,
        })
    });
    let open =
        computed(move || shown.with(|tab| tab.as_ref().is_some_and(|t| t.selected.is_some())));
    let room = signal(None::<f32>);
    let beside = computed(move || {
        let width = room.get();
        match form {
            Form::Phone => false,
            Form::Desktop => width.is_some_and(|width| width >= BESIDE),
            Form::Terminal => width.is_some_and(|width| width >= term::cells(BESIDE_CELLS).0),
        }
    });
    let content = dynamic(move || {
        let (beside, open) = (beside.get(), open.get());
        untracked(|| body(app, shown, beside, open))
    });
    let root = view()
        .test_id("session-changes")
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .flex_col()
        .on_layout(move |frame: &Bounds<Points>| {
            let width = Some(frame.size.width.0);
            if room.get_untracked() != width {
                room.set(width);
            }
        });
    match form {
        // The phone scrolls the whole tab: its head, then the list or the diff.
        Form::Phone => root
            .child(
                scroll().flex_1().min_h(px(0.0)).child(
                    view()
                        .px(px(16.0))
                        .py(px(10.0))
                        .gap(px(12.0))
                        .child(head(app, shown))
                        .child(content),
                ),
            )
            .into_element(),
        Form::Desktop => root
            .child(
                view()
                    .px(px(18.0))
                    .pt(px(12.0))
                    .pb(px(10.0))
                    .child(head(app, shown)),
            )
            .child(content)
            .into_element(),
        Form::Terminal => root
            .child(view().px(term::cells(1.0)).child(head(app, shown)))
            .child(content)
            .into_element(),
    }
}

/// The list and the diff, laid out for the form and the room.
fn body(app: AppState, shown: Memo<Option<ChangesTab>>, beside: bool, open: bool) -> AnyElement {
    let form = app.form;
    if form == Form::Phone {
        return if open {
            diff::diff(app, shown)
        } else {
            files(app, shown)
        };
    }
    let rule = |side: bool| -> AnyElement {
        match (form, side) {
            (Form::Terminal, true) => term::vrule(),
            (Form::Terminal, false) => term::hrule(),
            (_, true) => view()
                .w(px(1.0))
                .flex_none()
                .bg(color(Token::Rule))
                .into_element(),
            (_, false) => view()
                .h(px(1.0))
                .flex_none()
                .bg(color(Token::Rule))
                .into_element(),
        }
    };
    let inset = |content: AnyElement| -> AnyElement {
        match form {
            Form::Terminal => view().px(term::cells(1.0)).child(content).into_element(),
            _ => view()
                .px(px(18.0))
                .py(px(10.0))
                .child(content)
                .into_element(),
        }
    };
    let list = scroll()
        .test_id("changes-files")
        .min_h(px(0.0))
        .child(inset(files(app, shown)));
    let patch = || {
        scroll()
            .test_id("changes-diff")
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .child(inset(diff::diff(app, shown)))
    };
    if beside && open {
        let list = match form {
            Form::Terminal => list.w(term::cells(LIST_CELLS)),
            _ => list.w(px(LIST)),
        };
        return view()
            .flex_1()
            .min_h(px(0.0))
            .flex_row()
            .border_t(px(if form == Form::Terminal { 0.0 } else { 1.0 }))
            .border_color(color(Token::Rule))
            .child(list.flex_none())
            .child(rule(true))
            .child(patch())
            .into_element();
    }
    if !open {
        return view()
            .flex_1()
            .min_h(px(0.0))
            .flex_col()
            .child(rule(false))
            .child(list.flex_1())
            .into_element();
    }
    let kept = match form {
        Form::Terminal => term::rows(LIST_ROWS),
        _ => px(LIST_ROWS * ROW + 20.0),
    };
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(rule(false))
        .child(list.flex_none().max_h(kept))
        .child(rule(false))
        .child(patch())
        .into_element()
}

/// What stands over the list: the summary and read, the switch, the comparison's identity, the
/// repositories, the host's error and bound, then the filter and the authors.
fn head(app: AppState, shown: Memo<Option<ChangesTab>>) -> AnyElement {
    let form = app.form;
    let summary = computed(move || shown.with(|tab| tab.as_ref().map(|tab| tab.part.clone())));
    let summary = dynamic(move || {
        summary.with(|body| {
            body.as_ref()
                .map(|body| untracked(|| part(app, body, "changes")))
        })
    });
    let switch = computed(move || {
        shown.with(|tab| {
            tab.as_ref()
                .map(|tab| tab.comparisons.clone())
                .unwrap_or_default()
        })
    });
    let switch = dynamic(move || {
        let chips = switch.get();
        untracked(|| comparisons(app, &chips))
    });
    let line =
        computed(move || shown.with(|tab| tab.as_ref().and_then(|tab| tab.comparison.clone())));
    let line = dynamic(move || line.get().map(|line| untracked(|| identity(app, &line))));
    let scopes = computed(move || {
        shown.with(|tab| {
            tab.as_ref()
                .map(|tab| tab.scopes.clone())
                .unwrap_or_default()
        })
    });
    let scopes = dynamic(move || {
        let chips = scopes.get();
        (!chips.is_empty()).then(|| untracked(|| repositories(app, &chips)))
    });
    let said = computed(move || {
        shown.with(|tab| {
            tab.as_ref()
                .map(|tab| (tab.error.clone(), tab.bound.clone()))
                .unwrap_or_default()
        })
    });
    let said = dynamic(move || {
        let (error, bound) = said.get();
        untracked(|| {
            view()
                .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
                .children(error.map(|error| notice(app, Hue::Held, &vec![span(error)], None)))
                .children(bound.map(|bound| notice(app, Hue::Neutral, &vec![span(bound)], None)))
                .into_element()
        })
    });
    let authors = computed(move || {
        shown.with(|tab| {
            tab.as_ref()
                .map(|tab| tab.authors.clone())
                .unwrap_or_default()
        })
    });
    let authors = dynamic(move || {
        let chips = authors.get();
        (!chips.is_empty()).then(|| untracked(|| author_chips(app, &chips)))
    });
    let tools = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(if form == Form::Terminal { 16.0 } else { 10.0 }))
        .child(filter(app, shown))
        .child(authors);
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .child(summary)
        .child(switch)
        .child(line)
        .child(scopes)
        .child(said)
        .child(tools)
        .into_element()
}

/// A pressed-or-not chip with words that can change: a comparison, a repository, an author.
/// Without an action it is muted and does nothing; `hint` says why.
fn chip(
    app: AppState,
    test_id: &str,
    words: String,
    count: Option<usize>,
    selected: bool,
    hint: Option<String>,
    action: Option<Action>,
) -> AnyElement {
    let form = app.form;
    let live = action.is_some();
    let ink = match (live, selected) {
        (false, _) => Token::Ink4,
        (true, true) => Token::Ink,
        (true, false) => Token::Ink2,
    };
    let label = text(words.clone())
        .number_of_lines(1u32)
        .text_color(color(ink))
        .font_weight(if selected {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        });
    let label = match form {
        Form::Terminal => label,
        _ => label
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 13.5 } else { 12.5 })),
    };
    let number = count.map(|n| {
        let n = text(n.to_string()).text_color(color(Token::Ink3));
        match form {
            Form::Terminal => n,
            _ => n.font_family(FONT_MONO).tabular_nums().text_size(px(11.0)),
        }
    });
    let said = match &hint {
        Some(why) => format!("{words} · {why}"),
        None => words,
    };
    let face = view()
        .test_id(test_id)
        .role(AccessibilityRole::Button)
        .label(said)
        .accessibility_state(AccessibilityState {
            selected,
            disabled: !live,
            ..AccessibilityState::default()
        })
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(if form == Form::Terminal { 8.0 } else { 5.0 }))
        .when(live, |face| face.cursor_pointer())
        .on_press(move || {
            if let Some(action) = action.clone() {
                run(app, action);
            }
        })
        .child(label)
        .children(number);
    let face = match form {
        Form::Terminal => face
            .px(term::cells(1.0))
            .when(selected, |face| face.bg(color(Token::Sel))),
        Form::Desktop | Form::Phone => face
            .h(px(if form == Form::Phone { 34.0 } else { 26.0 }))
            .px(px(10.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(color(if selected { Token::Rule2 } else { Token::Rule }))
            .bg(color(if selected { Token::Surface } else { Token::Bg }))
            .when(live && !selected, |face| {
                face.hover(|s| s.bg(color(Token::Hover)))
            })
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(1.0))
            }),
    };
    match hint {
        Some(hint) => hint_on(app, face, hint).into_element(),
        None => face.into_element(),
    }
}

fn chip_row(app: AppState, chips: Vec<AnyElement>) -> AnyElement {
    let form = app.form;
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(match form {
            Form::Terminal => 8.0,
            Form::Phone => 6.0,
            Form::Desktop => 4.0,
        }))
        .children(chips)
        .into_element()
}

/// The comparisons the host offers, the shown one pressed; then, for each it cannot serve now,
/// its name and the host's reason.
fn comparisons(app: AppState, chips: &[ComparisonChip]) -> AnyElement {
    let form = app.form;
    let row = chip_row(
        app,
        chips
            .iter()
            .map(|one| {
                chip(
                    app,
                    &one.id,
                    one.label.clone(),
                    None,
                    one.selected,
                    one.unavailable
                        .as_ref()
                        .map(|why| format!("unavailable: {why}")),
                    one.action.clone(),
                )
            })
            .collect(),
    );
    let reasons: Vec<AnyElement> = chips
        .iter()
        .filter_map(|one| {
            let why = one.unavailable.as_ref()?;
            Some(
                dim(app, format!("{}: unavailable · {why}", one.label))
                    .number_of_lines(2u32)
                    .into_element(),
            )
        })
        .collect();
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .child(row)
        .children(reasons)
        .into_element()
}

/// `aece0a19 → 7c41d0e2 · /work/repo · 14 files · +120 −30`.
fn identity(app: AppState, line: &ComparisonLine) -> AnyElement {
    let form = app.form;
    let mut parts = Vec::with_capacity(3);
    if let Some(range) = &line.range {
        parts.push(
            mono(app, range.clone())
                .text_color(color(Token::Ink2))
                .into_element(),
        );
    }
    parts.push(
        mono(app, line.root.clone())
            .text_color(color(Token::Ink3))
            .number_of_lines(1u32)
            .text_overflow(EllipsizeMode::Middle)
            .min_w(px(0.0))
            .flex_shrink(1.0)
            .into_element(),
    );
    parts.push(
        dim(app, line.totals.clone())
            .number_of_lines(1u32)
            .into_element(),
    );
    view()
        .flex_row()
        .items_center()
        .min_w(px(0.0))
        .gap(px(if form == Form::Terminal { 16.0 } else { 12.0 }))
        .children(parts)
        .into_element()
}

/// The repositories the host can read here, the read one pressed.
fn repositories(app: AppState, chips: &[ScopeChip]) -> AnyElement {
    let mut row = vec![dim(app, "Repository").into_element()];
    row.extend(chips.iter().map(|one| {
        chip(
            app,
            &one.id,
            one.label.clone(),
            None,
            one.selected,
            None,
            Some(one.action.clone()),
        )
    }));
    chip_row(app, row)
}

fn author_chips(app: AppState, chips: &[AuthorChip]) -> AnyElement {
    chip_row(
        app,
        chips
            .iter()
            .map(|one| {
                chip(
                    app,
                    &one.id,
                    one.label.clone(),
                    Some(one.count),
                    one.selected,
                    Some(one.hint.clone()),
                    Some(one.action.clone()),
                )
            })
            .collect(),
    )
}

/// The path filter; F3 brings the keyboard to it outside the web.
fn filter(app: AppState, shown: Memo<Option<ChangesTab>>) -> AnyElement {
    let form = app.form;
    let handle = element_ref();
    app.focus_when_asked(FILTER_FIELD, handle);
    let entry = input()
        .element_ref(&handle)
        .test_id(FILTER_FIELD)
        .label("Filter changed paths")
        .placeholder("Filter paths")
        .value(move || {
            shown.with(|tab| {
                tab.as_ref()
                    .map(|tab| tab.filter.clone())
                    .unwrap_or_default()
            })
        })
        .on_change(move |typed: &str| {
            let session = shown.with_untracked(|tab| tab.as_ref().map(|tab| tab.session.clone()));
            if let Some(session) = session {
                run(
                    app,
                    Action::Changes(ChangesAction::Filter {
                        session,
                        text: typed.to_owned(),
                    }),
                );
            }
        })
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || {
            for_appearance(color(if form == Form::Terminal {
                Token::Ink3
            } else {
                Token::Ink4
            }))
        })
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(AdaptiveColor::TRANSPARENT);
    match form {
        Form::Terminal => view()
            .flex_row()
            .items_center()
            .w(term::cells(28.0))
            .child(entry.flex_1().h(term::rows(1.0)))
            .into_element(),
        Form::Desktop | Form::Phone => {
            let (height, size) = if form == Form::Phone {
                (TARGET, 16.0)
            } else {
                (28.0, text_size(form))
            };
            view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .h(px(height))
                .px(px(8.0))
                .when(form == Form::Desktop, |field| field.w(px(240.0)))
                .when(form == Form::Phone, |field| field.flex_1().min_w(px(160.0)))
                .rounded(px(if form == Form::Phone { 10.0 } else { 6.0 }))
                .border_1()
                .border_color(color(Token::Rule2))
                .bg(color(Token::Surface))
                .focus_within(|s| s.border_color(color(Token::Focus)))
                .child(icon(Icon::Search, 14.0, color(Token::Ink3)))
                .child(
                    entry
                        .flex_1()
                        .h(px(height))
                        .font_family(FONT_TEXT)
                        .text_size(px(size)),
                )
                .into_element()
        }
    }
}

/// The file list, or what stands for it.
fn files(app: AppState, shown: Memo<Option<ChangesTab>>) -> AnyElement {
    // The rows redraw when the list changes, not when another one is selected.
    let rows = computed(move || {
        shown.with(|tab| {
            tab.as_ref()
                .map(|tab| {
                    tab.files
                        .iter()
                        .map(|row| FileRow {
                            selected: false,
                            ..row.clone()
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let empty = computed(move || shown.with(|tab| tab.as_ref().and_then(|tab| tab.empty.clone())));
    let list = dynamic(move || {
        let rows = rows.get();
        untracked(|| {
            view()
                .children(
                    rows.iter()
                        .map(|row| file(app, shown, row))
                        .collect::<Vec<_>>(),
                )
                .into_element()
        })
    });
    let empty = dynamic(move || {
        empty
            .get()
            .map(|words| untracked(|| dim(app, words).into_element()))
    });
    view().child(list).child(empty).into_element()
}

/// One file: its status, path (cut in the middle), authors, flag and counts. A flag or a rename
/// takes a second line on desktop and phone; a terminal keeps one row.
fn file(app: AppState, shown: Memo<Option<ChangesTab>>, row: &FileRow) -> AnyElement {
    let form = app.form;
    let path = row.path.clone();
    let selected = computed(move || {
        shown.with(|tab| {
            tab.as_ref()
                .is_some_and(|tab| tab.files.iter().any(|one| one.selected && one.path == path))
        })
    });
    let status = mono(app, row.status.letter)
        .text_color(if row.status.letter == "!" {
            hue_ink(Hue::Fail)
        } else {
            color(Token::Ink3)
        })
        .flex_shrink_0();
    let name = mono(app, row.path.clone())
        .number_of_lines(1u32)
        .text_overflow(EllipsizeMode::Middle)
        .min_w(px(0.0));
    let mut second = Vec::new();
    if let Some(from) = &row.from {
        second.push(
            dim(app, format!("from {from}"))
                .number_of_lines(1u32)
                .into_element(),
        );
    }
    if let Some(flag) = &row.flag
        && form != Form::Terminal
    {
        second.push(
            // The row's own `!` mark leads it already.
            dim(app, flag.trim_start_matches("! ").to_owned())
                .text_color(hue_ink(Hue::Held))
                .number_of_lines(1u32)
                .into_element(),
        );
    }
    let tall = !second.is_empty();
    let title = if tall {
        view()
            .flex_1()
            .min_w(px(0.0))
            .gap(px(1.0))
            .child(name)
            .child(
                view()
                    .flex_row()
                    .gap(px(8.0))
                    .min_w(px(0.0))
                    .children(second),
            )
            .into_element()
    } else {
        view().flex_1().min_w(px(0.0)).child(name).into_element()
    };
    let authors = (!row.authors.is_empty()).then(|| {
        dim(app, row.authors.join(" "))
            .number_of_lines(1u32)
            .flex_shrink_0()
            .into_element()
    });
    let mark = row.flag.as_ref().map(|flag| {
        let words = if form == Form::Terminal {
            flag.clone()
        } else {
            "!".to_owned()
        };
        text(words)
            .number_of_lines(1u32)
            .text_color(hue_ink(Hue::Held))
            .font_weight(FontWeight::BOLD)
            .flex_shrink_0()
            .into_element()
    });
    let counts = counts(app, row);
    let content = vec![status.into_element(), title]
        .into_iter()
        .chain(authors)
        .chain(mark)
        .chain(std::iter::once(counts))
        .collect::<Vec<_>>();
    let open = row.open.clone();
    let press = move || run(app, open.clone());
    let shape = if tall && form != Form::Terminal {
        list_row_tall(
            app,
            row.label.clone(),
            row.id.clone(),
            move || selected.get(),
            press,
            content,
        )
    } else {
        list_row(
            app,
            row.label.clone(),
            row.id.clone(),
            move || selected.get(),
            press,
            content,
        )
    };
    revealing(selected, shape)
}

/// `+12 −3` in the Ok and Fail inks, `bin` when git counts no lines.
fn counts(app: AppState, row: &FileRow) -> AnyElement {
    let figure =
        |words: String, ink: AdaptiveColor| mono(app, words).text_color(ink).flex_shrink_0();
    if row.binary {
        return figure("bin".into(), color(Token::Ink3)).into_element();
    }
    view()
        .flex_row()
        .flex_shrink_0()
        .gap(px(if app.form == Form::Terminal { 8.0 } else { 4.0 }))
        .child(figure(
            format!("+{}", row.added.unwrap_or(0)),
            hue_ink(Hue::Ok),
        ))
        .child(figure(
            format!("\u{2212}{}", row.removed.unwrap_or(0)),
            hue_ink(Hue::Fail),
        ))
        .into_element()
}
