//! The roster's lines, one per session of every role. Desktop (`.rh`, `.ar`): the column heads,
//! then one 40 pt line per session: the state's mark and word (a build's exit status in its
//! place) with the needs-you count, role, id, name (`untitled` muted when the host recorded
//! none), project, what it does, model and effort, cost, context share, quiet time and start;
//! the project, quiet time and start give way while the room runs short. Phone: the mark, name
//! and id; what it does on up to two lines; role, model, cost and quiet time. Terminal: the
//! desktop's columns as Cells, one row each, giving way one at a time as the grid narrows; words
//! too long for their column are cut at the character. Pressing a line opens its session page;
//! a run's line carries its Session menu (secondary press, Shift+F10 or the Menu key; a held
//! finger on the phone; `.` while the roster has the focus); a stopped build's line ends with
//! Remove…, which opens its Remove sheet (`btn-remove-<session>`).
//!
//! Rows are kept by session while the roster changes, each part following its own fact, so a
//! row the keyboard is on keeps the focus as its times and counts tick. Every word and number is
//! the model's; the row's hint (`doing_hint`) is its tooltip and description.

use nsh_core::EllipsizeMode;
use nsh_core::prelude::*;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::model::Form;
use crate::model::agents::{Action, Roster, RosterRow};
use crate::model::hue::Hue;
use crate::model::work::Record;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::hint::hint_on_live;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::more::with_menu;
use crate::ui::term::{self, cells};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, row_height};

use super::run_agents;
use super::session::{hue_words, state_mark};

/// A desktop line (`.ar` height) and the column heads' (`.rh`).
const LINE: f32 = 40.0;
const HEADS: f32 = 30.0;
/// The roster's sides, as the page head's, and the space between columns (`.ar` gap).
const SIDE: f32 = 18.0;
const GAP: f32 = 12.0;
const RING: f32 = 2.0;
/// Terminal column widths in Cells; the name and doing columns' least widths.
const ROLE_CELLS: f32 = 10.0;
const ID_CELLS: f32 = 9.0;
const NAME_CELLS: f32 = 16.0;
const PROJECT_CELLS: f32 = 14.0;
const DOING_CELLS: f32 = 16.0;
const AGENT_CELLS: f32 = 14.0;
const COST_CELLS: f32 = 7.0;
const CTX_CELLS: f32 = 4.0;
const QUIET_CELLS: f32 = 10.0;
const SINCE_CELLS: f32 = 6.0;
/// A terminal line's selection bar, and the Cell between columns.
const BAR_CELLS: f32 = 1.0;
const GAP_CELLS: f32 = 1.0;

/// Which optional columns show (tracked by the page from its measured room).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Columns {
    role: bool,
    project: bool,
    doing: bool,
    agent: bool,
    cost: bool,
    ctx: bool,
    quiet: bool,
    since: bool,
    /// The State column's Cells in a terminal: the longest state shown and a Cell either side.
    state: u16,
}

impl Columns {
    /// Desktop and phone: every column while `wide`, else without the project, quiet time and
    /// start.
    pub(super) fn points(wide: bool) -> Self {
        Self {
            role: true,
            project: wide,
            doing: true,
            agent: true,
            cost: true,
            ctx: true,
            quiet: wide,
            since: wide,
            state: 0,
        }
    }

    /// Terminal: the columns `room` Cells hold. State, id and name always; then, as long as
    /// they fit, the role, what the session does, its model, cost, context, project, quiet time
    /// and start, so a narrowing grid gives up the start first and the role last.
    pub(super) fn cells(room: f32, state: u16) -> Self {
        let mut used = BAR_CELLS + f32::from(state) + ID_CELLS + NAME_CELLS + 3.0 * GAP_CELLS;
        let mut fits = |cells_wide: f32| {
            let fits = used + GAP_CELLS + cells_wide <= room;
            if fits {
                used += GAP_CELLS + cells_wide;
            }
            fits
        };
        let role = fits(ROLE_CELLS);
        let doing = role && fits(DOING_CELLS);
        let agent = doing && fits(AGENT_CELLS);
        let cost = agent && fits(COST_CELLS);
        let ctx = cost && fits(CTX_CELLS);
        let project = ctx && fits(PROJECT_CELLS);
        let quiet = project && fits(QUIET_CELLS);
        let since = quiet && fits(SINCE_CELLS);
        Self {
            role,
            project,
            doing,
            agent,
            cost,
            ctx,
            quiet,
            since,
            state,
        }
    }
}

