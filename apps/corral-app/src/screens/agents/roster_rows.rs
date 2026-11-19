//! The roster's lines. Desktop (`.rh`, `.ar`): the column heads, then one 40 pt line per
//! session: state chip, task id, role (worker, reviewer or assistant), project, name, what it
//! does, its profile and model, how long it has been quiet and when it started, the last two and
//! the project only while the roster has the room and no panel is open. Phone (the mock's `.ar`
//! at 640 pt and under): chip, name and id; what it does; role, profile, model and quiet time.
//! Terminal: the desktop's columns as Cells, one row each, giving way one at a time as the grid
//! narrows; words too long for their column are cut at the character. A run's line carries its
//! Session menu (secondary press, Shift+F10 or the Menu key; a held finger on the phone; `.`
//! while the roster has the focus).
//!
//! Rows are kept by session while the roster changes, each part following its own fact, so a
//! row the keyboard is on keeps the focus as its times and counts tick. Every word and number is
//! the model's; the row's hint (`doing_hint`) is its tooltip, status line and description.

use nsh_core::EllipsizeMode;
use nsh_core::prelude::*;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::model::Form;
use crate::model::agents::{Action, Roster, RosterRow};
use crate::model::work::Record;
use crate::screens::now::with_menu;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::chip::chip;
use crate::ui::kit::hint::hint_on_live;
use crate::ui::term::{self, cells};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, row_height};

use super::{run, run_roster};

/// A desktop line (`.ar` height) and the column heads' (`.rh`).
const LINE: f32 = 40.0;
const HEADS: f32 = 30.0;
/// The roster's sides, as the page head's, and the space between columns (`.ar` gap).
const SIDE: f32 = 18.0;
const GAP: f32 = 12.0;
const RING: f32 = 2.0;
/// Terminal column widths in Cells; the name and doing columns' least widths.
const ID_CELLS: f32 = 8.0;
/// "reviewer", "assistant" and a Cell after.
const ROLE_CELLS: f32 = 10.0;
const PROJECT_CELLS: f32 = 11.0;
const AGENT_CELLS: f32 = 18.0;
const QUIET_CELLS: f32 = 11.0;
const SINCE_CELLS: f32 = 7.0;
const NAME_CELLS: (f32, f32) = (16.0, 20.0);
const DOING_CELLS: f32 = 16.0;
/// A terminal line's selection bar, and the Cell between columns.
const BAR_CELLS: f32 = 1.0;
const GAP_CELLS: f32 = 1.0;

/// Which optional columns show (tracked by the page from its measured room).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Columns {
    /// Worker, reviewer or assistant.
    pub(super) role: bool,
    pub(super) project: bool,
    pub(super) doing: bool,
    pub(super) agent: bool,
    pub(super) quiet: bool,
    pub(super) since: bool,
    /// A panel is open: the name and doing columns narrow (`.s-agents.has-sel`).
    pub(super) open: bool,
    /// The State column's Cells in a terminal: the longest chip shown and its Cell either side.
    pub(super) state: u16,
}

impl Columns {
    /// Desktop and phone: the role, what the session does and its agent always; the project,
    /// quiet time and start together while `wide`.
    pub(super) fn points(wide: bool, open: bool) -> Self {
        Self {
            role: true,
            project: wide,
            doing: true,
            agent: true,
            quiet: wide,
            since: wide,
            open,
            state: 0,
        }
    }

    /// Terminal: the columns `room` Cells hold. State, id and name always; then, as long as
    /// they fit, the role, what the session does, its agent, the project, quiet time and start,
    /// so a narrowing grid gives up the start first and the role last.
    pub(super) fn cells(room: f32, state: u16, open: bool) -> Self {
        let name = if open { NAME_CELLS.0 } else { NAME_CELLS.1 };
        let mut used = BAR_CELLS + f32::from(state) + ID_CELLS + name + 3.0 * GAP_CELLS;
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
        let project = agent && fits(PROJECT_CELLS);
        let quiet = project && fits(QUIET_CELLS);
        let since = quiet && fits(SINCE_CELLS);
        Self {
            role,
            project,
            doing,
            agent,
            quiet,
            since,
            open,
            state,
        }
    }
}

/// The roster's columns in the model's order (`Roster::columns`).
#[derive(Clone, Copy)]
enum Column {
    State,
    Id,
    Role,
    Project,
    Name,
    Doing,
    Agent,
    Quiet,
    Since,
}

