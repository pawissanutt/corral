use super::*;

fn budget() -> Expected {
    Expected {
        tasks: 21,
        sample_stats: 50,
        volumes: BTreeSet::new(),
    }
}

fn reduced_work() -> Cells {
    Cells::from([
        ("calls.FU-3.sample".into(), 3),
        ("file_stats.followup.FU-3.sample".into(), 150),
    ])
}

#[test]
fn resource_samples_decode_no_task_rows() {
    assert!(followup_failures("base", "steady/pm_next", &reduced_work(), &budget()).is_empty());
}

#[test]
fn v2_still_refuses_hydration_sample_decodes_processes_and_unexpected_work() {
    for (key, value) in [
        ("full_task_hydrations.followup.FU-3.sample", 1),
        ("task_head_decodes.followup.FU-3.sample", 1),
        ("decoded_doc_bytes.followup.FU-3.sample", 1),
        ("processes.followup.FU-3.sample", 1),
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