/// The roster's columns in the model's order (`Roster::columns`).
#[derive(Clone, Copy)]
enum Column {
    State,
    Role,
    Id,
    Name,
    Project,
    Doing,
    Agent,
    Cost,
    Ctx,
    Quiet,
    Since,
}

const ORDER: [Column; 11] = [
    Column::State,
    Column::Role,
    Column::Id,
    Column::Name,
    Column::Project,
    Column::Doing,
    Column::Agent,
    Column::Cost,
    Column::Ctx,
    Column::Quiet,
    Column::Since,
];

/// The state's words as the row shows them, with the needs-you count: `Needs you ▲ 2`.
pub(super) fn state_words(row: &RosterRow) -> String {
    match &row.needs {
        Some(needs) => format!("{} {needs}", row.state.word),
        None => row.state.word.clone(),
    }
}

/// The box of `column` in a line or the heads: its width (desktop points as the mock's grid,
/// terminal Cells), and hidden whole while `columns` leaves it out. In a terminal the name takes
/// the largest share of the room left, what the session does the next.
fn slot(form: Form, column: Column, columns: Memo<Columns>) -> View {
    let terminal = form == Form::Terminal;
    let fixed = |points: f32, cells_wide: f32| {
        view()
            .flex_row()
            .items_center()
            .flex_shrink_0()
            .min_w(px(0.0))
            .overflow_hidden()
            .w(if terminal {
                cells(cells_wide)
            } else {
                px(points)
            })
    };
    let optional = |slot: View, show: fn(Columns) -> bool| {
        slot.style_when(move || !show(columns.get()), |s| s.hidden())
            .accessibility_elements_hidden(move || !show(columns.get()))
    };
    let growing = |grow: f32, points: f32, cells_wide: f32| {
        view()
            .flex_row()
            .items_center()
            .flex_1()
            .flex_grow(grow)
            .overflow_hidden()
            .min_w(if terminal {
                cells(cells_wide)
            } else {
                px(points)
            })
    };
    match column {
        Column::State if terminal => {
            fixed(0.0, 0.0).w(move || cells(f32::from(columns.get().state)))
        }
        Column::State => fixed(128.0, 0.0),
        Column::Role => optional(fixed(68.0, ROLE_CELLS), |c| c.role),
        Column::Id => fixed(60.0, ID_CELLS),
        Column::Name => growing(if terminal { 2.0 } else { 1.0 }, 140.0, NAME_CELLS),
        Column::Project => optional(fixed(120.0, PROJECT_CELLS), |c| c.project),
        Column::Doing => optional(
            growing(if terminal { 1.0 } else { 1.6 }, 160.0, DOING_CELLS),
            |c| c.doing,
        ),
        Column::Agent => optional(fixed(120.0, AGENT_CELLS), |c| c.agent),
        Column::Cost => optional(fixed(52.0, COST_CELLS).justify_end(), |c| c.cost),
        Column::Ctx => optional(fixed(36.0, CTX_CELLS).justify_end(), |c| c.ctx),
        Column::Quiet => optional(fixed(84.0, QUIET_CELLS), |c| c.quiet),
        Column::Since => optional(fixed(44.0, SINCE_CELLS).justify_end(), |c| c.since),
    }
}

