//! Now's main column: corrald's word on a board it cannot keep current, the loop band (desktop
//! and terminal), the sections of owner moves and moving tasks, the Waiting line that opens its
//! queued, held and parked rows in place, then the Closed section. The rows are built again
//! only when what they are changes; selecting another row restyles the two concerned and keeps
//! every row, and its focus, in place. It is the destination's keyboard list
//! (`AppState::list_keys`), and a row the keyboard selects scrolls into view.

use nsh_core::prelude::*;

use crate::model::glance::Stat;
use crate::model::work::{self, NowPage, NowRow, NowSection, NowWaiting};
use crate::model::{Form, Hue, Id};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::chip::chip;
use crate::ui::kit::glance::{loop_band, stat_in};
use crate::ui::kit::hint::{hint_on, hinted};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::notice::{notice, skeleton};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::closed::closed_section;
use super::parts::{go_scoped, press, pressable, quiet};
use super::rows::{self, Fit, ask_row, more_row, task_row};

const RING: f32 = 2.0;

/// What the rows are, apart from which one is selected.
#[derive(Debug, Clone, PartialEq)]
struct Shape {
    loading_rows: usize,
    notice: Option<String>,
    sections: Vec<NowSection>,
    waiting: NowWaiting,
}

fn shape(page: &NowPage) -> Shape {
    let mut sections = page.sections.clone();
    for row in sections
        .iter_mut()
        .flat_map(|section| section.rows.iter_mut())
    {
        match row {
            NowRow::Ask(row) => row.selected = false,
            NowRow::Task(row) => row.selected = false,
        }
    }
    let mut waiting = page.waiting.clone();
    for row in waiting
        .groups
        .iter_mut()
        .flat_map(|group| group.rows.iter_mut())
    {
        row.selected = false;
    }
    Shape {
        loading_rows: page.loading_rows,
        notice: page.notice.clone(),
        sections,
        waiting,
    }
}

fn ask_selected(page: &NowPage, key: &Id) -> bool {
    page.sections
        .iter()
        .flat_map(|section| &section.rows)
        .any(|row| matches!(row, NowRow::Ask(ask) if ask.key == *key && ask.selected))
}

fn task_selected(page: &NowPage, id: &str) -> bool {
    let moving = page
        .sections
        .iter()
        .flat_map(|section| &section.rows)
        .any(|row| matches!(row, NowRow::Task(task) if task.line.id == id && task.selected));
    moving
        || page
            .waiting
            .groups
            .iter()
            .flat_map(|group| &group.rows)
            .any(|task| task.line.id == id && task.selected)
}

/// The desktop and terminal main column, without its scroll: the board's notice, the loop band
/// (a stage opens its destination in Now's scope), then [`list`].
pub(super) fn main_column(app: AppState, page: Memo<NowPage>) -> AnyElement {
    let form = app.form;
    let band = computed(move || page.with(|page| page.loop_band.clone()));
    let band = dynamic(move || {
        band.with(|band| {
            loop_band(app, band, move |stage| {
                if let Some(to) = stage.to {
                    go_scoped(app, page, to);
                }
            })
        })
    });
    let band = match form {
        Form::Terminal => view().px(term::cells(1.0)).child(band),
        Form::Desktop | Form::Phone => view().mx(px(20.0)).mt(px(14.0)).mb(px(2.0)).child(band),
    };
    view()
        .child(board_status(app, page))
        .child(band)
        .child(list(app, page))
        .into_element()
}

