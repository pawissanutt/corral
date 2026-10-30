//! Unknown-inventory variants share the parent fixture and assertions unchanged.
use super::assert_unknown_inventory;

#[test]
fn tasks_and_review_capacity_survive_nonzero_inventory() {
    assert_unknown_inventory("nonzero");
}

#[test]
fn tasks_and_review_capacity_survive_timeout_inventory() {
    assert_unknown_inventory("timeout");
}

#[test]
fn tasks_and_review_capacity_survive_malformed_success_inventory() {
    assert_unknown_inventory("malformed");
}

#[test]
fn tasks_and_review_capacity_survive_invalid_json_inventory() {
    assert_unknown_inventory("invalid-json");
}

#[test]
fn tasks_and_review_capacity_survive_empty_inventory() {
    assert_unknown_inventory("empty");
}