/// The column heads (`.rh`), in the model's words, over the lines while there are any. Hidden
/// from screen readers: each line's label names its facts. Desktop 30 pt, 11.5 pt ink-3 under a
/// rule; terminal one row a Cell in, where the lines keep their selection bar, over a joined
/// rule. None on a phone.
pub(super) fn heads(app: AppState, roster: Memo<Roster>, columns: Memo<Columns>) -> AnyElement {
    let form = app.form;
    if form == Form::Phone {
        return view().into_element();
    }
    let words = untracked(|| roster.with(|r| r.columns));
    let any = computed(move || roster.with(|r| !r.rows.is_empty()));
    let captions = ORDER.into_iter().zip(words).map(|(column, words)| {
        let caption = text(words)
            .number_of_lines(1u32)
            .truncate()
            .min_w(px(0.0))
            .text_color(color(Token::Ink3));
        let caption = if form == Form::Terminal {
            caption
        } else {
            caption.font_family(FONT_TEXT).text_size(px(11.5))
        };
        slot(form, column, columns).child(caption).into_element()
    });
    let row = view().flex_row().items_center().flex_shrink_0();
    let row = match form {
        Form::Terminal => view()
            .flex_shrink_0()
            .child(
                row.h(term::rows(1.0))
                    .gap(px(8.0))
                    .child(term::space(1.0))
                    .children(captions),
            )
            .child(term::hrule()),
        Form::Desktop | Form::Phone => row
            .h(px(HEADS))
            .px(px(SIDE))
            .gap(px(GAP))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .children(captions),
    };
    row.accessibility_elements_hidden(true)
        .style_when(move || !any.get(), |s| s.hidden())
        .into_element()
}

/// One line per session, kept by session id, in the destination's keyboard list
/// (`AppState::list_keys`), so j, k and the arrows move the selection while a line has the
/// focus. A run's line carries its Session menu.
pub(super) fn lines(app: AppState, roster: Memo<Roster>, columns: Memo<Columns>) -> AnyElement {
    let rows = each(
        move || roster.with(|r| r.rows.clone()),
        |row: &RosterRow| row.id.clone(),
        move |row, _| {
            let drawn = match app.form {
                Form::Phone => phone_line(app, row),
                Form::Desktop | Form::Terminal => line(app, row, columns),
            };
            match untracked(|| row.with(|r| r.task.is_some().then(|| r.session.clone()))) {
                Some(session) => with_menu(app, Record::Session(session), drawn),
                None => drawn,
            }
        },
    );
    view()
        .element_ref(&app.list_keys)
        .role(AccessibilityRole::List)
        .label(roster.with_untracked(|r| r.title))
        .child(rows)
        .into_element()
}

/// `pick` of the row, following it.
fn fact(row: ReadSignal<RosterRow>, pick: fn(&RosterRow) -> &str) -> impl Fn() -> String + 'static {
    move || row.with(|r| pick(r).to_owned())
}

/// What a screen reader names a row by: its state, role, id and name; its hint follows as the
/// description.
fn spoken(row: &RosterRow) -> String {
    [
        state_words(row).as_str(),
        row.role.as_str(),
        row.shown_id.as_str(),
        row.name.as_str(),
    ]
    .join(", ")
}

/// Quiet one-line words in a cell: 12 pt ink-3, mono for ids, times and money; in a terminal
/// [`cut`] to its column.
fn quiet(form: Form, words: impl Fn() -> String + 'static, mono: bool) -> AnyElement {
    if form == Form::Terminal {
        return cut(words, Token::Ink3);
    }
    text(words)
        .number_of_lines(1u32)
        .truncate()
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(color(Token::Ink3))
        .font_family(if mono { FONT_MONO } else { FONT_TEXT })
        .when(mono, |t| t.tabular_nums())
        .text_size(px(12.0))
        .into_element()
}

