//! The layered drawing behind the map's graph: rank by references, wrap wide ranks, order
//! rows to reduce crossings, pull cards toward their neighbours without overlap, then route
//! each edge through the rows it crosses. A port of the mockup's `layered()`
//! (`docs/mockup/system-map.html`).

use std::collections::{BTreeMap, BTreeSet};

use super::layout::Rect;

pub(super) struct LEdge {
    /// Index of the edge in `Scene.edges`.
    pub index: usize,
    pub a: usize,
    pub b: usize,
    /// Stronger edges keep the flow when a cycle has to be broken.
    pub pri: f64,
}

pub(super) struct Layered {
    pub pos: Vec<Rect>,
    pub routes: Vec<(usize, Vec<(f32, f32)>)>,
    pub width: f32,
    pub height: f32,
    /// Where the first row of ports below the package starts.
    pub bottom_top: Option<f32>,
}

#[derive(Clone, Copy)]
struct Vertex {
    row: usize,
    w: f32,
    h: f32,
    dummy: bool,
}

/// A box's end of a segment, `(box, other end)`, and the x it heads toward.
type Port = ((usize, usize), f32);

const GAP: f32 = 24.0;
/// The share of a row's room its cards may fill when a wide rank wraps.
const ROOM_FOR_EDGES: f32 = 0.9;
/// Room for a caption over the rows of ports.
const CAPTION_GAP: f32 = 30.0;

fn reaches(out: &[Vec<usize>], from: usize, to: usize) -> bool {
    let mut stack = vec![from];
    let mut seen = BTreeSet::from([from]);
    while let Some(x) = stack.pop() {
        if x == to {
            return true;
        }
        for &y in &out[x] {
            if seen.insert(y) {
                stack.push(y);
            }
        }
    }
    false
}

fn rank_of(x: usize, out: &[Vec<usize>], free: &[bool], rank: &mut [Option<i32>]) -> i32 {
    if let Some(r) = rank[x] {
        return r;
    }
    let mut r = 0;
    for &y in &out[x] {
        if free[y] {
            r = r.max(rank_of(y, out, free, rank) + 1);
        }
    }
    rank[x] = Some(r);
    r
}

/// Places ordered boxes as close to their desired centres as possible without overlap
/// (weighted isotonic regression).
fn pava(widths: &[f32], desired: &[f32], weights: &[f32], gap: impl Fn(usize) -> f32) -> Vec<f32> {
    let n = widths.len();
    let mut off = vec![0.0_f32; n];
    for i in 1..n {
        off[i] = off[i - 1] + (widths[i - 1] + widths[i]) / 2.0 + gap(i);
    }
    // (value, weight, count)
    let mut blocks: Vec<(f32, f32, usize)> = Vec::new();
    for i in 0..n {
        let mut b = (desired[i] - off[i], weights[i].max(1e-3), 1);
        while let Some(&a) = blocks.last() {
            if a.0 <= b.0 {
                break;
            }
            blocks.pop();
            let w = a.1 + b.1;
            b = ((a.0 * a.1 + b.0 * b.1) / w, w, a.2 + b.2);
        }
        blocks.push(b);
    }
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    for (v, _, count) in blocks {
        for _ in 0..count {
            out.push(v + off[i]);
            i += 1;
        }
    }
    out
}

fn mean(values: impl Iterator<Item = f32>) -> Option<f32> {
    let (sum, n) = values.fold((0.0, 0), |(s, n), v| (s + v, n + 1));
    (n > 0).then(|| sum / n as f32)
}