/// corrald's notice that the board is out of date or refreshing, with its Retry
/// (`btn-retry-board`), and the stale board's `as of` chip.
pub(crate) fn board_status(app: AppState, page: Memo<NowPage>) -> AnyElement {
    let status = computed(move || page.with(|page| page.status.clone()));
    dynamic(move || {
        status.with(|status| {
            let banner = status.banner.as_ref().map(|banner| {
                let retry = banner
                    .actions
                    .first()
                    .map(|button| pressable(app, button, false));
                notice(app, banner.hue, &banner.text, retry)
            });
            let stamp = status
                .as_of
                .clone()
                .map(|stamp| chip(app, stamp, Hue::Held));
            if banner.is_none() && stamp.is_none() {
                return view().into_element();
            }
            let frame = match app.form {
                Form::Terminal => view().px(term::cells(1.0)),
                Form::Desktop => view().px(px(20.0)).pt(px(12.0)).gap(px(8.0)),
                Form::Phone => view().px(px(18.0)).pt(px(8.0)).gap(px(8.0)),
            };
            frame
                .children(banner)
                .children(stamp.map(|stamp| view().flex_row().child(stamp)))
                .into_element()
        })
    })
}

/// The keyboard list on every form: the sections, Waiting and Closed. It measures its width
/// once, and its rows give way by it ([`rows::fit`]).
pub(super) fn list(app: AppState, page: Memo<NowPage>) -> AnyElement {
    let form = app.form;
    let width = signal(0.0_f32);
    let fit = computed(move || rows::fit(form, width.get()));
    let shape = computed(move || page.with(shape));
    view()
        .element_ref(&app.list_keys)
        .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        .role(AccessibilityRole::List)
        .label(work::NowPage::LIST_LABEL)
        .pb(px(if form == Form::Terminal { 0.0 } else { 14.0 }))
        .child(dynamic(move || {
            shape.with(|shape| body(app, shape, page, fit))
        }))
        .child(closed_section(app, page))
        .into_element()
}

fn body(app: AppState, shape: &Shape, page: Memo<NowPage>, fit: Memo<Fit>) -> AnyElement {
    if shape.loading_rows != 0 {
        return loading(app, shape.loading_rows);
    }
    let mut out: Vec<AnyElement> = Vec::new();
    if let Some(words) = &shape.notice {
        out.push(quiet(app, words.clone()));
    }
    for section in &shape.sections {
        out.push(section_head(
            app,
            &section.label,
            section.count.to_string(),
            section.hue,
            &section.hint,
        ));
        for row in &section.rows {
            out.push(match row {
                NowRow::Ask(row) => {
                    let key = row.key.clone();
                    let selected = computed(move || page.with(|page| ask_selected(page, &key)));
                    revealing(selected, ask_row(app, row, selected, fit))
                }
                NowRow::Task(row) => task(app, row, page, fit),
            });
        }
    }
    if shape.waiting.total > 0 {
        out.push(waiting(app, &shape.waiting, page, fit));
    }
    view().children(out).into_element()
}

fn task(app: AppState, row: &work::NowTaskRow, page: Memo<NowPage>, fit: Memo<Fit>) -> AnyElement {
    let id = row.line.id.clone();
    let selected = computed(move || page.with(|page| task_selected(page, &id)));
    revealing(selected, task_row(app, row, selected, fit))
}

/// The Waiting line: its counts after the label; a press opens its groups in place.
pub(crate) fn waiting(
    app: AppState,
    waiting: &NowWaiting,
    page: Memo<NowPage>,
    fit: Memo<Fit>,
) -> AnyElement {
    let toggle = waiting.toggle.clone();
    let head = disclosure_head(
        app,
        Disclosure {
            test_id: "btn-waiting",
            label: waiting.label,
            count: Some(waiting.total.to_string()),
            stats: &waiting.stats,
            hint: &waiting.hint,
            open: waiting.expanded,
        },
        move || press(app, toggle.clone()),
    );
    let mut out = vec![head];
    if waiting.expanded {
        for group in &waiting.groups {
            out.push(section_head(
                app,
                &group.label,
                group.count.to_string(),
                group.hue,
                "",
            ));
            out.extend(group.rows.iter().map(|row| task(app, row, page, fit)));
            out.extend(group.more.as_ref().map(|button| more_row(app, button)));
        }
    }
    view().children(out).into_element()
}