/// Terminal words that fill their column and, when they do not fit, are cut at the character
/// with a trailing ellipsis. NSH's one-line clamp wraps at a word before it cuts, which left a
/// name at its first word with room to spare, so the box measures its Cells and the words are
/// cut to them, then drawn unwrapped and clipped at the box's edge.
fn cut(words: impl Fn() -> String + 'static, ink: Token) -> AnyElement {
    let room = signal(None::<usize>);
    let shown = move || {
        let words = words();
        match room.get() {
            Some(room) => fit(&words, room),
            None => words,
        }
    };
    view()
        .flex_1()
        .min_w(px(0.0))
        .overflow_hidden()
        .on_layout(move |frame: &Bounds<Points>| {
            let fits = (frame.size.width.0 / cells(1.0).0 + 0.01).floor().max(0.0) as usize;
            if room.get_untracked() != Some(fits) {
                room.set(Some(fits));
            }
        })
        .child(
            text(shown)
                .number_of_lines(1u32)
                .text_overflow(EllipsizeMode::Clip)
                .flex_shrink_0()
                .text_color(color(ink)),
        )
        .into_element()
}

/// `words` in at most `room` Cells: whole when they fit, else as many characters as leave a
/// Cell for the ellipsis after them.
fn fit(words: &str, room: usize) -> String {
    if words.width() <= room {
        return words.to_owned();
    }
    let mut kept = String::with_capacity(words.len());
    let mut used = 0;
    for glyph in words.chars() {
        let wide = glyph.width().unwrap_or(0);
        if used + wide + 1 > room {
            break;
        }
        kept.push(glyph);
        used += wide;
    }
    if room > 0 {
        kept.push('…');
    }
    kept
}

/// The state's mark and word in its hue, then the needs-you count in the owner's colour;
/// redrawn only when they change.
fn state_cell(app: AppState, row: ReadSignal<RosterRow>) -> AnyElement {
    let form = app.form;
    let face = computed(move || row.with(|r| (r.state.clone(), r.needs.clone())));
    dynamic(move || {
        face.with(|(state, needs)| {
            untracked(|| {
                let size = |t: Text| {
                    if form == Form::Terminal {
                        t
                    } else {
                        t.font_family(FONT_TEXT).text_size(px(12.5))
                    }
                };
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(if form == Form::Terminal { 8.0 } else { 5.0 }))
                    .min_w(px(0.0))
                    .children(state_mark(app, state.spinner, state.glyph, state.hue))
                    .child(
                        size(text(state.word.clone()))
                            .number_of_lines(1u32)
                            .truncate()
                            .min_w(px(0.0))
                            .text_color(hue_words(form, state.hue)),
                    )
                    .children(needs.clone().map(|needs| {
                        size(text(needs))
                            .number_of_lines(1u32)
                            .flex_shrink_0()
                            .font_weight(FontWeight::BOLD)
                            .text_color(hue_ink(Hue::You))
                    }))
                    .into_element()
            })
        })
    })
}

/// The pin before the quick chat's name (desktop and phone; a terminal shows no images and
/// keeps the quick chat first instead).
fn pin(app: AppState, row: ReadSignal<RosterRow>, size: f32) -> AnyElement {
    let pinned = computed(move || row.with(|r| r.pinned));
    if app.form == Form::Terminal {
        return view().into_element();
    }
    dynamic(move || {
        pinned
            .get()
            .then(|| icon(Icon::Pin, size, color(Token::Ink3)))
    })
}

/// The session's name, `untitled` muted when the host recorded none.
fn name(form: Form, row: ReadSignal<RosterRow>, size: f32) -> AnyElement {
    let named = computed(move || row.with(|r| r.named));
    if form == Form::Terminal {
        return dynamic(move || {
            let ink = if named.get() { Token::Ink } else { Token::Ink3 };
            untracked(|| cut(fact(row, |r| r.name.as_str()), ink))
        });
    }
    text(fact(row, |r| r.name.as_str()))
        .number_of_lines(1u32)
        .truncate()
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(move || color(if named.get() { Token::Ink } else { Token::Ink3 }))
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .font_weight(FontWeight::SEMIBOLD)
        .into_element()
}

