//! T-328: slim verdict notes recorded through the verdict entry point, read by every report.

use super::*;

#[test]
fn a_twenty_kib_verdict_is_one_blob_that_digest_stats_sprint_and_log_read_from_rows() {
    // Given a reviewed run whose reviewer records a 20 KiB approval through the verdict entry point.
    let fx = Fx::new();
    fx.review();
    let notes = "checked-and-approved ".repeat(1024).trim_end().to_owned();
    assert!(notes.len() >= 20 * 1024);
    corral_ops::verdict(&fx.ctx, &fx.id, Verdict::Approve, &notes).unwrap();
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    let store = &fx.ctx.store;
    let task = store.get(&fx.id).unwrap();
    let run = task.current_run().unwrap();
    // Then the verdict event holds only the BlobRef of the notes, which the row keeps in full.
    let verdicts = store.events_of(Some(&fx.id), &["verdict"]).unwrap();
    assert_eq!(verdicts.len(), 1);
    let blob = corral_core::blob_ref::BlobHash::of(notes.as_bytes()).to_ref();
    assert_eq!(verdicts[0].detail["notes"], blob.to_string());
    assert_eq!(
        run.review.as_ref().unwrap().notes.as_deref(),
        Some(notes.as_str())
    );
    let approved = store.events_of(Some(&fx.id), &["approved"]).unwrap();
    let landed_sha = approved[0].detail["sha"].as_str().unwrap().to_owned();
    assert_eq!(git(&fx.app, &["rev-parse", "main"]).trim(), landed_sha);

    // When log, digest, stats and review-sprint read the history.
    let log = corral_ops::timeline(store, &fx.id, corral_ops::TimelineFilter::All).unwrap();
    let digest = corral_ops::digest(&fx.ctx, "999w").unwrap().digest;
    let stats = corral_ops::stats(store, corral_ops::StatsBy::Task, None).unwrap();
    let sprint = corral_ops::review_sprint(&fx.ctx, None, false)
        .unwrap()
        .sections;

    // Then each equals what the task's rows and the blob give.
    let collapsed = notes.split_whitespace().collect::<Vec<_>>().join(" ");
    let entry = log.iter().find(|entry| entry.kind == "verdict").unwrap();
    assert_eq!(entry.summary, format!("approve {collapsed}"));
    assert_eq!(digest.landed.len(), 1);
    let landed = &digest.landed[0];
    assert_eq!(landed.task.as_deref(), Some(fx.id.as_str()));
    assert_eq!(landed.run.as_deref(), Some(run.id.as_str()));
    assert_eq!(landed.sha.as_deref(), Some(landed_sha.as_str()));
    assert_eq!(landed.verdict.as_deref(), Some("approve"));
    let row = stats.iter().find(|row| row.group == fx.id).unwrap();
    let approved_runs = task
        .runs
        .iter()
        .filter(|r| r.review.as_ref().and_then(|review| review.verdict) == Some(Verdict::Approve))
        .count();
    assert_eq!(row.runs as usize, task.runs.len());
    assert_eq!(row.accepted as usize, approved_runs);
    assert_eq!((row.changes_requested, row.discards), (0, 0));
    assert_eq!(row.first_round_accept_rate, Some(1.0));
    assert_eq!(row.mean_rounds_to_accept, Some(1.0));
    let change = sprint
        .landed
        .iter()
        .find(|change| change.task.as_deref() == Some(fx.id.as_str()))
        .unwrap();
    assert_eq!(change.sha.as_deref(), Some(landed_sha.as_str()));
    assert_eq!(change.verdict.as_deref(), Some("approve"));
    assert_eq!(change.reason.as_deref(), Some(notes.as_str()));
}
