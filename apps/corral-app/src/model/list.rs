//! What every list page shares: grouped sections of rows, the selection that walks them, and
//! the columns of a table row with the order they give way in as the list narrows. Tasks and
//! Sessions are such lists beside an inspector, as wide as the page allows
//! (`inspector_width`); Home's phone form is one without it.

use crate::model::Id;
use crate::model::status::Look;

/// One group of a list page: its header (mark, name, count) and the rows under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// `blocked`, `done`, `needs-you`: the header's test id is `section-<key>`.
    pub key: String,
    pub label: String,
    /// The status mark before the name; none for a section that is not one status.
    pub look: Option<Look>,
    /// How many records the group has, which a folded or paged section shows without its rows.
    pub count: usize,
    /// A quiet remark after the count: `waits on you`.
    pub note: Option<String>,
    /// For a section that folds (Done, Dropped, Gone): whether it is open now. A folded
    /// section lists no rows and the keys pass over it.
    pub fold: Option<bool>,
    /// The rows' keys in order; empty while folded.
    pub rows: Vec<Id>,
}

impl Section {
    /// Whether the header shows at all: a group with nothing in it is left out, a fold stays
    /// so it can be opened.
    pub fn shown(&self) -> bool {
        self.count > 0 || !self.rows.is_empty()
    }
}

/// A row's test id, which also names the row holding the keyboard focus: `task-<id>` for a
/// task, `agent-<id>` for a session.
pub fn row_id(live: &crate::model::Live) -> String {
    use crate::model::Live;
    match live {
        Live::Task(id) | Live::Plan(id) | Live::Review(id) => format!("task-{id}"),
        Live::Session(id) => format!("agent-{id}"),
    }
}

/// Every row the keys walk, top to bottom: the rows of the sections that are open.
pub fn order(sections: &[Section]) -> Vec<Id> {
    sections
        .iter()
        .filter(|section| section.fold != Some(false))
        .flat_map(|section| section.rows.iter().cloned())
        .collect()
}

/// The row to select on arrival and whenever the selected one left the list: the selection
/// itself while it is still listed, else the first row.
pub fn settled(order: &[Id], selected: Option<&Id>) -> Option<Id> {
    selected
        .filter(|id| order.contains(id))
        .or(order.first())
        .cloned()
}

/// What a list key does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListKey {
    /// `j` `k` `↓` `↑`: select this row.
    Select(Id),
    /// Enter: open the selected row's page.
    Open(Id),
    /// `i`: show or hide the inspector.
    Inspector,
    /// `.`: the selected row's More menu.
    More(Id),
}

/// What `key` does on a list of `order` with `selected`. A move stops at the ends; with
/// nothing selected it takes the first row.
pub fn key(key: &str, order: &[Id], selected: Option<&Id>) -> Option<ListKey> {
    let at = selected.and_then(|id| order.iter().position(|row| row == id));
    let moved = |step: isize| {
        let to = match at {
            Some(at) => at
                .saturating_add_signed(step)
                .min(order.len().checked_sub(1)?),
            None => 0,
        };
        order.get(to).cloned().map(ListKey::Select)
    };
    match key {
        "j" | "down" => moved(1),
        "k" | "up" => moved(-1),
        "i" => Some(ListKey::Inspector),
        "enter" => at.map(|at| ListKey::Open(order[at].clone())),
        "." => at.map(|at| ListKey::More(order[at].clone())),
        _ => None,
    }
}

/// What a press on a row does: with the inspector beside the list a press selects and a
/// second press on the selected row opens its page; without it a row opens on one press.
pub fn press(row: &Id, selected: Option<&Id>, inspector: bool) -> ListKey {
    if inspector && selected != Some(row) {
        ListKey::Select(row.clone())
    } else {
        ListKey::Open(row.clone())
    }
}

/// How wide a table column is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColumnWidth {
    /// Exactly this many points on a desktop and Cells in a terminal.
    Fixed { points: f32, cells: f32 },
    /// Shares what the fixed columns leave, never under its least width; `grow` weighs it
    /// against the other flexible columns. `most` (points, Cells) bounds it, so on a wide page
    /// a name keeps a readable line and the columns after it stay beside it rather than
    /// across an empty middle.
    Flex {
        points: f32,
        cells: f32,
        grow: f32,
        most: Option<(f32, f32)>,
    },
}

impl ColumnWidth {
    const fn least(self, terminal: bool) -> f32 {
        match (self, terminal) {
            (Self::Fixed { cells, .. } | Self::Flex { cells, .. }, true) => cells,
            (Self::Fixed { points, .. } | Self::Flex { points, .. }, false) => points,
        }
    }
}

/// The inspector beside a list: a quarter of the page, never under the mock's 340 points (36
/// Cells) so it keeps its width wherever it showed before, and never over 520 (56), past which
/// its facts and buttons gain nothing. `page` is in points on a desktop, Cells in a terminal.
pub fn inspector_width(page: f32, terminal: bool) -> f32 {
    let (least, most) = if terminal {
        (36.0, 56.0)
    } else {
        (340.0, 520.0)
    };
    (page * 0.25).clamp(least, most)
}