/// Opens the row's session page, keeping the selection on it for the keyboard.
fn open(app: AppState, row: ReadSignal<RosterRow>) -> impl Fn() + 'static {
    let (session, action) = untracked(|| row.with(|r| (r.session.clone(), r.action.clone())));
    move || {
        run_agents(app, Action::SelectRoster(session.clone()));
        run_agents(app, action.clone());
    }
}

/// A desktop or terminal line: the columns, those `columns` leaves out hidden.
fn line(app: AppState, row: ReadSignal<RosterRow>, columns: Memo<Columns>) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let doing = if terminal {
        cut(fact(row, |r| r.doing.as_str()), Token::Ink2)
    } else {
        text(fact(row, |r| r.doing.as_str()))
            .truncate()
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .text_color(color(Token::Ink2))
            .font_family(FONT_TEXT)
            .text_size(px(13.0))
            .into_element()
    };
    let at = |column| slot(form, column, columns);
    let mut cells = vec![
        at(Column::State).child(state_cell(app, row)).into_element(),
        at(Column::Role)
            .child(quiet(form, fact(row, |r| r.role.as_str()), false))
            .into_element(),
        at(Column::Id)
            .child(quiet(form, fact(row, |r| r.shown_id.as_str()), true))
            .into_element(),
        at(Column::Name)
            .gap(px(4.0))
            .child(pin(app, row, 12.0))
            .child(name(form, row, 13.0))
            .into_element(),
        at(Column::Project)
            .child(quiet(form, fact(row, |r| r.project.as_str()), false))
            .into_element(),
        at(Column::Doing).child(doing).into_element(),
        at(Column::Agent)
            .child(quiet(form, fact(row, |r| r.agent.as_str()), true))
            .into_element(),
        at(Column::Cost)
            .child(quiet(form, fact(row, |r| r.cost.as_str()), true))
            .into_element(),
        at(Column::Ctx)
            .child(quiet(form, fact(row, |r| r.ctx.as_str()), true))
            .into_element(),
        at(Column::Quiet)
            .child(quiet(form, fact(row, |r| r.quiet.as_str()), false))
            .into_element(),
        at(Column::Since)
            .child(quiet(form, fact(row, |r| r.since.as_str()), true))
            .into_element(),
    ];
    cells.push(remove(app, row));
    shell(app, row, open(app, row), cells)
}

/// `Remove…` on a stopped build's line: its Remove sheet asks prune's dry run first.
fn remove(app: AppState, row: ReadSignal<RosterRow>) -> AnyElement {
    let action = computed(move || row.with(|r| r.remove.clone()));
    let session = untracked(|| row.with(|r| r.session.to_string()));
    dynamic(move || {
        let action = action.get()?;
        let test_id = format!("btn-remove-{session}");
        Some(untracked(|| {
            view()
                .flex_shrink_0()
                .child(button(
                    app,
                    "Remove…",
                    ButtonKind::Quiet,
                    test_id,
                    move || run_agents(app, action.clone()),
                ))
                .into_element()
        }))
    })
}