pub(super) fn layered(
    sizes: &[(f32, f32)],
    edges: &[LEdge],
    top: &[bool],
    bottom: &[bool],
    max_row_w: f32,
    (has_top, has_bottom): (bool, bool),
) -> Layered {
    let n = sizes.len();
    let edges: Vec<&LEdge> = edges
        .iter()
        .filter(|e| e.a != e.b && e.a < n && e.b < n)
        .collect();
    // 1. Break cycles: strongest first; one that would close a cycle runs against the flow.
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut by_strength = edges.clone();
    by_strength.sort_by(|x, y| y.pri.total_cmp(&x.pri));
    for e in &by_strength {
        if !top[e.b] && !bottom[e.a] && !reaches(&out, e.b, e.a) {
            out[e.a].push(e.b);
        }
    }
    // 2. Rank: longest path down to a node that references nothing; such sinks then sit just
    // below their nearest user.
    let free: Vec<bool> = (0..n).map(|i| !top[i] && !bottom[i]).collect();
    let mut rank: Vec<Option<i32>> = vec![None; n];
    for x in (0..n).filter(|x| free[*x]) {
        rank_of(x, &out, &free, &mut rank);
    }
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (a, list) in out.iter().enumerate() {
        for &b in list {
            parents[b].push(a);
        }
    }
    for x in (0..n).filter(|x| free[*x]) {
        if out[x].iter().any(|y| free[*y]) {
            continue;
        }
        if let Some(r) = parents[x].iter().filter_map(|p| rank[*p]).min() {
            rank[x] = Some(r - 1);
        }
    }
    let ranked: Vec<i32> = rank.iter().flatten().copied().collect();
    let lo = ranked.iter().copied().min().unwrap_or(0);
    let hi = ranked.iter().copied().max().unwrap_or(0);
    for i in 0..n {
        if top[i] {
            rank[i] = Some(hi + 1);
        } else if bottom[i] {
            rank[i] = Some(lo - 1);
        }
    }
    let rank: Vec<i32> = rank.into_iter().map(|r| r.unwrap_or(0)).collect();
    let max_r = rank.iter().copied().max().unwrap_or(0);
    let min_r = rank.iter().copied().min().unwrap_or(0);
    // A rank wider than the room wraps onto more rows; one rank never references itself.
    let mut by_rank: Vec<Vec<usize>> = vec![Vec::new(); (max_r - min_r + 1) as usize];
    for x in 0..n {
        by_rank[(max_r - rank[x]) as usize].push(x);
    }
    let mut row_of = vec![0usize; n];
    let mut next = 0;
    for list in by_rank.iter().filter(|l| !l.is_empty()) {
        let total: f32 = list.iter().map(|x| sizes[*x].0 + GAP).sum();
        // Edges crossing a row take room the cards alone do not; leave them some.
        let parts = ((total / (max_row_w * ROOM_FOR_EDGES)).ceil() as usize).max(1);
        let target = total / parts as f32;
        let (mut part, mut acc) = (0, 0.0);
        for &x in list {
            let sw = sizes[x].0 + GAP;
            if acc > 0.0 && acc + sw / 2.0 > target * (part + 1) as f32 && part < parts - 1 {
                part += 1;
            }
            row_of[x] = next + part;
            acc += sw;
        }
        next += parts;
    }
    let n_rows = next.max(1);
    // 3. Rows, with a dummy vertex wherever an edge crosses a row; long edges leaving the same
    // card share one trunk until they branch off.
    let mut verts: Vec<Vertex> = (0..n)
        .map(|i| Vertex {
            row: row_of[i],
            w: sizes[i].0,
            h: sizes[i].1,
            dummy: false,
        })
        .collect();
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); n_rows];
    for i in 0..n {
        rows[row_of[i]].push(i);
    }
    let mut dummies: BTreeMap<(usize, usize, bool), usize> = BTreeMap::new();
    let mut chains: Vec<(Vec<usize>, bool, bool)> = Vec::new();
    let mut seg_set = BTreeSet::new();
    let mut segs: Vec<(usize, usize)> = Vec::new();
    for e in &edges {
        let (mut a, mut b) = (e.a, e.b);
        let (mut ra, mut rb) = (row_of[a], row_of[b]);
        let flip = ra > rb;
        if flip {
            std::mem::swap(&mut a, &mut b);
            std::mem::swap(&mut ra, &mut rb);
        }
        let mut chain = vec![a];
        for (r, row) in rows.iter_mut().enumerate().take(rb).skip(ra + 1) {
            let d = *dummies.entry((a, r, flip)).or_insert_with(|| {
                verts.push(Vertex {
                    row: r,
                    w: 2.0,
                    h: 0.0,
                    dummy: true,
                });
                row.push(verts.len() - 1);
                verts.len() - 1
            });
            chain.push(d);
        }
        chain.push(b);
        if ra != rb {
            for pair in chain.windows(2) {
                if seg_set.insert((pair[0], pair[1])) {
                    segs.push((pair[0], pair[1]));
                }
            }
        }
        chains.push((chain, flip, ra == rb));
    }
    let v = verts.len();
    let mut up: Vec<Vec<usize>> = vec![Vec::new(); v];
    let mut down: Vec<Vec<usize>> = vec![Vec::new(); v];
    for &(a, b) in &segs {
        down[a].push(b);
        up[b].push(a);
    }
    // 4. Order rows to reduce crossings: barycentre sweeps, keep the best, then transpose
    // neighbours.
    let mut pos = vec![0usize; v];
    let set_pos = |rows: &Vec<Vec<usize>>, pos: &mut Vec<usize>| {
        for row in rows {
            for (i, x) in row.iter().enumerate() {
                pos[*x] = i;
            }
        }
    };
    let crossings = |rows: &Vec<Vec<usize>>, pos: &Vec<usize>| {
        let mut c = 0;
        for row in rows {
            let sg: Vec<(usize, usize)> = row
                .iter()
                .flat_map(|x| down[*x].iter().map(|y| (pos[*x], pos[*y])))
                .collect();
            for i in 0..sg.len() {
                for j in i + 1..sg.len() {
                    let d0 = sg[i].0 as i64 - sg[j].0 as i64;
                    let d1 = sg[i].1 as i64 - sg[j].1 as i64;
                    if d0 * d1 < 0 {
                        c += 1;
                    }
                }
            }
        }
        c
    };
    set_pos(&rows, &mut pos);
    let mut best = rows.clone();
    let mut best_c = crossings(&rows, &pos);
    for it in 0..24 {
        if best_c == 0 {
            break;
        }
        let order: Vec<usize> = if it % 2 == 0 {
            (1..n_rows).collect()
        } else {
            (0..n_rows.saturating_sub(1)).rev().collect()
        };
        for r in order {
            let nb = if it % 2 == 0 { &up } else { &down };
            let mut keyed: Vec<(usize, f32, usize)> = rows[r]
                .iter()
                .enumerate()
                .map(|(i, x)| {
                    let bary = mean(nb[*x].iter().map(|y| pos[*y] as f32)).unwrap_or(i as f32);
                    (*x, bary, i)
                })
                .collect();
            keyed.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.2.cmp(&b.2)));
            rows[r] = keyed.into_iter().map(|k| k.0).collect();
            for (i, x) in rows[r].iter().enumerate() {
                pos[*x] = i;
            }
        }
        let c = crossings(&rows, &pos);
        if c < best_c {
            best_c = c;
            best = rows.clone();
        }
    }
    rows = best;
    set_pos(&rows, &mut pos);
    for _ in 0..4 {
        if best_c == 0 {
            break;
        }
        let mut improved = false;
        for r in 0..n_rows {
            for i in 0..rows[r].len().saturating_sub(1) {
                rows[r].swap(i, i + 1);
                set_pos(&rows, &mut pos);
                let c = crossings(&rows, &pos);
                if c < best_c {
                    best_c = c;
                    improved = true;
                } else {
                    rows[r].swap(i, i + 1);
                    set_pos(&rows, &mut pos);
                }
            }
        }
        if !improved {
            break;
        }
    }
    // 5. x: pull each vertex toward its neighbours, never overlapping.
    let gap_between = |a: usize, b: usize| match (verts[a].dummy, verts[b].dummy) {
        (true, true) => 5.0,
        (true, false) | (false, true) => 12.0,
        (false, false) => GAP,
    };
    let mut xs = vec![0.0_f32; v];
    let row_width = |row: &Vec<usize>, xs: &Vec<f32>| {
        row.last()
            .map_or(0.0, |last| xs[*last] + verts[*last].w / 2.0)
    };
    for row in &rows {
        let mut x = 0.0;
        for (i, &vx) in row.iter().enumerate() {
            if i > 0 {
                x += gap_between(row[i - 1], vx);
            }
            xs[vx] = x + verts[vx].w / 2.0;
            x += verts[vx].w;
        }
    }
    let widest = rows.iter().map(|r| row_width(r, &xs)).fold(0.0, f32::max);
    for row in &rows {
        let off = (widest - row_width(row, &xs)) / 2.0;
        for &vx in row {
            xs[vx] += off;
        }
    }
    let place = |r: usize, nbs: &[&Vec<Vec<usize>>], xs: &mut Vec<f32>| {
        let items = &rows[r];
        if items.is_empty() {
            return;
        }
        let desired: Vec<f32> = items
            .iter()
            .map(|&vx| {
                let (mut total, mut weight) = (0.0, 0.0);
                for nb in nbs {
                    for &x in &nb[vx] {
                        let k = match (verts[vx].dummy, verts[x].dummy) {
                            (true, true) => 6.0,
                            (true, false) | (false, true) => 2.0,
                            (false, false) => 1.0,
                        };
                        total += xs[x] * k;
                        weight += k;
                    }
                }
                if weight > 0.0 { total / weight } else { xs[vx] }
            })
            .collect();
        let weights: Vec<f32> = items
            .iter()
            .map(|&vx| {
                if verts[vx].dummy {
                    3.0
                } else if up[vx].len() + down[vx].len() > 0 {
                    1.0
                } else {
                    0.05
                }
            })
            .collect();
        let widths: Vec<f32> = items.iter().map(|x| verts[*x].w).collect();
        let placed = pava(&widths, &desired, &weights, |i| {
            gap_between(items[i - 1], items[i])
        });
        for (i, &vx) in items.iter().enumerate() {
            xs[vx] = placed[i];
        }
    };
    for _ in 0..10 {
        for r in 1..n_rows {
            place(r, &[&up], &mut xs);
        }
        for r in (0..n_rows.saturating_sub(1)).rev() {
            place(r, &[&down], &mut xs);
        }
    }
    for r in 0..n_rows {
        place(r, &[&up, &down], &mut xs);
    }
    // Close empty vertical corridors (between unconnected groups) down to one wide gap.
    let mut spans: Vec<(f32, f32)> = (0..v)
        .map(|x| (xs[x] - verts[x].w / 2.0, xs[x] + verts[x].w / 2.0))
        .collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut cuts: Vec<(f32, f32)> = Vec::new();
    let mut end: Option<f32> = None;
    for (a, c) in spans {
        if let Some(e) = end
            && a - e > 64.0
        {
            cuts.push((e, a - e - 64.0));
        }
        end = Some(end.map_or(c, |e: f32| e.max(c)));
    }
    if !cuts.is_empty() {
        for x in xs.iter_mut() {
            let shift: f32 = cuts.iter().filter(|(at, _)| *x > *at).map(|c| c.1).sum();
            *x -= shift;
        }
    }
    // Still wider than the room: scale centres toward the left edge and let each row settle
    // again without overlap.
    let bounds = |xs: &Vec<f32>| {
        (0..v).fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), x| {
            (
                lo.min(xs[x] - verts[x].w / 2.0),
                hi.max(xs[x] + verts[x].w / 2.0),
            )
        })
    };
    let (lo, hi) = bounds(&xs);
    if hi - lo > max_row_w {
        let k = max_row_w / (hi - lo);
        for row in &rows {
            if row.is_empty() {
                continue;
            }
            let widths: Vec<f32> = row.iter().map(|x| verts[*x].w).collect();
            let desired: Vec<f32> = row.iter().map(|x| lo + (xs[*x] - lo) * k).collect();
            let placed = pava(&widths, &desired, &vec![1.0; row.len()], |i| {
                gap_between(row[i - 1], row[i])
            });
            for (i, &vx) in row.iter().enumerate() {
                xs[vx] = placed[i];
            }
        }
    }
    let (min_x, _) = bounds(&xs);
    let min_x = if min_x.is_finite() { min_x } else { 0.0 };
    for x in xs.iter_mut() {
        *x -= min_x;
    }
    // A row the settling left past the room's right edge slides back left as far as its own
    // left edge allows; rows never overlap each other, so this cannot make two cards collide.
    for row in &rows {
        let right = row
            .iter()
            .map(|x| xs[*x] + verts[*x].w / 2.0)
            .fold(0.0, f32::max);
        let left = row
            .iter()
            .map(|x| xs[*x] - verts[*x].w / 2.0)
            .fold(f32::INFINITY, f32::min);
        if right > max_row_w && left.is_finite() {
            let shift = (right - max_row_w).min(left).max(0.0);
            for &vx in row {
                xs[vx] -= shift;
            }
        }
        // Still over: the edges crossing this row widened it past the room the wrapping
        // measured for its cards alone. Squeeze it in, the crossing edges packed tight.
        let right = row
            .iter()
            .map(|x| xs[*x] + verts[*x].w / 2.0)
            .fold(0.0, f32::max);
        if right > max_row_w {
            let k = max_row_w / right;
            let widths: Vec<f32> = row.iter().map(|x| verts[*x].w).collect();
            let desired: Vec<f32> = row.iter().map(|x| xs[*x] * k).collect();
            // Crossing edges may share a lane here rather than push a card out of the room.
            let tight = |i: usize| match (verts[row[i - 1]].dummy, verts[row[i]].dummy) {
                (true, true) => -2.0,
                (true, false) | (false, true) => 4.0,
                (false, false) => GAP,
            };
            let placed = pava(&widths, &desired, &vec![1.0; row.len()], tight);
            let lo = row
                .iter()
                .enumerate()
                .map(|(i, x)| placed[i] - verts[*x].w / 2.0)
                .fold(f32::INFINITY, f32::min);
            let lift = lo.min(0.0);
            for (i, &vx) in row.iter().enumerate() {
                xs[vx] = placed[i] - lift;
            }
        }
    }
    let (_, max_x) = bounds(&xs);
    // 6. y: rows top to bottom; busier gaps get a little more room.
    let row_h: Vec<f32> = rows
        .iter()
        .map(|r| r.iter().map(|x| verts[*x].h).fold(0.0, f32::max))
        .collect();
    let first_free = (0..n).filter(|x| free[*x]).map(|x| row_of[x]).min();
    let first_bottom = (0..n).filter(|x| bottom[*x]).map(|x| row_of[x]).min();
    let mut row_top = vec![0.0_f32; n_rows];
    let mut y = 0.0;
    for r in 0..n_rows {
        if r > 0 {
            let nseg: usize = rows[r - 1].iter().map(|x| down[*x].len()).sum();
            y += 56.0 + (nseg as f32 * 1.2).min(30.0);
            if has_top && Some(r) == first_free {
                y += CAPTION_GAP;
            }
            if has_bottom && Some(r) == first_bottom {
                y += CAPTION_GAP;
            }
        }
        row_top[r] = y;
        y += if row_h[r] > 0.0 { row_h[r] } else { 8.0 };
    }
    let pos: Vec<Rect> = (0..n)
        .map(|i| {
            let vx = verts[i];
            Rect {
                x: xs[i] - vx.w / 2.0,
                y: row_top[vx.row] + (row_h[vx.row] - vx.h) / 2.0,
                w: vx.w,
                h: vx.h,
            }
        })
        .collect();
    // 7. Routes: ports spread along each box edge, in the order of where the edge goes next.
    let mut bot: BTreeMap<usize, Vec<Port>> = BTreeMap::new();
    let mut tops: BTreeMap<usize, Vec<Port>> = BTreeMap::new();
    for &(a, b) in &segs {
        if !verts[a].dummy {
            bot.entry(a).or_default().push(((a, b), xs[b]));
        }
        if !verts[b].dummy {
            tops.entry(b).or_default().push(((b, a), xs[a]));
        }
    }
    let mut port_x: BTreeMap<(usize, usize), f32> = BTreeMap::new();
    for map in [bot, tops] {
        for (vx, mut list) in map {
            let b = pos[vx];
            list.sort_by(|p, q| p.1.total_cmp(&q.1));
            let inner = (b.w - if b.h < 40.0 { 24.0 } else { 28.0 }).max(0.0);
            let span = inner.min((list.len() as f32 - 1.0) * 12.0);
            let centre = b.x + b.w / 2.0;
            let avg = mean(list.iter().map(|p| p.1)).unwrap_or(centre);
            let c0 = avg
                .min(centre + inner / 2.0 - span / 2.0)
                .max(centre - inner / 2.0 + span / 2.0);
            let count = list.len();
            for (i, (key, toward)) in list.into_iter().enumerate() {
                let x = if count == 1 {
                    toward.min(b.x + b.w - 14.0).max(b.x + 14.0)
                } else {
                    c0 - span / 2.0 + span * i as f32 / (count - 1) as f32
                };
                port_x.insert(key, x);
            }
        }
    }
    let mut routes = Vec::new();
    for (e, (chain, flip, same)) in edges.iter().zip(chains) {
        let (first, last) = (chain[0], chain[chain.len() - 1]);
        let (a, b) = (pos[first], pos[last]);
        let mut pts = if same {
            let yb = (a.y + a.h).max(b.y + b.h) + 20.0;
            vec![
                (a.x + a.w / 2.0, a.y + a.h),
                (a.x + a.w / 2.0, yb),
                (b.x + b.w / 2.0, yb),
                (b.x + b.w / 2.0, b.y + b.h),
            ]
        } else {
            let mut pts = vec![(
                port_x
                    .get(&(first, chain[1]))
                    .copied()
                    .unwrap_or(a.x + a.w / 2.0),
                a.y + a.h,
            )];
            for &d in &chain[1..chain.len() - 1] {
                let row = verts[d].row;
                pts.push((xs[d], row_top[row] - 2.0));
                pts.push((xs[d], row_top[row] + row_h[row] + 2.0));
            }
            pts.push((
                port_x
                    .get(&(last, chain[chain.len() - 2]))
                    .copied()
                    .unwrap_or(b.x + b.w / 2.0),
                b.y,
            ));
            pts
        };
        if flip {
            pts.reverse();
        }
        routes.push((e.index, pts));
    }
    Layered {
        pos,
        routes,
        width: if max_x.is_finite() { max_x } else { 0.0 },
        height: y,
        bottom_top: first_bottom.filter(|_| has_bottom).map(|r| row_top[r]),
    }
}