/// One column of a list's table: the header's name and how its cells sit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Column {
    /// `title`, `project`: names the column for the page's cell builder.
    pub key: &'static str,
    /// The header's words; empty over a mark.
    pub title: &'static str,
    /// The header's words in a terminal, where they differ: a column of a few Cells takes a
    /// shorter name.
    pub short: Option<&'static str>,
    pub width: ColumnWidth,
    /// Figures and the trailing step sit at the column's trailing edge.
    pub end: bool,
    /// When the column gives way as the list narrows: 1 first (profile), 2 next (project);
    /// none for a column that always shows.
    pub gives_way: Option<u8>,
}

impl Column {
    pub const fn fixed(key: &'static str, title: &'static str, points: f32, cells: f32) -> Column {
        Column {
            key,
            title,
            width: ColumnWidth::Fixed { points, cells },
            end: false,
            gives_way: None,
            short: None,
        }
    }
    pub const fn flex(
        key: &'static str,
        title: &'static str,
        points: f32,
        cells: f32,
        grow: f32,
    ) -> Column {
        Column {
            key,
            title,
            width: ColumnWidth::Flex {
                points,
                cells,
                grow,
                most: None,
            },
            end: false,
            gives_way: None,
            short: None,
        }
    }
    /// A flexible column grows to at most `points` on a desktop and `cells` in a terminal.
    pub const fn most(mut self, points: f32, cells: f32) -> Column {
        if let ColumnWidth::Flex {
            points: least,
            cells: least_cells,
            grow,
            ..
        } = self.width
        {
            self.width = ColumnWidth::Flex {
                points: least,
                cells: least_cells,
                grow,
                most: Some((points, cells)),
            };
        }
        self
    }
    /// Set against the trailing edge.
    pub const fn end(mut self) -> Column {
        self.end = true;
        self
    }
    /// Gives way `order`-th as the list narrows.
    pub const fn gives_way(mut self, order: u8) -> Column {
        self.gives_way = Some(order);
        self
    }
    /// Named `short` in a terminal's header row.
    pub const fn short(mut self, short: &'static str) -> Column {
        self.short = Some(short);
        self
    }
    /// The header's words on a form.
    pub const fn heading(&self, terminal: bool) -> &'static str {
        match (terminal, self.short) {
            (true, Some(short)) => short,
            _ => self.title,
        }
    }
}

/// Which columns show in `room` (points on a desktop, Cells in a terminal), `gap` between two
/// columns: all of them while their least widths fit, else the ones that give way leave in
/// their order until the rest fits. A column that never gives way always shows.
pub fn fitted(columns: &[Column], room: f32, gap: f32, terminal: bool) -> Vec<bool> {
    fitted_among(columns, &vec![true; columns.len()], room, gap, terminal)
}

