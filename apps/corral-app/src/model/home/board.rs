//! Home's board on the desktop and web: widgets the owner arranges, kept on this device.
//!
//! The board is a flow grid of [`COLUMNS`] columns. Each widget has a span of columns and is
//! placed in order, wrapping to the next row when it no longer fits; its height follows the
//! tallest widget of its row. Customize adds and removes widgets, moves them (drag, or the
//! earlier/later buttons) and changes their span (drag the right edge, or the narrower/wider
//! buttons). The arrangement is stored as one line under [`KEY`] in this device's storage (the
//! browser's local storage on the web); nothing is sent to corrald.

/// Where this device keeps its arrangement.
pub const KEY: &str = "corral.home.board";
/// The grid's columns.
pub const COLUMNS: u8 = 12;
/// The first word of a stored arrangement: a later shape can tell this one apart.
const VERSION: &str = "v1";

/// What Customize and the board say.
pub const CUSTOMIZE: &str = "Customize";
pub const CUSTOMIZE_HINT: &str = "Add, remove, move and resize Home's widgets on this device";
pub const DONE: &str = "Done";
pub const RESET: &str = "Reset layout";
pub const ADD: &str = "Add widget";
pub const ALL_SHOWN: &str = "Every widget is on the board.";
pub const EDITING_NOTE: &str =
    "Drag a widget by its head to move it, drag its right edge to resize. Saved on this device.";
pub const EMPTY: &str = "The board is empty. Customize to add widgets.";

/// A widget Home can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Widget {
    StatNeeds,
    StatWorking,
    StatReady,
    StatSessions,
    StatLanded,
    StatCost,
    NeedsYou,
    Working,
    ByStatus,
    Project,
    Cost,
    Events,
    Finished,
    Burnup,
    /// The daemon host's CPU, memory, disk and network (`model::host`).
    Host,
}

impl Widget {
    /// Every widget, in the order Add widget lists them.
    pub const ALL: [Widget; 15] = [
        Widget::NeedsYou,
        Widget::Working,
        Widget::ByStatus,
        Widget::Project,
        Widget::Cost,
        Widget::Events,
        Widget::Finished,
        Widget::Burnup,
        Widget::Host,
        Widget::StatNeeds,
        Widget::StatWorking,
        Widget::StatReady,
        Widget::StatSessions,
        Widget::StatLanded,
        Widget::StatCost,
    ];

    /// The stored and test-id name (`board-<slug>`).
    pub const fn slug(self) -> &'static str {
        match self {
            Widget::StatNeeds => "stat-needs",
            Widget::StatWorking => "stat-working",
            Widget::StatReady => "stat-ready",
            Widget::StatSessions => "stat-sessions",
            Widget::StatLanded => "stat-landed",
            Widget::StatCost => "stat-cost",
            Widget::NeedsYou => "needs-you",
            Widget::Working => "working",
            Widget::ByStatus => "by-status",
            Widget::Project => "project",
            Widget::Cost => "cost",
            Widget::Events => "events",
            Widget::Finished => "finished",
            Widget::Burnup => "burnup",
            Widget::Host => "host",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Widget> {
        Widget::ALL.into_iter().find(|w| w.slug() == slug)
    }

    /// The name Customize shows.
    pub const fn title(self) -> &'static str {
        match self {
            Widget::StatNeeds => "Needs you (number)",
            Widget::StatWorking => "Working (number)",
            Widget::StatReady => "Ready (number)",
            Widget::StatSessions => "Sessions (number)",
            Widget::StatLanded => "Landed since last visit (number)",
            Widget::StatCost => "Cost today (number)",
            Widget::NeedsYou => "Needs you",
            Widget::Working => "Working",
            Widget::ByStatus => "Tasks by status",
            Widget::Project => "Project",
            Widget::Cost => "Cost today",
            Widget::Events => "Latest events",
            Widget::Finished => "Just finished",
            Widget::Burnup => "Burn-up",
            Widget::Host => "Host performance",
        }
    }