/// A row scrolled into view whenever it becomes the selected one, so a selection the keyboard
/// moves never leaves the viewport. A row already in view stays where it is.
pub(super) fn revealing(selected: Memo<bool>, row: AnyElement) -> AnyElement {
    let handle = element_ref();
    effect(move || {
        if selected.get() {
            after_answer(move || {
                handle.reveal(RevealOptions::default().vertical(ScrollAlignment::Nearest));
            });
        }
    });
    view().element_ref(&handle).child(row).into_element()
}

/// The ink of a heading in `hue`: the hue's own, full ink for a neutral one.
fn head_ink(hue: Hue) -> AdaptiveColor {
    match hue {
        Hue::Neutral => color(Token::Ink),
        hue => hue_ink(hue),
    }
}

/// The frame of a section heading (`.gh`): desktop 20 pt above and 6 below, 20 pt in, a
/// hairline under it; phone 18 pt above and in; terminal one row.
fn head_frame(form: Form) -> View {
    let row = view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(8.0));
    match form {
        Form::Desktop => row
            .pt(px(20.0))
            .pb(px(6.0))
            .px(px(20.0))
            .border_b_1()
            .border_b_color(color(Token::Rule)),
        Form::Phone => row.pt(px(18.0)).pb(px(6.0)).px(px(18.0)),
        Form::Terminal => row.h(px(16.0)),
    }
}

fn head_label(form: Form, label: &str, ink: AdaptiveColor) -> Text {
    let words = text(label.to_owned())
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(ink)
        .font_weight(if form == Form::Terminal {
            FontWeight::BOLD
        } else {
            FontWeight::SEMIBOLD
        });
    if form == Form::Terminal {
        words
    } else {
        words.font_family(FONT_TEXT).text_size(px(13.0))
    }
}

fn head_count(form: Form, count: String) -> Text {
    let figures = text(count)
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(color(Token::Ink3));
    if form == Form::Terminal {
        figures
    } else {
        figures
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(12.0))
    }
}

/// A section heading: the label in its hue (its sentence on hover, focus or the status line),
/// the count in mono; a terminal rules the rest of the row.
pub(crate) fn section_head(
    app: AppState,
    label: &str,
    count: String,
    hue: Hue,
    hint: &str,
) -> AnyElement {
    let form = app.form;
    head_frame(form)
        .role(AccessibilityRole::Header)
        .child(hinted(
            app,
            head_label(form, label, head_ink(hue)),
            hint.to_owned(),
        ))
        .child(head_count(form, count))
        .when(form == Form::Terminal, |row| {
            row.child(view().flex_1().min_w(px(0.0)).child(term::hrule()))
        })
        .into_element()
}

/// The line under the scope bar when owner moves wait in other projects: its words, pressing
/// it shows every project. test id `btn-scope-all`.
pub(super) fn scope_note(app: AppState, page: Memo<NowPage>) -> AnyElement {
    let note =
        computed(move || page.with(|page| page.scope_note.clone().zip(page.scope_action.clone())));
    dynamic(move || {
        note.with(|note| {
            note.clone().map(|(words, action)| {
                let form = app.form;
                let line = rows::words(form, words.clone(), 12.5, hue_ink(Hue::You));
                let link = view()
                    .flex_row()
                    .items_center()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .role(AccessibilityRole::Link)
                    .label(words)
                    .test_id("btn-scope-all")
                    .on_press(move || app.set_project_filter(action.screen, action.filter.clone()))
                    .focus_visible(|s| {
                        s.outline(px(RING))
                            .outline_color(color(Token::Focus))
                            .outline_offset(px(RING))
                    })
                    .child(line);
                match form {
                    Form::Desktop => view()
                        .flex_row()
                        .px(px(20.0))
                        .py(px(6.0))
                        .border_b_1()
                        .border_b_color(color(Token::Rule))
                        .child(link.hover(|s| s.opacity(0.8))),
                    Form::Phone => view()
                        .flex_row()
                        .px(px(18.0))
                        .py(px(8.0))
                        .child(link.min_h(px(32.0))),
                    Form::Terminal => view().flex_row().px(term::cells(1.0)).child(link),
                }
                .into_element()
            })
        })
    })
}

