use corral_core::Limits;
use corral_ops::SlotUsage;

use super::{slot_badge, slot_summary};

fn usage(tasks: usize, reviews: usize, gates: usize, reservations: usize) -> SlotUsage {
    let mut usage = SlotUsage::from_tasks(&[], &Limits::default());
    usage.tasks = tasks;
    usage.reviews = reviews;
    usage.gates = gates;
    usage.reservations = reservations;
    usage
}

#[test]
fn summary_lists_only_nonzero_components() {
    let limits = Limits::default();
    assert_eq!(slot_summary(&usage(0, 0, 0, 0), &limits), "0/3 used");
    assert_eq!(
        slot_summary(&usage(1, 2, 0, 1), &limits),
        "4/3 used: 1 task · 2 reviews · 1 reserved"
    );
}

#[test]
fn summary_marks_work_kept_off_the_count() {
    let limits = Limits {
        count_reviews: false,
        ..Limits::default()
    };
    let mut u = SlotUsage::from_tasks(&[], &limits);
    u.tasks = 2;
    u.reviews = 1;
    u.gates = 1;
    assert_eq!(u.total(), 3, "the review is not counted");
    assert_eq!(
        slot_summary(&u, &limits),
        "3/3 used: 2 tasks · 1 review uncounted · 1 gate"
    );
}

#[test]
fn soft_target_below_the_hard_limit_is_shown() {
    let limits = Limits {
        running: 5,
        running_target: Some(3),
        ..Limits::default()
    };
    assert_eq!(
        slot_summary(&usage(2, 0, 0, 0), &limits),
        "2/5 used (soft target 3): 2 tasks"
    );
    assert_eq!(
        slot_badge(&usage(2, 0, 0, 0), &limits, 80),
        " 2/5 (soft target 3) "
    );
    let equal = Limits {
        running_target: Some(3),
        ..Limits::default()
    };
    assert_eq!(slot_summary(&usage(0, 0, 0, 0), &equal), "0/3 used");
}

#[test]
fn badge_shrinks_to_the_space_it_has() {
    let limits = Limits::default();
    let tasks_only = usage(2, 0, 0, 0);
    assert_eq!(slot_badge(&tasks_only, &limits, 80), " 2/3 ");
    let mixed = usage(1, 1, 1, 0);
    assert_eq!(
        slot_badge(&mixed, &limits, 80),
        " 1 task · 1 review · 1 gate · 3/3 "
    );
    assert_eq!(slot_badge(&mixed, &limits, 16), " t1 r1 g1 · 3/3 ");
    assert_eq!(slot_badge(&mixed, &limits, 8), " 3/3 ");
}
