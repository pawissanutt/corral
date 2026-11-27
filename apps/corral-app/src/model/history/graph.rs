//! The lanes of a commit graph: each loaded commit gets a column, and each of its parents an
//! edge to the lane that carries it down to that parent. Commits arrive children before parents
//! (`git log --topo-order`), so a lane waits for one commit at a time. A parent that is not
//! loaded keeps its lane open past the last row: it continues below, further down the walk,
//! and is never drawn as a root. Only a commit without any parent is a root.

use std::collections::BTreeSet;

/// A line through half a row: from a column at the half's top to a column at its bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    pub from: usize,
    pub to: usize,
}

/// One parent's edge: the lane it continues on below its child, and whether the parent is
/// among the loaded commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentEdge {
    pub sha: String,
    pub lane: usize,
    pub loaded: bool,
}

/// A commit's row of the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    /// The commit's column.
    pub column: usize,
    /// From the row's top edge to its middle: lanes passing and lanes that end at this commit.
    pub upper: Vec<Line>,
    /// From the row's middle to its bottom edge: lanes passing and this commit's parent edges.
    pub lower: Vec<Line>,
    /// Every parent in Git's order, first parent first.
    pub parents: Vec<ParentEdge>,
    /// No parent at all: where the history starts.
    pub root: bool,
}

/// A lane still open under the last row: its parent is further down the walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Continuing {
    pub lane: usize,
    pub parent: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Graph {
    pub rows: Vec<GraphRow>,
    /// The widest row's columns.
    pub lanes: usize,
    pub continuing: Vec<Continuing>,
}

/// The graph of `commits` (each its SHA and parents), children before parents.
pub fn lanes<'a>(commits: &[(&'a str, &'a [String])]) -> Graph {
    let loaded: BTreeSet<&str> = commits.iter().map(|(sha, _)| *sha).collect();
    let mut open: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    let mut widest = 0;
    for (sha, parents) in commits {
        let before = open.clone();
        let waiting: Vec<usize> = (0..open.len())
            .filter(|&i| open[i].as_deref() == Some(*sha))
            .collect();
        let column = waiting.first().copied().unwrap_or_else(|| free(&open));
        let upper = before
            .iter()
            .enumerate()
            .filter_map(|(i, lane)| match lane.as_deref() {
                Some(waits) if waits == *sha => Some(Line {
                    from: i,
                    to: column,
                }),
                Some(_) => Some(Line { from: i, to: i }),
                None => None,
            })
            .collect();
        for &i in &waiting {
            open[i] = None;
        }
        if column >= open.len() {
            open.resize(column + 1, None);
        }
        let mut edges = Vec::with_capacity(parents.len());
        let mut lower: Vec<Line> = Vec::new();
        for (n, parent) in parents.iter().enumerate() {
            let lane = match open.iter().position(|lane| lane.as_ref() == Some(parent)) {
                // The first parent keeps the commit's column: a lane to its right already
                // waiting for it bends into this one, so a line of first parents stays straight.
                // A lane to its left keeps its place: this branch joins it there.
                Some(waiting) if n == 0 && waiting > column && open[column].is_none() => {
                    open[waiting] = None;
                    open[column] = Some(parent.clone());
                    lower.push(Line {
                        from: waiting,
                        to: column,
                    });
                    column
                }
                Some(lane) => lane,
                None => {
                    let lane = if n == 0 && open[column].is_none() {
                        column
                    } else {
                        free(&open)
                    };
                    if lane >= open.len() {
                        open.resize(lane + 1, None);
                    }
                    open[lane] = Some(parent.clone());
                    lane
                }
            };
            lower.push(Line {
                from: column,
                to: lane,
            });
            edges.push(ParentEdge {
                sha: parent.clone(),
                lane,
                loaded: loaded.contains(parent.as_str()),
            });
        }
        // A lane this commit neither ended nor started passes straight on.
        for (i, lane) in before.iter().enumerate() {
            if lane.is_some() && lane.as_deref() != Some(*sha) && open.get(i) == Some(lane) {
                lower.push(Line { from: i, to: i });
            }
        }
        while open.last().is_some_and(Option::is_none) {
            open.pop();
        }
        widest = widest.max(before.len()).max(open.len()).max(column + 1);
        rows.push(GraphRow {
            column,
            upper,
            lower,
            parents: edges,
            root: parents.is_empty(),
        });
    }
    let continuing = open
        .into_iter()
        .enumerate()
        .filter_map(|(lane, parent)| parent.map(|parent| Continuing { lane, parent }))
        .collect();
    Graph {
        rows,
        lanes: widest,
        continuing,
    }
}

fn free(open: &[Option<String>]) -> usize {
    open.iter().position(Option::is_none).unwrap_or(open.len())
}

/// Lanes the text graph draws; a wider graph is cut with `…`.
pub const TEXT_LANES: usize = 8;

/// A row as lane glyphs for a terminal or phone, two Cells a lane: `●` the commit (`○` a root),
/// `│` a lane passing, `╮ ╭` a parent edge leaving for another lane, `╯ ╰` a lane ending here,
/// `┤ ├` both, `─` the way across.
pub fn glyphs(row: &GraphRow, lanes: usize) -> String {
    let shown = lanes.min(TEXT_LANES);
    let has = |lines: &[Line], from: usize, to: usize| {
        lines.iter().any(|line| line.from == from && line.to == to)
    };
    let pass = |i: usize| i != row.column && has(&row.upper, i, i) && has(&row.lower, i, i);
    // A lane ending at the commit, or bending into its column under it.
    let up = |i: usize| {
        i != row.column && (has(&row.upper, i, row.column) || has(&row.lower, i, row.column))
    };
    let down = |i: usize| i != row.column && has(&row.lower, row.column, i);
    let joined: Vec<usize> = (0..lanes).filter(|&i| up(i) || down(i)).collect();
    let across = |i: usize| {
        joined
            .iter()
            .any(|&k| row.column.min(k) < i && i < row.column.max(k))
    };
    let span = |i: usize| {
        joined
            .iter()
            .any(|&k| row.column.min(k) <= i && i < row.column.max(k))
    };
    let mut out = String::with_capacity(shown * 2 + 1);
    for i in 0..shown {
        let right = i > row.column;
        let glyph = if i == row.column {
            if row.root { '○' } else { '●' }
        } else {
            match (up(i), down(i), pass(i)) {
                (true, true, _) | (true, false, true) | (false, true, true) => {
                    if right {
                        '┤'
                    } else {
                        '├'
                    }
                }
                (true, false, false) => {
                    if right {
                        '╯'
                    } else {
                        '╰'
                    }
                }
                (false, true, false) => {
                    if right {
                        '╮'
                    } else {
                        '╭'
                    }
                }
                (false, false, true) => '│',
                (false, false, false) if across(i) => '─',
                _ => ' ',
            }
        };
        out.push(glyph);
        out.push(if span(i) { '─' } else { ' ' });
    }
    let beyond = lanes > shown
        && (row.column >= shown
            || row
                .upper
                .iter()
                .chain(&row.lower)
                .any(|l| l.from >= shown || l.to >= shown));
    if beyond {
        out.pop();
        out.push('…');
    }
    out
}

/// The line under the last row: `┊` where a lane continues below.
pub fn continuing_glyphs(graph: &Graph) -> String {
    let shown = graph.lanes.min(TEXT_LANES);
    (0..shown)
        .flat_map(|i| {
            let open = graph.continuing.iter().any(|c| c.lane == i);
            [if open { '┊' } else { ' ' }, ' ']
        })
        .collect()
}