/// What a [`disclosure_head`] says: its test id, label, count, counts at the trailing edge,
/// hint, and whether it is open.
pub(super) struct Disclosure<'a> {
    pub test_id: &'a str,
    pub label: &'a str,
    pub count: Option<String>,
    pub stats: &'a [Stat],
    pub hint: &'a str,
    pub open: bool,
}

/// A heading that opens and folds what is under it (Waiting, Closed): a chevron that turns
/// down while open (a terminal paints its own ▸ / ▾ from the expanded state), the label in
/// quiet ink, the count, then its counts at the trailing edge. One button for screen readers,
/// saying whether it is expanded; its hint on hover, focus or the status line.
pub(super) fn disclosure_head(
    app: AppState,
    head: Disclosure<'_>,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let form = app.form;
    let Disclosure {
        test_id,
        label,
        count,
        stats,
        hint,
        open,
    } = head;
    let spoken = match &count {
        Some(count) => format!("{label} {count}"),
        None => label.to_owned(),
    };
    let ink = color(if form == Form::Terminal {
        Token::Ink
    } else {
        Token::Ink3
    });
    let chevron = (form != Form::Terminal).then(|| {
        view()
            .flex_shrink_0()
            .rotate(move || deg(if open { 90.0 } else { 0.0 }))
            .child(icon(Icon::Chevron, 12.0, color(Token::Ink3)))
    });
    let row = head_frame(form)
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(spoken)
        .accessibility_state(move || AccessibilityState {
            expanded: Some(open),
            ..AccessibilityState::default()
        })
        .test_id(test_id)
        .on_press(move || on_press())
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-RING))
        })
        .when(form == Form::Desktop, |row| {
            row.hover(|s| s.bg(color(Token::Hover)))
        })
        .when(form == Form::Phone, |row| {
            row.min_h(px(48.0)).active(|s| s.bg(color(Token::Hover)))
        })
        .children(chevron)
        .child(head_label(form, label, ink))
        .children(count.map(|count| head_count(form, count)))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .when(form == Form::Terminal, |rule| rule.child(term::hrule())),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_shrink_0()
                .gap(if form == Form::Terminal {
                    term::cells(2.0)
                } else {
                    px(12.0)
                })
                .children(
                    stats
                        .iter()
                        .map(|stat| stat_in(app, stat))
                        .collect::<Vec<_>>(),
                ),
        );
    hint_on(app, row, hint.to_owned()).into_element()
}

/// Placeholder rows while the board's first page loads, in the rows' own heights.
pub(crate) fn loading(app: AppState, count: usize) -> AnyElement {
    let form = app.form;
    let rows = (0..count).map(|index| {
        // Each title bar a different length, so the rows read as text.
        let title = 0.6 + ((index * 13) % 35) as f32 / 100.0;
        match form {
            Form::Phone => view()
                .px(px(18.0))
                .py(px(10.0))
                .child(skeleton(app, &[(0.7, 14.0), (0.4, 10.0)]))
                .into_element(),
            Form::Terminal => view()
                .px(term::cells(2.0))
                .child(skeleton(app, &[(title, 16.0)]))
                .into_element(),
            Form::Desktop => view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .h(px(38.0))
                .px(px(20.0))
                .border_b_1()
                .border_b_color(color(Token::Rule))
                .child(view().w(px(52.0)).child(skeleton(app, &[(1.0, 10.0)])))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(skeleton(app, &[(title, 10.0)])),
                )
                .child(view().w(px(150.0)).child(skeleton(app, &[(1.0, 10.0)])))
                .into_element(),
        }
    });
    view()
        .label(NowPage::LOADING)
        .children(rows.collect::<Vec<_>>())
        .into_element()
}