/// The phone's third line: role, model, cost and quiet time, the absent ones left out.
fn agent_line(row: &RosterRow) -> String {
    [&row.role, &row.agent, &row.cost, &row.quiet]
        .into_iter()
        .filter(|words| words.as_str() != "–")
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A phone row (`.ar` at 640 pt and under): the state, the pinned mark and the name in
/// semibold, the id at the trailing edge; what the session does on up to two lines; then its
/// role, model, cost and quiet time.
fn phone_line(app: AppState, row: ReadSignal<RosterRow>) -> AnyElement {
    let first = view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .child(state_cell(app, row))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(pin(app, row, 14.0))
                .child(name(app.form, row, 15.5)),
        )
        .child(
            text(fact(row, |r| r.shown_id.as_str()))
                .number_of_lines(1u32)
                .flex_shrink_0()
                .font_family(FONT_MONO)
                .text_size(px(13.0))
                .text_color(color(Token::Ink3)),
        );
    let doing = text(fact(row, |r| r.doing.as_str()))
        .number_of_lines(2u32)
        .font_family(FONT_TEXT)
        .text_size(px(14.0))
        .line_height(px(19.0))
        .text_color(color(Token::Ink2));
    let body = view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(4.0))
        .child(first)
        .child(doing)
        .child(quiet(app.form, move || row.with(agent_line), true))
        .child(remove(app, row))
        .into_element();
    shell(app, row, open(app, row), vec![body])
}

/// The pressable line around `content`: a button named by [`spoken`] with the row's hint as its
/// description and tooltip (status line in a terminal), selected while the model says so. A
/// line the selection moves to scrolls into the roster's view; one already in view stays put.
/// Desktop: 40 pt, 18 pt in, 12 pt between columns, a rule under it, hover fill, selected the
/// sel fill with a 1 pt ink bar at the leading edge. Phone: 10 by 18 pt padding, a rule under
/// it, a pressed fill. Terminal: one row led by `▌` on the selected line.
fn shell(
    app: AppState,
    row: ReadSignal<RosterRow>,
    on_press: impl Fn() + 'static,
    content: Vec<AnyElement>,
) -> AnyElement {
    let form = app.form;
    let test_id = untracked(|| row.with(|r| r.id.clone()));
    let selected = computed(move || row.with(|r| r.selected));
    let hint = computed(move || row.with(|r| r.doing_hint.clone()));
    let handle = element_ref();
    effect(move || {
        if selected.get() {
            after_answer(move || {
                handle.reveal(RevealOptions::default().vertical(ScrollAlignment::Nearest));
            });
        }
    });
    let line = view()
        .element_ref(&handle)
        .relative()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(move || row.with(spoken))
        .accessibility_description(move || hint.get())
        .accessibility_state(move || AccessibilityState {
            selected: selected.get(),
            ..AccessibilityState::default()
        })
        .test_id(&test_id)
        .on_press(move || on_press());
    // The list's Enter opens the selected session only while its own row has the focus.
    let (focused, blurred) = (test_id.clone(), test_id.clone());
    let focused_row = app.focused_row;
    let line = line
        .on_focus(move || focused_row.set(Some(focused.clone())))
        .on_blur(move || {
            focused_row.update(|row| {
                if row.as_deref() == Some(blurred.as_str()) {
                    *row = None;
                }
            })
        });
    let ring = |s: nsh_core::element::StyleRefinement| {
        s.outline(px(RING))
            .outline_color(color(Token::Focus))
            .outline_offset(px(-RING))
    };
    let line = match form {
        Form::Desktop => line
            .h(px(LINE))
            .px(px(SIDE))
            .gap(px(GAP))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .style_when(selected, |s| s.bg(color(Token::Sel)))
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(ring)
            .child(
                view()
                    .absolute()
                    .left(px(0.0))
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .w(px(1.0))
                    .bg(color(Token::Ink))
                    .opacity(move || if selected.get() { 1.0_f32 } else { 0.0_f32 }),
            ),
        Form::Phone => line
            .py(px(10.0))
            .px(px(18.0))
            .gap(px(10.0))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .active(|s| s.bg(color(Token::Hover)))
            .focus_visible(ring),
        Form::Terminal => line.h(px(row_height(form))).gap(px(8.0)).child(
            view()
                .w(cells(1.0))
                .flex_shrink_0()
                .accessibility_elements_hidden(true)
                .child(
                    text(move || if selected.get() { "▌" } else { " " })
                        .text_color(color(Token::Ink)),
                ),
        ),
    };
    hint_on_live(app, line.children(content), move || hint.get().into()).into_element()
}