const ORDER: [Column; 9] = [
    Column::State,
    Column::Id,
    Column::Role,
    Column::Project,
    Column::Name,
    Column::Doing,
    Column::Agent,
    Column::Quiet,
    Column::Since,
];

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
    let growing = |(points_grow, cells_grow): (f32, f32), open: (f32, f32), shut: (f32, f32)| {
        view()
            .flex_row()
            .items_center()
            .flex_1()
            .flex_grow(if terminal { cells_grow } else { points_grow })
            .overflow_hidden()
            .min_w(move || {
                let (points, cells_wide) = if columns.get().open { open } else { shut };
                if terminal {
                    cells(cells_wide)
                } else {
                    px(points)
                }
            })
    };
    match column {
        Column::State if terminal => {
            fixed(0.0, 0.0).w(move || cells(f32::from(columns.get().state)))
        }
        Column::State => fixed(92.0, 0.0),
        Column::Id => fixed(52.0, ID_CELLS),
        Column::Role => optional(fixed(68.0, ROLE_CELLS), |c| c.role),
        Column::Project => optional(fixed(64.0, PROJECT_CELLS), |c| c.project),
        Column::Name => growing((1.0, 2.0), (110.0, NAME_CELLS.0), (150.0, NAME_CELLS.1)),
        Column::Doing => optional(
            growing((1.6, 1.0), (150.0, DOING_CELLS), (220.0, DOING_CELLS)),
            |c| c.doing,
        ),
        Column::Agent => optional(fixed(140.0, AGENT_CELLS), |c| c.agent),
        Column::Quiet => optional(fixed(100.0, QUIET_CELLS), |c| c.quiet),
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
/// focus. Desktop and terminal lines select (the panel opens for them); a phone row opens the
/// session page. A run's line carries its Session menu.
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

/// What a screen reader names a row by: its state, id and name; its hint follows as the
/// description.
fn spoken(row: &RosterRow) -> String {
    [row.chip.as_str(), row.shown_id.as_str(), row.name.as_str()].join(", ")
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

/// The state chip, redrawn only when the state changes.
fn state_chip(app: AppState, row: ReadSignal<RosterRow>) -> AnyElement {
    let face = computed(move || row.with(|r| (r.chip.clone(), r.hue)));
    dynamic(move || face.with(|(label, hue)| untracked(|| chip(app, label.clone(), *hue))))
}

/// The run's role, profile and model, then how long it has been quiet: the phone's third line.
fn agent_line(row: &RosterRow) -> String {
    if row.quiet == "—" {
        format!("{} · {}", row.role, row.agent)
    } else {
        format!("{} · {} · {}", row.role, row.agent, row.quiet)
    }
}

/// The pin before the quick thread's name (desktop and phone; a terminal shows no images and
/// keeps the quick thread first instead).
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

/// A desktop or terminal line: the nine columns, those `columns` leaves out hidden.
fn line(app: AppState, row: ReadSignal<RosterRow>, columns: Memo<Columns>) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let action = untracked(|| row.with(|r| r.action.clone()));
    let name = if terminal {
        cut(fact(row, |r| r.name.as_str()), Token::Ink)
    } else {
        text(fact(row, |r| r.name.as_str()))
            .truncate()
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .text_color(color(Token::Ink))
            .font_family(FONT_TEXT)
            .text_size(px(13.0))
            .font_weight(FontWeight::SEMIBOLD)
            .into_element()
    };
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
    let cells = vec![
        at(Column::State).child(state_chip(app, row)).into_element(),
        at(Column::Id)
            .child(quiet(form, fact(row, |r| r.shown_id.as_str()), true))
            .into_element(),
        at(Column::Role)
            .child(quiet(form, fact(row, |r| r.role.as_str()), false))
            .into_element(),
        at(Column::Project)
            .child(quiet(form, fact(row, |r| r.project.as_str()), false))
            .into_element(),
        at(Column::Name)
            .gap(px(4.0))
            .child(pin(app, row, 12.0))
            .child(name)
            .into_element(),
        at(Column::Doing).child(doing).into_element(),
        at(Column::Agent)
            .child(quiet(form, fact(row, |r| r.agent.as_str()), true))
            .into_element(),
        at(Column::Quiet)
            .child(quiet(form, fact(row, |r| r.quiet.as_str()), false))
            .into_element(),
        at(Column::Since)
            .child(quiet(form, fact(row, |r| r.since.as_str()), true))
            .into_element(),
    ];
    // Where the panel takes the roster's place the pressed row goes out of sight: the focus
    // moves on to the panel it opened rather than being lost with the row.
    let press = move || {
        run_roster(app, action.clone());
        if untracked(|| super::roster::panel_covers(app)) {
            app.run(vec![crate::model::Effect::Focus(
                super::panel::OPEN_SESSION,
            )]);
        }
    };
    shell(app, row, press, cells)
}

/// A phone row (`.ar` at 640 pt and under): the chip, the pinned mark and the name in semibold,
/// the id at the trailing edge; what the session does, its one explanatory line, on up to two
/// lines; then its profile, model and quiet time. Pressing it opens the session page.
fn phone_line(app: AppState, row: ReadSignal<RosterRow>) -> AnyElement {
    let session = untracked(|| row.with(|r| r.session.clone()));
    let first = view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .child(state_chip(app, row))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(pin(app, row, 14.0))
                .child(
                    text(fact(row, |r| r.name.as_str()))
                        .number_of_lines(1u32)
                        .truncate()
                        .flex_shrink(1.0)
                        .min_w(px(0.0))
                        .font_family(FONT_TEXT)
                        .text_size(px(15.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(Token::Ink)),
                ),
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
        .into_element();
    shell(
        app,
        row,
        move || run(app, Action::OpenSession(session.clone())),
        vec![body],
    )
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