    /// The tile key (`model::home::Tile::key`) a number widget shows.
    pub const fn tile(self) -> Option<&'static str> {
        match self {
            Widget::StatNeeds => Some("needs"),
            Widget::StatWorking => Some("working"),
            Widget::StatReady => Some("ready"),
            Widget::StatSessions => Some("sessions"),
            Widget::StatLanded => Some("landed"),
            Widget::StatCost => Some("cost"),
            _ => None,
        }
    }

    /// The fewest columns it keeps readable.
    pub const fn least(self) -> u8 {
        match self {
            Widget::NeedsYou | Widget::Working | Widget::Burnup => 4,
            _ if self.tile().is_some() => 2,
            _ => 3,
        }
    }

    /// The span it takes when added.
    pub const fn span(self) -> u8 {
        match self {
            Widget::NeedsYou | Widget::Working | Widget::Events | Widget::Finished => 6,
            Widget::Burnup | Widget::Host => 12,
            _ if self.tile().is_some() => 2,
            _ => 4,
        }
    }
}

/// One placed widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    pub widget: Widget,
    /// Columns of [`COLUMNS`], at least the widget's least.
    pub span: u8,
}

impl Item {
    fn new(widget: Widget, span: u8) -> Self {
        Self {
            widget,
            span: span.clamp(widget.least(), COLUMNS),
        }
    }
}

/// The arrangement: the widgets in reading order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    pub items: Vec<Item>,
}

impl Default for Board {
    /// Rows that each fill the grid: the six numbers; Needs you beside Working; status, cost
    /// and project; events beside what just finished; the burn-up across; the daemon host's
    /// performance across.
    fn default() -> Self {
        Self {
            items: [
                Widget::StatNeeds,
                Widget::StatWorking,
                Widget::StatReady,
                Widget::StatSessions,
                Widget::StatLanded,
                Widget::StatCost,
                Widget::NeedsYou,
                Widget::Working,
                Widget::ByStatus,
                Widget::Cost,
                Widget::Project,
                Widget::Events,
                Widget::Finished,
                Widget::Burnup,
                Widget::Host,
            ]
            .into_iter()
            .map(|w| Item::new(w, w.span()))
            .collect(),
        }
    }
}

impl Board {
    /// A stored arrangement: `v1 <slug>:<span> ...`. Unknown widgets and repeats are dropped,
    /// spans clamped; another version or a malformed line is none, so the default shows.
    pub fn parse(stored: &str) -> Option<Board> {
        let mut words = stored.split_whitespace();
        if words.next() != Some(VERSION) {
            return None;
        }
        let mut items: Vec<Item> = Vec::new();
        for word in words {
            let (slug, span) = word.split_once(':')?;
            let span: u8 = span.parse().ok()?;
            if let Some(widget) = Widget::from_slug(slug)
                && !items.iter().any(|i| i.widget == widget)
            {
                items.push(Item::new(widget, span));
            }
        }
        Some(Board { items })
    }

    /// The line [`Board::parse`] reads back.
    pub fn encode(&self) -> String {
        let mut line = VERSION.to_owned();
        for item in &self.items {
            line.push_str(&format!(" {}:{}", item.widget.slug(), item.span));
        }
        line
    }

    pub fn has(&self, widget: Widget) -> bool {
        self.items.iter().any(|i| i.widget == widget)
    }

    fn at(&self, widget: Widget) -> Option<usize> {
        self.items.iter().position(|i| i.widget == widget)
    }

    /// The widgets not on the board, in [`Widget::ALL`]'s order.
    pub fn hidden(&self) -> Vec<Widget> {
        Widget::ALL.into_iter().filter(|w| !self.has(*w)).collect()
    }

    /// Puts a hidden widget at the end with its usual span.
    pub fn add(&mut self, widget: Widget) {
        if !self.has(widget) {
            self.items.push(Item::new(widget, widget.span()));
        }
    }

    pub fn remove(&mut self, widget: Widget) {
        self.items.retain(|i| i.widget != widget);
    }

    /// Moves a widget so it stands at `index` among the others (0 first; past the end last).
    pub fn move_to(&mut self, widget: Widget, index: usize) {
        if let Some(from) = self.at(widget) {
            let item = self.items.remove(from);
            let index = index.min(self.items.len());
            self.items.insert(index, item);
        }
    }

