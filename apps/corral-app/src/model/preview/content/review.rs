use super::*;

pub(super) fn reviews(feed: &mut Feed) {
    for (id, sha, state, approvable) in [
        ("T-868", "85b45e1e", "needs_owner", true),
        ("T-890", "1751bd2", "gating", false),
        ("T-894", "6b36916", "gating", false),
        ("T-867", "5949a4b", "stale", false),
    ] {
        if let Some(mut row) = feed.board.task(id).cloned() {
            row.rail[0].mark = Mark::Done;
            row.rail[0].sha7 = Some(sha.into());
            if approvable {
                row.now_code = "review".into();
                row.rail[1].mark = Mark::Done;
                row.rail[2].mark = Mark::Done;
                let key = format!("review:{id}:{sha}");
                owner_move(
                    feed,
                    &key,
                    id,
                    row.plan.as_deref(),
                    "Your verdict on the candidate",
                    InboxKind::Review,
                    AttentionDestination::Review,
                );
                row.attention.push(key);
            }
            save_task(feed, row);
            let target = DetailRef::Task { id: id.into() };
            if let Some(DetailBody::Task(mut task)) =
                feed.detail(&target).and_then(|cache| cache.body()).cloned()
            {
                task.run = Some(TaskRun {
                    id: "R1".into(),
                    session: Some(
                        match id {
                            "T-890" => "gate-T-890",
                            "T-894" => "gate-T-894",
                            _ => id,
                        }
                        .into(),
                    ),
                    profile: Some("pi-sol xhigh".into()),
                    model: Some("gpt-6.1-sol".into()),
                    effort: Some("xhigh".into()),
                    base: Some("0123456789abcdef".into()),
                    head: Some(sha.into()),
                    started_at: Some(stamp(feed, feed.now.saturating_sub(10800))),
                    ended_at: Some(stamp(feed, feed.now.saturating_sub(3600))),
                });
                put(feed, target, DetailBody::Task(task));
            }
        }
        let path = match id {
            "T-868" => "packages/corral-pi/test/cross-lane.test.ts",
            "T-890" => "crates/corral-ops/src/pm/read_view.rs",
            "T-894" => "crates/corral-core/src/config/connections.rs",
            _ => "crates/corral-core/src/queue/contract.rs",
        };
        let patch = if id == "T-868" {
            "@@ -212,3 +212,3 @@ describe(\"cross-lane paging\")\n-  const inputs = 256;\n+  const inputs = 128;\n   for (let i = 0; i < inputs; i++) await submit(lane, `msg ${i}`);\n".to_owned()
        } else {
            format!(
                "@@ -1 +1 @@\n-// previous {} contract\n+// preview candidate {sha}: {} contract\n",
                id,
                feed.board.task(id).map_or(id, |task| task.title.as_str())
            )
        };
        put(
            feed,
            DetailRef::Review { task: id.into() },
            DetailBody::Review(ReviewDetail {
                task: id.into(),
                candidate_sha: Some(sha.into()),
                base: Some("0123456789abcdef".into()),
                state: state.into(),
                verdicts: vec![ReviewVerdict {
                    by: "review · pi-sol".into(),
                    sha: sha.into(),
                    verdict: if approvable { "approve" } else { "pending" }.into(),
                    at: Some(stamp(feed, feed.now.saturating_sub(3600))),
                    text: Some("Preview · candidate-bound reviewer record".into()),
                    stale: false,
                }],
                gates: vec![ReviewGate {
                    by: "quick tier".into(),
                    sha: sha.into(),
                    verdict: if approvable {
                        "pass"
                    } else if id == "T-867" {
                        "stale"
                    } else {
                        "running"
                    }
                    .into(),
                    at: Some(stamp(feed, feed.now.saturating_sub(1800))),
                    text: Some("Preview · receipt 77b8a3c1".into()),
                    stale: id == "T-867",
                }],
                files: vec![ReviewFile {
                    path: path.into(),
                    added: 1,
                    deleted: 1,
                }],
                report: Some(if id == "T-868" {
                    "Preview · 256 → 128 inputs keeps every assertion and production frame bound unchanged. Original failed candidate remains in history.".into()
                } else {
                    format!("Preview · candidate {sha} for {id} · {state}")
                }),
                acceptance: vec![format!("Preview · preserve the {id} consumer contract")],
                gate_log_tail: vec![format!("Preview · {id} candidate {sha} · {state}")],
                artifacts: vec![],
                annotations: vec![],
                approvable,
            }),
        );
        put(
            feed,
            DetailRef::ReviewFile {
                task: id.into(),
                path: path.into(),
            },
            DetailBody::ReviewFile(PatchPage {
                path: path.into(),
                patch,
                truncated: false,
                candidate: Some(ReviewCandidate {
                    run: "R1".into(),
                    base: "0123456789abcdef".into(),
                    head: sha.into(),
                }),
            }),
        );
    }
}