/// [`fitted`] over the columns the owner `chosen` to show: one left out takes no room and
/// never shows, while one chosen still gives way in its order.
pub fn fitted_among(
    columns: &[Column],
    chosen: &[bool],
    room: f32,
    gap: f32,
    terminal: bool,
) -> Vec<bool> {
    let mut shown: Vec<bool> = (0..columns.len())
        .map(|at| chosen.get(at).copied().unwrap_or(true))
        .collect();
    let needs = |shown: &[bool]| {
        let widths: f32 = columns
            .iter()
            .zip(shown)
            .filter(|(_, shown)| **shown)
            .map(|(column, _)| column.width.least(terminal))
            .sum();
        let count = shown.iter().filter(|shown| **shown).count();
        widths + gap * count.saturating_sub(1) as f32
    };
    let mut leaving: Vec<(u8, usize)> = columns
        .iter()
        .enumerate()
        .filter(|(at, _)| shown[*at])
        .filter_map(|(at, column)| column.gives_way.map(|order| (order, at)))
        .collect();
    leaving.sort_unstable();
    for (_, at) in leaving {
        if needs(&shown) <= room {
            break;
        }
        shown[at] = false;
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<Id> {
        names.iter().map(|name| Id::from(*name)).collect()
    }

    fn section(key: &str, fold: Option<bool>, rows: &[&str]) -> Section {
        Section {
            key: key.into(),
            label: key.into(),
            look: None,
            count: rows.len(),
            note: None,
            fold,
            rows: ids(rows),
        }
    }

    /// `j` past a folded Done section selected a row that was not on screen, so Enter opened a
    /// task the owner could not see selected.
    #[test]
    fn the_keys_walk_only_rows_on_screen_and_stop_at_the_ends() {
        let sections = [
            section("blocked", None, &["T-1", "T-2"]),
            section("done", Some(false), &["T-9"]),
            section("gone", Some(true), &["T-3"]),
        ];
        let order = order(&sections);
        assert_eq!(order, ids(&["T-1", "T-2", "T-3"]));
        let first = Id::from("T-1");
        let last = Id::from("T-3");
        assert_eq!(key("j", &order, None), Some(ListKey::Select(first.clone())));
        assert_eq!(
            key("down", &order, Some(&first)),
            Some(ListKey::Select(Id::from("T-2")))
        );
        assert_eq!(
            key("j", &order, Some(&last)),
            Some(ListKey::Select(last.clone()))
        );
        assert_eq!(
            key("k", &order, Some(&first)),
            Some(ListKey::Select(first.clone()))
        );
        assert_eq!(key("enter", &order, Some(&last)), Some(ListKey::Open(last)));
        // Nothing selected, or a selection that left the list: Enter opens nothing.
        assert_eq!(key("enter", &order, None), None);
        assert_eq!(key("enter", &order, Some(&Id::from("T-9"))), None);
        assert_eq!(key("j", &[], None), None);
    }

    /// The first row is selected on arrival, and a selection survives a refresh that keeps it.
    #[test]
    fn a_selection_that_left_the_list_falls_back_to_the_first_row() {
        let order = ids(&["T-1", "T-2"]);
        assert_eq!(settled(&order, None), Some(Id::from("T-1")));
        assert_eq!(
            settled(&order, Some(&Id::from("T-2"))),
            Some(Id::from("T-2"))
        );
        assert_eq!(
            settled(&order, Some(&Id::from("T-7"))),
            Some(Id::from("T-1"))
        );
        assert_eq!(settled(&[], Some(&Id::from("T-7"))), None);
    }

    /// With the inspector hidden nothing shows a selection, so a first press that only
    /// selected would look like a dead row.
    #[test]
    fn a_row_opens_on_one_press_without_the_inspector_and_on_the_second_with_it() {
        let row = Id::from("T-1");
        assert_eq!(press(&row, None, true), ListKey::Select(row.clone()));
        assert_eq!(press(&row, Some(&row), true), ListKey::Open(row.clone()));
        assert_eq!(press(&row, None, false), ListKey::Open(row));
    }

    /// A narrow list cut every column a little, so ids and titles lost their letters while
    /// the profile kept its room: the columns named to give way leave whole, profile first.
    #[test]
    fn columns_give_way_in_their_order_and_the_rest_always_show() {
        let columns = [
            Column::fixed("id", "ID", 58.0, 7.0),
            Column::flex("title", "Title", 140.0, 16.0, 1.4),
            Column::fixed("project", "Project", 104.0, 12.0).gives_way(2),
            Column::fixed("profile", "Profile", 92.0, 11.0).gives_way(1),
            Column::fixed("next", "Next step", 150.0, 16.0).end(),
        ];
        assert_eq!(fitted(&columns, 600.0, 8.0, false), [true; 5]);
        assert_eq!(
            fitted(&columns, 500.0, 8.0, false),
            [true, true, true, false, true]
        );
        assert_eq!(
            fitted(&columns, 400.0, 8.0, false),
            [true, true, false, false, true]
        );
        // Narrower than the columns that stay: they still show, and the flexible one is cut.
        assert_eq!(
            fitted(&columns, 100.0, 8.0, false),
            [true, true, false, false, true]
        );
        // A terminal measures in Cells: 80 columns of page keep id, title and the step.
        assert_eq!(
            fitted(&columns, 44.0, 1.0, true),
            [true, true, false, false, true]
        );
    }

    /// Hiding a column freed no room: a column the owner left out still counted, so the ones
    /// shown gave way early, and one the owner chose was kept at any width.
    #[test]
    fn a_column_left_out_takes_no_room_and_a_chosen_one_still_gives_way() {
        let columns = [
            Column::fixed("id", "ID", 58.0, 7.0),
            Column::fixed("priority", "Pri", 30.0, 3.0).gives_way(3),
            Column::fixed("project", "Project", 104.0, 12.0).gives_way(2),
            Column::fixed("cost", "Cost", 50.0, 5.0).gives_way(1),
        ];
        let chosen = [true, true, true, false];
        assert_eq!(
            fitted_among(&columns, &chosen, 210.0, 8.0, false),
            [true, true, true, false]
        );
        assert_eq!(
            fitted_among(&columns, &chosen, 100.0, 8.0, false),
            [true, true, false, false]
        );
        assert_eq!(
            fitted_among(&columns, &chosen, 60.0, 8.0, false),
            [true, false, false, false]
        );
    }

    /// On a 3400-point window the inspector stayed 340 points wide, so the selected row's
    /// facts sat in a narrow strip while the list stretched across the rest: it grows with
    /// the page, yet keeps its width where it showed before and stops at a readable bound.
    #[test]
    fn the_inspector_grows_with_a_wide_page_within_its_bounds() {
        // A 1440-point window less the sidebar.
        assert_eq!(inspector_width(1232.0, false), 340.0);
        assert!(inspector_width(2400.0, false) > 340.0);
        assert_eq!(inspector_width(3192.0, false), 520.0);
        assert_eq!(inspector_width(68.0, true), 36.0);
        assert_eq!(inspector_width(300.0, true), 56.0);
    }
}