    /// One place earlier (`-1`) or later (`1`).
    pub fn step(&mut self, widget: Widget, by: isize) {
        if let Some(from) = self.at(widget) {
            let to = from.saturating_add_signed(by).min(self.items.len() - 1);
            self.move_to(widget, to);
        }
    }

    /// Sets a widget's span, kept between its least and the whole grid.
    pub fn resize(&mut self, widget: Widget, span: u8) {
        if let Some(at) = self.at(widget) {
            self.items[at] = Item::new(widget, span);
        }
    }

    /// The next narrower or wider span among the usual ones.
    pub fn nudge(&mut self, widget: Widget, wider: bool) {
        let Some(at) = self.at(widget) else { return };
        let now = self.items[at].span;
        let next = if wider {
            STEPS.iter().copied().find(|s| *s > now).unwrap_or(COLUMNS)
        } else {
            STEPS.iter().rev().copied().find(|s| *s < now).unwrap_or(0)
        };
        self.resize(widget, next);
    }
}

/// The spans the narrower and wider buttons step through.
const STEPS: [u8; 7] = [2, 3, 4, 6, 8, 9, 12];

/// Whether a board `width` points wide shows every widget at its own span, so a dragged edge
/// follows the pointer; narrower, spans are widened and the edge is not offered.
pub fn spans_follow(width: f32) -> bool {
    width >= 960.0
}

/// The columns a widget takes on a board `width` points wide: its span when the grid has room
/// for twelve columns, doubled on a medium window, the whole row on a narrow one.
pub fn shown_span(span: u8, width: f32) -> u8 {
    if spans_follow(width) {
        span
    } else if width >= 600.0 {
        span.saturating_mul(2).min(COLUMNS)
    } else {
        COLUMNS
    }
}

/// The span a resize drag ends at: the span it began at plus the columns the pointer moved,
/// rounded to the nearest column and kept between the least and the whole grid.
pub fn dragged_span(widget: Widget, from: u8, dx: f32, column: f32) -> u8 {
    if column <= 0.0 {
        return from;
    }
    let span = (f32::from(from) + dx / column).round();
    (span.clamp(0.0, f32::from(COLUMNS)) as u8).clamp(widget.least(), COLUMNS)
}

/// A placed widget's frame, in the coordinates of the pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Frame {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    fn distance(&self, x: f32, y: f32) -> f32 {
        let dx = (self.x - x).max(0.0).max(x - (self.x + self.w));
        let dy = (self.y - y).max(0.0).max(y - (self.y + self.h));
        dx.hypot(dy)
    }
}

/// Where a moved widget would drop: the widget the pointer is over (or nearest to), and
/// whether it goes after it (the pointer past its middle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drop {
    pub target: Widget,
    pub after: bool,
}

/// The drop for `dragged` with the pointer at (`x`, `y`) over the widgets' frames; none over
/// the dragged widget itself or with no other widget.
pub fn drop_at(frames: &[(Widget, Frame)], dragged: Widget, x: f32, y: f32) -> Option<Drop> {
    if frames
        .iter()
        .any(|(w, f)| *w == dragged && f.contains(x, y))
    {
        return None;
    }
    let others = frames.iter().filter(|(w, _)| *w != dragged);
    let (target, frame) = others
        .clone()
        .find(|(_, f)| f.contains(x, y))
        .or_else(|| others.min_by(|a, b| a.1.distance(x, y).total_cmp(&b.1.distance(x, y))))?;
    Some(Drop {
        target: *target,
        after: x > frame.x + frame.w / 2.0,
    })
}

impl Board {
    /// Moves `dragged` to `drop`.
    pub fn drop_on(&mut self, dragged: Widget, drop: Drop) {
        if dragged == drop.target {
            return;
        }
        let others: Vec<Widget> = self
            .items
            .iter()
            .map(|i| i.widget)
            .filter(|w| *w != dragged)
            .collect();
        if let Some(at) = others.iter().position(|w| *w == drop.target) {
            self.move_to(dragged, at + usize::from(drop.after));
        }
    }
}

#[cfg(test)]
#[path = "board_tests.rs"]
mod tests;
