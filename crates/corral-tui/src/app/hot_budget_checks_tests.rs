use super::*;

fn budget() -> Expected {
    Expected {
        tasks: 21,
        walk_rows: 21,
        walk_bytes: 8000,
        sample_stats: 50,
        likely_per_shortlist: 4,
        volumes: BTreeSet::new(),
        walk_bytes_after: None,
    }
}

fn reduced_work() -> Cells {
    Cells::from([
        ("calls.FU-3.likely_files".into(), 4),
        ("calls.FU-3.sample".into(), 3),
        ("task_head_decodes.followup.FU-3.likely_files".into(), 21),
        ("decoded_doc_bytes.followup.FU-3.likely_files".into(), 7600),
        ("file_stats.followup.FU-3.sample".into(), 150),
    ])
}

#[test]
fn v2_header_reads_below_the_per_call_budget_are_valid() {
    // Four likely-files calls may reuse a single header scan; samples decode no task rows.
    assert!(followup_failures("base", "steady/pm_next", &reduced_work(), &budget()).is_empty());
}

#[test]
fn header_decode_upper_bounds_reject_one_extra_row_or_byte() {
    let exp = budget();
    let mut cells = reduced_work();
    for (counter, limit) in [
        ("task_head_decodes", 4 * exp.walk_rows),
        ("decoded_doc_bytes", 4 * exp.walk_bytes),
    ] {
        let key = format!("{counter}.followup.FU-3.likely_files");
        let old = cells.insert(key.clone(), limit).unwrap();
        assert!(followup_failures("base", "steady/pm_next", &cells, &exp).is_empty());
        cells.insert(key.clone(), limit + 1);
        assert!(!followup_failures("base", "steady/pm_next", &cells, &exp).is_empty());
        cells.insert(key, old);
    }
}

#[test]
fn mutation_uses_the_larger_header_byte_bound_without_requiring_a_full_walk() {
    let mut exp = budget();
    exp.walk_bytes_after = Some(exp.walk_bytes + 1);
    let mut cells = reduced_work();
    let key = "decoded_doc_bytes.followup.FU-3.likely_files".to_owned();
    cells.insert(key.clone(), 4 * (exp.walk_bytes + 1));
    assert!(followup_failures("base", "mutation/sync_tick", &cells, &exp).is_empty());
    cells.insert(key, 4 * (exp.walk_bytes + 1) + 1);
    assert!(!followup_failures("base", "mutation/sync_tick", &cells, &exp).is_empty());
}

#[test]
fn v2_still_refuses_hydration_sample_decodes_processes_and_unexpected_work() {
    for (key, value) in [
        ("full_task_hydrations.followup.FU-3.likely_files", 1),
        ("full_task_hydrations.followup.FU-3.sample", 1),
        ("task_head_decodes.followup.FU-3.sample", 1),
        ("decoded_doc_bytes.followup.FU-3.sample", 1),
        ("processes.followup.FU-3.likely_files", 1),
        ("processes.followup.FU-3.sample", 1),
        ("file_stats.followup.FU-3.likely_files", 1),
        ("file_stats.followup.FU-3.sample", 149),
        ("file_stats.followup.FU-3.sample", 151),
        ("views_opened.followup.FU-3.sample", 2),
        ("selected_artifact_bytes_read.total", 1),
    ] {
        let mut cells = reduced_work();
        cells.insert(key.into(), value);
        let failures = followup_failures("base", "steady/pm_next", &cells, &budget());
        assert!(
            !failures.is_empty(),
            "unexpected work accepted: {key}={value}"
        );
    }
}
