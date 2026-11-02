//! The same exact work/output assertions for each action, warm and in a fresh process.
use super::{Kind, budget};

#[test]
fn worker_warm() {
    budget(Kind::Worker, false);
}

#[test]
fn worker_cold() {
    budget(Kind::Worker, true);
}

#[test]
fn planner_warm() {
    budget(Kind::Planner, false);
}

#[test]
fn planner_cold() {
    budget(Kind::Planner, true);
}

#[test]
fn plan_review_pane_warm() {
    budget(Kind::PlanReviewPane, false);
}

#[test]
fn plan_review_pane_cold() {
    budget(Kind::PlanReviewPane, true);
}

#[test]
fn plan_review_pm_warm() {
    budget(Kind::PlanReviewPm, false);
}

#[test]
fn plan_review_pm_cold() {
    budget(Kind::PlanReviewPm, true);
}

#[test]
fn accept_plan_warm() {
    budget(Kind::AcceptPlan, false);
}

#[test]
fn accept_plan_cold() {
    budget(Kind::AcceptPlan, true);
}

#[test]
fn accept_plan_scopes_warm() {
    budget(Kind::AcceptPlanScopes, false);
}

#[test]
fn accept_plan_scopes_cold() {
    budget(Kind::AcceptPlanScopes, true);
}

#[test]
fn repair_legacy_warm() {
    budget(Kind::RepairLegacy, false);
}

#[test]
fn repair_legacy_cold() {
    budget(Kind::RepairLegacy, true);
}

#[test]
fn repair_inbox_warm() {
    budget(Kind::RepairInbox, false);
}

#[test]
fn repair_inbox_cold() {
    budget(Kind::RepairInbox, true);
}

#[test]
fn aggregate_code_warm() {
    budget(Kind::AggregateCode, false);
}

#[test]
fn aggregate_code_cold() {
    budget(Kind::AggregateCode, true);
}

#[test]
fn aggregate_no_code_warm() {
    budget(Kind::AggregateNoCode, false);
}

#[test]
fn aggregate_no_code_cold() {
    budget(Kind::AggregateNoCode, true);
}

#[test]
fn gate_warm() {
    budget(Kind::Gate, false);
}

#[test]
fn gate_cold() {
    budget(Kind::Gate, true);
}

#[test]
fn reviewer_pane_warm() {
    budget(Kind::ReviewerPane, false);
}

#[test]
fn reviewer_pane_cold() {
    budget(Kind::ReviewerPane, true);
}

#[test]
fn reviewer_pm_warm() {
    budget(Kind::ReviewerPm, false);
}

#[test]
fn reviewer_pm_cold() {
    budget(Kind::ReviewerPm, true);
}

#[test]
fn recovered_pane_warm() {
    budget(Kind::RecoveredPane, false);
}

#[test]
fn recovered_pane_cold() {
    budget(Kind::RecoveredPane, true);
}

#[test]
fn recovered_pm_warm() {
    budget(Kind::RecoveredPm, false);
}

#[test]
fn recovered_pm_cold() {
    budget(Kind::RecoveredPm, true);
}

#[test]
fn land_warm() {
    budget(Kind::Land, false);
}

#[test]
fn land_cold() {
    budget(Kind::Land, true);
}

#[test]
fn carry_warm() {
    budget(Kind::Carry, false);
}

#[test]
fn carry_cold() {
    budget(Kind::Carry, true);
}

#[test]
fn steady_warm() {
    budget(Kind::Steady, false);
}

#[test]
fn steady_cold() {
    budget(Kind::Steady, true);
}

#[test]
fn repair_tick_legacy_warm() {
    budget(Kind::RepairTickLegacy, false);
}

#[test]
fn repair_tick_legacy_cold() {
    budget(Kind::RepairTickLegacy, true);
}

#[test]
fn repair_tick_inbox_warm() {
    budget(Kind::RepairTickInbox, false);
}

#[test]
fn repair_tick_inbox_cold() {
    budget(Kind::RepairTickInbox, true);
}
