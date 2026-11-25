//! What the app's graphical charts share as numbers: a value scale of round steps from zero,
//! compact scale labels, which of a run of date ticks keep their label, and how wide a bar is
//! in its slot. `ui::chart` paints with them; the terminal draws its own Cells and takes none.
//!
//! The conventions: a value axis always starts at zero with a visible baseline, and its top is
//! the first round step (1, 2 or 5 times a power of ten) over the series' largest value, so a
//! gridline never lands on an odd figure. Date labels thin from the newest end, so the latest
//! day always keeps its label. Bars keep a fixed widest, so a wide window spaces them out
//! rather than drawing slabs.

/// A value axis from zero: `ticks` from 0 to `top`, each a gridline with its label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scale {
    pub top: u64,
    pub ticks: Vec<u64>,
}

/// The smallest round step (1, 2 or 5 times a power of ten) that reaches `max` in `lines`
/// steps; 1 for an empty series.
pub fn step(max: u64, lines: u64) -> u64 {
    let need = max.div_ceil(lines.max(1)).max(1);
    let mut magnitude: u64 = 1;
    loop {
        for factor in [1_u64, 2, 5] {
            let step = factor.saturating_mul(magnitude);
            if step >= need {
                return step;
            }
        }
        magnitude = magnitude.saturating_mul(10);
    }
}

/// The axis for a series whose largest value is `max`, in at most `lines` steps over zero. The
/// top is the last tick at or over `max`, so the busiest value never touches the frame's edge
/// by accident and an empty series still has one step to draw.
pub fn scale(max: u64, lines: u64) -> Scale {
    let step = step(max, lines);
    let steps = max.div_ceil(step).max(1);
    Scale {
        top: step * steps,
        ticks: (0..=steps).map(|at| at * step).collect(),
    }
}

/// A scale label: `0`, `800`, `1.5k`, `250k`, `2M`, `1.2B`, a decimal only where the step
/// needs it.
pub fn compact(value: u64) -> String {
    let (unit, suffix) = match value {
        0..1_000 => return value.to_string(),
        1_000..1_000_000 => (1_000.0, "k"),
        1_000_000..1_000_000_000 => (1_000_000.0, "M"),
        _ => (1_000_000_000.0, "B"),
    };
    let scaled = format!("{:.1}", value as f64 / unit);
    let scaled = scaled.strip_suffix(".0").unwrap_or(&scaled);
    format!("{scaled}{suffix}")
}

/// The ticks of `count` evenly spaced items, `room` wide, that keep a label `label` wide with
/// room between: every one when they fit, else every 2nd, 7th, 14th… counted back from the
/// last, so the newest keeps its label and a week's labels fall on the same weekday.
pub fn labelled(count: usize, room: f32, label: f32) -> Vec<usize> {
    if count == 0 {
        return Vec::new();
    }
    let slot = room / count as f32;
    let need = if slot > 0.0 {
        (label / slot).ceil().max(1.0) as usize
    } else {
        count
    };
    let mut every = 1;
    for step in [1, 2, 7, 14] {
        every = step;
        if step >= need {
            break;
        }
    }
    while every < need {
        every *= 2;
    }
    let mut ticks: Vec<usize> = (0..count).rev().step_by(every).collect();
    ticks.reverse();
    ticks
}

/// A bar's width in a slot of `slot`: most of it with a gap either side, at most `widest`, and
/// the whole slot once the slot is too narrow for a gap to read.
pub fn bar_width(slot: f32, widest: f32) -> f32 {
    if slot < 4.0 {
        slot.max(0.5)
    } else {
        (slot * 0.62).min(widest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_top_covers_the_largest_value_on_a_round_step() {
        for (max, top) in [(0, 1), (7, 8), (1_830_000, 2_000_000), (950, 1_000)] {
            let scale = scale(max, 4);
            assert_eq!(scale.top, top, "max {max}");
            assert_eq!(scale.ticks.first(), Some(&0));
            assert_eq!(scale.ticks.last(), Some(&top));
        }
    }

    #[test]
    fn thinned_date_labels_keep_the_newest_and_do_not_collide() {
        let ticks = labelled(30, 330.0, 40.0);
        assert_eq!(ticks.last(), Some(&29));
        let slot = 330.0 / 30.0;
        assert!(
            ticks
                .windows(2)
                .all(|w| (w[1] - w[0]) as f32 * slot >= 40.0)
        );
        assert_eq!(labelled(7, 700.0, 40.0), (0..7).collect::<Vec<_>>());
    }
}
